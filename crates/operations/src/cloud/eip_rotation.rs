mod client;
mod engine;
#[cfg(test)]
mod tests;

use crate::cloud::bandwidth;
use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_networking::security_groups;
use apsp_protocol::now_timestamp;
use apsp_resources::cloud::{
    account_on,
    client::Cloud,
    failure, lock,
    model::{Account, Resource},
    resource,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, post},
};
use client::{Eip, LiveOps, LiveProbe, Ops, Probe, Tally};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction, types::Json as DbJson};
use uuid::Uuid;

pub use engine::run;

const PREVIEW_SECONDS: i64 = 300;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/resources/alicloud/resources/{id}/eip-rotation/preview",
            post(preview),
        )
        .route("/api/resources/alicloud/eip-rotations/{id}", get(detail))
        .route(
            "/api/resources/alicloud/eip-rotations/{id}/confirm",
            post(confirm),
        )
        .route(
            "/api/resources/alicloud/eip-rotations/{id}/cancel",
            post(cancel),
        )
        .route(
            "/api/resources/alicloud/eip-rotations/{id}/reconcile",
            post(reconcile),
        )
}

/// The frozen state a rotation was previewed against, and the spec new addresses copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Before {
    resource_id: Uuid,
    account_id: Uuid,
    account_revision: i64,
    resource_revision: i64,
    instance_id: String,
    region: String,
    server_ids: Vec<i64>,
    link_updated_at: Option<i64>,
    allocation_id: String,
    public_ip: String,
    package_id: Option<String>,
    isp: String,
    netmode: String,
    internet_charge_type: String,
    bandwidth: i64,
    resource_group: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    token: Uuid,
    allocation_id: Option<String>,
    ip: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {
    n: i32,
    allocation_id: String,
    ip: String,
    verdict: String,
    screen: Option<Tally>,
    confirm: Option<Tally>,
}
#[derive(Serialize, FromRow)]
pub struct Rotation {
    id: Uuid,
    resource_id: Uuid,
    requested_by: i64,
    #[sqlx(rename = "before_state")]
    before: DbJson<Before>,
    probe_port: i32,
    max_attempts: i32,
    snapshot_digest: String,
    status: String,
    phase: String,
    phase_started_at: i64,
    attempt: i32,
    bound_id: String,
    candidate: Option<DbJson<Candidate>>,
    measurement_id: Option<String>,
    drop_ids: Vec<String>,
    after_drop: Option<String>,
    attempts: DbJson<Vec<Attempt>>,
    new_ip: Option<String>,
    error_code: Option<String>,
    created_at: i64,
    expires_at: i64,
    updated_at: i64,
    next_check_at: i64,
    reconciliation: Option<Value>,
    /// Bumped by every stored step; a step stores only over the version it read.
    #[serde(skip_serializing)]
    version: i64,
}

fn digest(before: &Before, port: i32, attempts: i32) -> ApiResult<String> {
    let value = json!({"before":before,"probe_port":port,"max_attempts":attempts});
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&value).map_err(anyhow::Error::from)?)
    ))
}

/// New addresses copy only these fields, so anything else would be silently lost.
fn unsupported(eip: &Eip, instance: &str) -> Option<&'static str> {
    if eip.status != "InUse" || eip.instance_id.as_deref() != Some(instance) {
        Some("EIP 与实例的绑定关系已变化，请刷新后重试")
    } else if eip.charge_type != "PostPaid" {
        Some("只支持按量付费的 EIP；包年包月 EIP 不能由此释放")
    } else if eip.mode != "NAT" {
        Some("只支持 NAT 模式的 EIP，EIP 网卡可见模式不支持")
    } else if eip.business_status != "Normal" {
        Some("EIP 当前业务状态异常，请先在阿里云核对")
    } else if eip.deletion_protection {
        Some("EIP 开启了删除保护，不能自动释放；请先在阿里云关闭")
    } else if eip.security_protection || eip.address_pool || eip.segment {
        Some("EIP 使用了安全防护、地址池或连续 EIP，新地址无法保持相同配置")
    } else if eip.package_id.is_none()
        && !matches!(
            eip.internet_charge_type.as_str(),
            "PayByTraffic" | "PayByBandwidth"
        )
    {
        // Inside a shared bandwidth package the package bills the traffic and the
        // EIP may report the package's method; a new address joins the same package.
        Some("EIP 计费方式不受支持")
    } else {
        None
    }
}

