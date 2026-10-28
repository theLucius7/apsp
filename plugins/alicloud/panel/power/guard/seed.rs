use super::super::{
    super::{account_on, billing, client::Cloud, failure, lock, operations, resource},
    jobs,
};
use super::{MAX_BYTES, Observation, key_digest, meter, valid_cycle};
use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, types::Json as SqlJson};
use uuid::Uuid;

pub(in super::super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/plugins/alicloud/resources/{id}/cdt-state",
            post(import),
        )
        .route(
            "/api/plugins/alicloud/resources/{id}/cdt-check",
            post(check),
        )
}

async fn check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    let initial = resource(&state.pool, id).await?;
    let mut tx = lock(&state.pool, initial.account_id).await?;
    let r = jobs::fresh(&mut tx, id).await?;
    let account = account_on(&mut tx, r.account_id).await?;
    let config = r
        .power_policy
        .cdt_guard
        .as_ref()
        .ok_or_else(|| ApiError::BadRequest("请先配置 CDT 脚本守护".into()))?;
    let cloud = Cloud::new(&state.pool).map_err(failure)?;
    let now = apsp_protocol::now_timestamp();
    let power = cloud.power_state(&account, &r).await.map_err(failure)?;
    power.validate("stop", "StopCharging").map_err(failure)?;
    let observation = super::read(&account, &r, &cloud, now)
        .await
        .map_err(failure)?;
    let backlog = if observation.max_jump_bytes >= config.jump_bytes {
        observation.max_jump_bytes
    } else {
        0
    };
    let over =
        observation.max_usage_bytes as u128 + backlog as u128 >= config.threshold_bytes as u128;
    Ok(Json(
        json!({"read_only":true,"observation":observation,"power_state":power,"exceeded":over}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    pub revision: i64,
    pub state: LegacyState,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyState {
    pub version: u32,
    pub identity: Identity,
    pub billing_cycle: String,
    pub max_usage_bytes: String,
    pub max_jump_bytes: String,
    pub observed_at: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub instance_id: String,
    pub region_id: String,
    pub quota_pool: String,
}

async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Input>,
) -> ApiResult<StatusCode> {
    auth::require_admin(&state, &headers).await?;
    import_on(&state.pool, id, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn import_on(pool: &PgPool, id: Uuid, input: Input) -> ApiResult<()> {
    let initial = resource(pool, id).await?;
    let mut tx = lock(pool, initial.account_id).await?;
    let r = jobs::fresh(&mut tx, id).await?;
    let account = account_on(&mut tx, r.account_id).await?;
    if r.kind != "ecs"
        || r.revision != input.revision
        || r.power_policy.enabled
        || r.power_policy.cdt_guard.is_none()
    {
        return Err(ApiError::Conflict(
            "请先保存已暂停的 CDT 脚本守护策略，再导入状态".into(),
        ));
    }
    operations::idle(&mut tx, id).await?;
    let input = input.state;
    let now = apsp_protocol::now_timestamp();
    let parse = |value: &str| -> Option<u64> {
        (!value.is_empty() && value.bytes().all(|v| v.is_ascii_digit()))
            .then(|| value.parse::<u64>().ok())
            .flatten()
            .filter(|v| *v <= MAX_BYTES)
    };
    let (Some(usage), Some(jump)) = (parse(&input.max_usage_bytes), parse(&input.max_jump_bytes))
    else {
        return Err(ApiError::BadRequest(
            "脚本累计量和最大突增必须是整数字节数".into(),
        ));
    };
    if input.version != 1
        || input.identity.instance_id != r.cloud_id
        || input.identity.region_id != r.region
        || input.identity.quota_pool != meter::pool(&r.region).map_err(failure)?
        || !valid_cycle(&input.billing_cycle)
        || input.observed_at <= 0
        || input.observed_at > now
        || billing::month(input.observed_at) != input.billing_cycle
        || jump > usage
    {
        return Err(ApiError::BadRequest(
            "脚本状态身份、版本、账期或观察时间不匹配".into(),
        ));
    }
    let resolved = Cloud::new(pool)
        .map_err(failure)?
        .resolved_account(&account)
        .await
        .map_err(failure)?;
    let mut imported = Observation {
        billing_cycle: input.billing_cycle,
        region: r.region.clone(),
        cloud_id: r.cloud_id.clone(),
        key_digest: key_digest(&resolved.access_key_id),
        account_revision: account.revision,
        max_usage_bytes: usage,
        max_jump_bytes: jump,
        // Imported history preserves protection but is never fresh authorization.
        queried_at: 0,
        cdt_observed: false,
    };
    if let Some(old) = r.cdt_guard_state {
        if old.billing_cycle > imported.billing_cycle {
            return Err(ApiError::Conflict("不能用旧账期覆盖已有 CDT 状态".into()));
        }
        if old.billing_cycle == imported.billing_cycle {
            imported.max_usage_bytes = imported.max_usage_bytes.max(old.max_usage_bytes);
            imported.max_jump_bytes = imported.max_jump_bytes.max(old.max_jump_bytes);
        }
    }
    sqlx::query("UPDATE alicloud_resources SET cdt_guard_state=$2,cdt_guard_error=NULL,next_power_at=0,revision=revision+1 WHERE id=$1")
        .bind(id).bind(SqlJson(imported)).execute(&mut *tx).await?;
    sqlx::query("UPDATE alicloud_power_jobs SET status='cancelled',updated_at=$2 WHERE resource_id=$1 AND status IN ('preview','queued')")
        .bind(id).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