async fn observe<O: Ops>(
    pool: &PgPool,
    ops: &O,
    account: &Account,
    resource: &Resource,
) -> ApiResult<Before> {
    let instance = ops.instance().await.map_err(failure)?;
    if instance.status != "Running" {
        return Err(ApiError::Conflict(
            "实例需要处于运行中，才能确认新地址可用".into(),
        ));
    }
    let id = instance.attached_eip.ok_or_else(|| {
        ApiError::Conflict("此实例的公网地址不是 EIP；固定公网 IP 不能在此更换".into())
    })?;
    let eip = ops
        .eip(&id)
        .await
        .map_err(|error| {
            // The CDT policy grants no VPC actions, so this is where a missing grant shows.
            if error.code == "authentication_failed" {
                ApiError::Conflict(
                    "无法读取实例绑定的 EIP：访问密钥缺少 VPC 权限。更换公网 IP 需要另外授予申请、换绑和释放 EIP 的权限，见文档「RAM 最小权限」中的「更换公网 IP 的额外权限」".into(),
                )
            } else {
                failure(error)
            }
        })?
        .ok_or_else(|| ApiError::Conflict("未读取到实例绑定的 EIP，请刷新后重试".into()))?;
    if let Some(reason) = unsupported(&eip, &resource.cloud_id) {
        return Err(ApiError::Conflict(reason.into()));
    }
    let registered: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alicloud_resources WHERE kind='eip' AND cloud_id=$1 AND NOT archived)")
        .bind(&id).fetch_one(pool).await?;
    if registered {
        return Err(ApiError::Conflict(
            "此 EIP 已单独登记为云资源；请先移除登记，避免释放后留下失效记录".into(),
        ));
    }
    let (server_ids, link_updated_at) = security_groups::linked(pool, resource.id).await?;
    Ok(Before {
        resource_id: resource.id,
        account_id: account.id,
        account_revision: account.revision,
        resource_revision: resource.revision,
        instance_id: resource.cloud_id.clone(),
        region: resource.region.clone(),
        server_ids,
        link_updated_at,
        allocation_id: id,
        public_ip: eip.ip,
        package_id: eip.package_id,
        isp: eip.isp,
        netmode: eip.netmode,
        internet_charge_type: eip.internet_charge_type,
        bandwidth: eip.bandwidth,
        resource_group: eip.resource_group,
    })
}

async fn permission(
    state: &AppState,
    headers: &HeaderMap,
    id: Uuid,
    historical: &[i64],
    write: bool,
) -> ApiResult<i64> {
    let actor = control_center::authenticate(state, headers).await?;
    if !actor.global_servers() || !actor.allows("cloud:read") {
        return Err(ApiError::Forbidden("更换公网 IP 需要全局云读取授权".into()));
    }
    if write && actor.token_id.is_some() {
        return Err(ApiError::Forbidden(
            "更换公网 IP 需要管理员会话，API 令牌不能确认".into(),
        ));
    }
    let capability = if write { "cloud:write" } else { "cloud:read" };
    control_center::require_capability(state, headers, capability).await?;
    let (mut ids, _) = security_groups::linked(&state.pool, id).await?;
    ids.extend(historical);
    ids.sort_unstable();
    ids.dedup();
    for server in ids {
        control_center::require_server(state, headers, server, capability).await?;
    }
    Ok(actor.admin_id)
}

/// Whether an unfinished rotation may still bind or release this EIP: the
/// address it started from, the one bound now, a candidate, or one it allocated.
pub async fn involves(tx: &mut Transaction<'_, Postgres>, allocation: &str) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alicloud_eip_rotations WHERE status IN ('running','unknown') AND (before_state->>'allocation_id'=$1 OR bound_id=$1 OR $1=ANY(drop_ids) OR candidate->>'allocation_id'=$1 OR EXISTS(SELECT 1 FROM jsonb_array_elements(attempts) a WHERE a->>'allocation_id'=$1)))")
        .bind(allocation).fetch_one(&mut **tx).await?)
}

/// Cancels the resources' address rotations that are still in preview.
pub async fn cancel_previews(
    tx: &mut Transaction<'_, Postgres>,
    resources: &[Uuid],
    now: i64,
) -> ApiResult<()> {
    sqlx::query("UPDATE alicloud_eip_rotations SET status='cancelled',updated_at=$2 WHERE resource_id=ANY($1) AND status='preview'").bind(resources).bind(now).execute(&mut **tx).await?;
    Ok(())
}

/// The latest address rotations, unfinished ones first.
pub async fn recent(pool: &PgPool) -> ApiResult<Vec<Rotation>> {
    Ok(sqlx::query_as("SELECT * FROM alicloud_eip_rotations ORDER BY (status IN ('running','unknown')) DESC,created_at DESC,id LIMIT 50").fetch_all(pool).await?)
}

async fn load(pool: &PgPool, id: Uuid) -> ApiResult<Rotation> {
    sqlx::query_as("SELECT * FROM alicloud_eip_rotations WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound)
}

async fn context(pool: &PgPool, id: Uuid) -> ApiResult<(Account, Resource)> {
    let resource = resource(pool, id).await?;
    if resource.kind != "ecs" {
        return Err(ApiError::Conflict(
            "更换公网 IP 只支持已登记的 ECS 实例".into(),
        ));
    }
    let mut tx = lock(pool, resource.account_id).await?;
    let account = account_on(&mut tx, resource.account_id).await?;
    tx.commit().await?;
    Ok((account, resource))
}

async fn row_context(pool: &PgPool, id: Uuid) -> ApiResult<(Account, Resource)> {
    context(pool, load(pool, id).await?.resource_id).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    probe_port: u16,
    max_attempts: i32,
    resource_revision: i64,
}

async fn preview_with<O: Ops, P: Probe>(
    pool: &PgPool,
    actor: i64,
    account: &Account,
    resource: &Resource,
    input: Preview,
    ops: &O,
    probe: &P,
) -> ApiResult<Rotation> {
    if input.probe_port == 0 || !(1..=12).contains(&input.max_attempts) {
        return Err(ApiError::BadRequest(
            "请填写有效的探测端口，候选数量为 1–12 个".into(),
        ));
    }
    if !account.enabled {
        return Err(ApiError::Conflict("云账号已停用".into()));
    }
    if resource.revision != input.resource_revision {
        return Err(ApiError::Conflict("资源配置已变化，请刷新后重试".into()));
    }
    let before = observe(pool, ops, account, resource).await?;
    // A closed port would make every candidate look blocked and waste all of them.
    if !probe.reachable(&before.public_ip, input.probe_port).await {
        return Err(ApiError::Conflict(format!(
            "面板无法连接当前地址的 {} 端口。请填写实例上正在监听且安全组放行的 TCP 端口",
            input.probe_port
        )));
    }
    let port = i32::from(input.probe_port);
    let hash = digest(&before, port, input.max_attempts)?;
    let mut tx = lock(pool, account.id).await?;
    bandwidth::idle(&mut tx, resource.id).await?;
    let id = Uuid::new_v4();
    let now = now_timestamp();
    sqlx::query("UPDATE alicloud_eip_rotations SET status='cancelled',updated_at=$2 WHERE resource_id=$1 AND status='preview'")
        .bind(resource.id).bind(now).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO alicloud_eip_rotations(id,resource_id,requested_by,before_state,probe_port,max_attempts,snapshot_digest,status,bound_id,created_at,expires_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'preview',$8,$9,$10,$9)")
        .bind(id).bind(resource.id).bind(actor).bind(DbJson(&before)).bind(port).bind(input.max_attempts)
        .bind(hash).bind(&before.allocation_id).bind(now).bind(now + PREVIEW_SECONDS).execute(&mut *tx).await?;
    tx.commit().await?;
    load(pool, id).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirm {
    confirm: bool,
    snapshot_digest: String,
}

async fn confirm_with<O: Ops>(
    pool: &PgPool,
    actor: i64,
    id: Uuid,
    input: Confirm,
    ops: &O,
) -> ApiResult<Rotation> {
    let initial = load(pool, id).await?;
    if actor != initial.requested_by {
        return Err(ApiError::Forbidden(
            "更换公网 IP 只允许原发起管理员确认".into(),
        ));
    }
    if !input.confirm || input.snapshot_digest != initial.snapshot_digest {
        return Err(ApiError::Conflict("请明确确认完整的预览摘要".into()));
    }
    // A submitted intent is never submitted again.
    if initial.status != "preview" {
        return Ok(initial);
    }
    if initial.expires_at <= now_timestamp() {
        return Err(ApiError::Conflict("预览已过期，请重新预览".into()));
    }
    let (account, resource) = context(pool, initial.resource_id).await?;
    if !account.enabled {
        return Err(ApiError::Conflict("云账号已停用".into()));
    }
    let fresh = observe(pool, ops, &account, &resource).await?;
    if fresh != initial.before.0 {
        return Err(ApiError::Conflict(
            "EIP、实例或配置已变化，请重新预览".into(),
        ));
    }
    let mut tx = lock(pool, account.id).await?;
    let current: Rotation =
        sqlx::query_as("SELECT * FROM alicloud_eip_rotations WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if current.status != "preview" {
        tx.commit().await?;
        return Ok(current);
    }
    bandwidth::idle(&mut tx, resource.id).await?;
    let now = now_timestamp();
    sqlx::query("UPDATE alicloud_eip_rotations SET status='running',phase='allocate',phase_started_at=$2,updated_at=$2,next_check_at=$2 WHERE id=$1")
        .bind(id).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    load(pool, id).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reconcile {
    process_stopped: bool,
    evidence: String,
}

/// Closes a halted rotation on a person's word. It never releases or binds anything:
/// addresses still listed here must be cleaned up in the cloud console.
async fn reconcile_with<O: Ops>(
    pool: &PgPool,
    actor: i64,
    id: Uuid,
    input: Reconcile,
    ops: &O,
) -> ApiResult<Rotation> {
    let initial = load(pool, id).await?;
    if actor != initial.requested_by {
        return Err(ApiError::Forbidden("只允许原发起管理员核对".into()));
    }
    if initial.status != "unknown" {
        return Err(ApiError::Conflict("此操作不需要人工核对".into()));
    }
    if !input.process_stopped || input.evidence.trim().len() < 32 || input.evidence.len() > 4096 {
        return Err(ApiError::BadRequest(
            "请确认已在云端核对，并写下核对方法与结论（32–4096 字节）".into(),
        ));
    }
    let instance = ops.instance().await.map_err(failure)?;
    let (account, _) = context(pool, initial.resource_id).await?;
    let mut tx = lock(pool, account.id).await?;
    let now = now_timestamp();
    let allocated: Vec<&str> = initial
        .attempts
        .iter()
        .map(|a| a.allocation_id.as_str())
        .collect();
    let evidence = json!({"observed_at":now,"attached_eip":instance.attached_eip,"instance_status":instance.status,
        "original_eip":initial.before.allocation_id,"allocated_by_rotation":allocated,"evidence":input.evidence,"automatic_cleanup":false});
    let changed = sqlx::query("UPDATE alicloud_eip_rotations SET status='reconciled',reconciliation=$2,updated_at=$3 WHERE id=$1 AND status='unknown'")
        .bind(id).bind(evidence).bind(now).execute(&mut *tx).await?.rows_affected();
    tx.commit().await?;
    if changed == 0 {
        return Err(ApiError::Conflict("操作已经核对或结束".into()));
    }
    load(pool, id).await
}

async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Preview>,
) -> ApiResult<Json<Rotation>> {
    let actor = permission(&state, &headers, id, &[], true).await?;
    let (account, resource) = context(&state.pool, id).await?;
    let cloud = Cloud::new(&state.pool).map_err(failure)?;
    let ops = LiveOps::new(&cloud, &account, &resource)
        .await
        .map_err(failure)?;
    let probe = LiveProbe::new().map_err(failure)?;
    preview_with(&state.pool, actor, &account, &resource, input, &ops, &probe)
        .await
        .map(Json)
}

async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Rotation>> {
    let value = load(&state.pool, id).await?;
    permission(
        &state,
        &headers,
        value.resource_id,
        &value.before.server_ids,
        false,
    )
    .await?;
    Ok(Json(value))
}

async fn confirm(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Confirm>,
) -> ApiResult<Json<Rotation>> {
    let row = load(&state.pool, id).await?;
    let actor = permission(
        &state,
        &headers,
        row.resource_id,
        &row.before.server_ids,
        true,
    )
    .await?;
    let (account, resource) = context(&state.pool, row.resource_id).await?;
    let cloud = Cloud::new(&state.pool).map_err(failure)?;
    let ops = LiveOps::new(&cloud, &account, &resource)
        .await
        .map_err(failure)?;
    confirm_with(&state.pool, actor, id, input, &ops)
        .await
        .map(Json)
}

async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Rotation>> {
    let row = load(&state.pool, id).await?;
    permission(
        &state,
        &headers,
        row.resource_id,
        &row.before.server_ids,
        true,
    )
    .await?;
    let changed = sqlx::query("UPDATE alicloud_eip_rotations SET status='cancelled',updated_at=$2 WHERE id=$1 AND status='preview'")
        .bind(id).bind(now_timestamp()).execute(&state.pool).await?.rows_affected();
    if changed == 0 {
        return Err(ApiError::Conflict("此操作已经开始或结束，不能取消".into()));
    }
    load(&state.pool, id).await.map(Json)
}

async fn reconcile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Reconcile>,
) -> ApiResult<Json<Rotation>> {
    let row = load(&state.pool, id).await?;
    let actor = permission(
        &state,
        &headers,
        row.resource_id,
        &row.before.server_ids,
        true,
    )
    .await?;
    let (account, resource) = context(&state.pool, row.resource_id).await?;
    let cloud = Cloud::new(&state.pool).map_err(failure)?;
    let ops = LiveOps::new(&cloud, &account, &resource)
        .await
        .map_err(failure)?;
    reconcile_with(&state.pool, actor, id, input, &ops)
        .await
        .map(Json)
}
