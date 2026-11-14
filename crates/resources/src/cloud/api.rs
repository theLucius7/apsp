use super::{
    account_on, billing,
    client::Cloud,
    failure, lock,
    model::{self, Account, Resource},
    resource,
};
use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{patch, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/resources/alicloud/accounts", post(create_account))
        .route(
            "/api/resources/alicloud/accounts/{id}",
            patch(update_account).delete(remove_account),
        )
        .route(
            "/api/resources/alicloud/accounts/{id}/refresh",
            post(refresh_account),
        )
        .route("/api/resources/alicloud/resources", post(create_resource))
        .route(
            "/api/resources/alicloud/resources/{id}",
            patch(update_resource).delete(remove_resource),
        )
        .route(
            "/api/resources/alicloud/resources/{id}/refresh",
            post(refresh_resource),
        )
}
/// Every resource registered under the account, archived ones included: their pending work
/// follows the account's configuration.
async fn account_resources(
    tx: &mut Transaction<'_, Postgres>,
    account: Uuid,
) -> ApiResult<Vec<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM alicloud_resources WHERE account_id=$1")
            .bind(account)
            .fetch_all(&mut **tx)
            .await?,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    revision: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountWrite {
    name: String,
    site: String,
    enabled: bool,
    auto_enabled: bool,
    billing_enabled: Option<bool>,
    balance_enabled: Option<bool>,
    limit_gb: i64,
    credential_id: Option<Uuid>,
    revision: Option<i64>,
}
impl AccountWrite {
    fn validate(&self) -> ApiResult<()> {
        model::label(&self.name)?;
        if !matches!(self.site.as_str(), "china" | "international")
            || !(1..=1_000_000_000).contains(&self.limit_gb)
        {
            return Err(ApiError::BadRequest(
                "请选择账号站点，并填写 1–1000000000 GB 的自动降速阈值".into(),
            ));
        }
        Ok(())
    }
    /// The credential reference to store: the one submitted, or on an edit the stored one.
    fn credential(&self, previous: Option<&Account>) -> ApiResult<Uuid> {
        self.credential_id
            .or_else(|| previous.and_then(|account| account.credential_id))
            .ok_or_else(|| ApiError::BadRequest("请引用集中云凭据".into()))
    }
}
async fn validate_reference(state: &AppState, id: Uuid) -> ApiResult<()> {
    let available: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM credential_entries WHERE id=$1 AND kind='cloud' AND enabled)",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    if !available {
        return Err(ApiError::BadRequest(
            "集中云凭据不存在、已停用或用途不符".into(),
        ));
    }
    Ok(())
}
async fn create_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<AccountWrite>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    auth::require_admin(&state, &headers).await?;
    input.validate()?;
    let settings = billing::Settings {
        billing_enabled: input.billing_enabled.unwrap_or(false),
        balance_enabled: input.balance_enabled.unwrap_or(false),
    };
    if input.auto_enabled && !settings.billing_enabled {
        return Err(ApiError::BadRequest("自动降速需要先启用账单查询".into()));
    }
    let credential = input.credential(None)?;
    validate_reference(&state, credential).await?;
    let id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    crate::locks::ALICLOUD_ACCOUNTS.lock(&mut tx).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM alicloud_accounts WHERE NOT archived")
            .fetch_one(&mut *tx)
            .await?;
    if count >= 8 {
        return Err(ApiError::Conflict("最多登记 8 个云账号".into()));
    }
    sqlx::query("INSERT INTO alicloud_accounts(id,name,site,enabled,auto_enabled,limit_gb,credential_id) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(id).bind(input.name.trim()).bind(input.site).bind(input.enabled).bind(input.auto_enabled).bind(input.limit_gb).bind(credential).execute(&mut *tx).await?;
    billing::changed(&mut tx, id, billing::Change::Created(settings)).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(json!({"id":id}))))
}
async fn update_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<AccountWrite>,
) -> ApiResult<StatusCode> {
    auth::require_admin(&state, &headers).await?;
    input.validate()?;
    let mut tx = lock(&state.pool, id).await?;
    let previous = account_on(&mut tx, id).await?;
    if input.revision != Some(previous.revision) {
        return Err(ApiError::Conflict("账号配置已变化，请刷新后重试".into()));
    }
    let stored = billing::state(&mut tx, id).await?.settings;
    let settings = billing::Settings {
        billing_enabled: input.billing_enabled.unwrap_or(stored.billing_enabled),
        balance_enabled: input.balance_enabled.unwrap_or(stored.balance_enabled),
    };
    if !settings.billing_enabled {
        let uses_bills: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alicloud_resources WHERE account_id=$1 AND NOT archived AND power_policy->>'enabled'='true' AND power_policy->>'threshold_action' IN ('notify','stop'))")
            .bind(id).fetch_one(&mut *tx).await?;
        if input.auto_enabled || uses_bills {
            return Err(ApiError::BadRequest(
                "请先关闭自动降速及资源账单阈值策略，再关闭账单查询".into(),
            ));
        }
    }
    let credential = input.credential(Some(&previous))?;
    let resources = account_resources(&mut tx, id).await?;
    if Some(credential) != previous.credential_id {
        validate_reference(&state, credential).await?;
    }
    let rekeyed = input.site != previous.site || Some(credential) != previous.credential_id;
    if rekeyed && super::work::unresolved(&mut tx, &resources).await? {
        return Err(ApiError::Conflict(
            "此账号仍有已发送或结果待核对的云操作，请先核对或结束跟踪，再更换站点及访问密钥".into(),
        ));
    }
    // Any edit invalidates queued authorization and cached billing evidence.
    if rekeyed {
        sqlx::query("UPDATE alicloud_resources SET ipv6_addresses='[]',ipv6_checked_at=NULL,ipv6_error=NULL WHERE account_id=$1").bind(id).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE alicloud_accounts SET name=$2,site=$3,enabled=$4,auto_enabled=$5,limit_gb=$6,credential_id=$7,revision=revision+1,next_run_at=0 WHERE id=$1")
        .bind(id).bind(input.name.trim()).bind(input.site).bind(input.enabled).bind(input.auto_enabled).bind(input.limit_gb).bind(credential).execute(&mut *tx).await?;
    billing::changed(
        &mut tx,
        id,
        billing::Change::Edited {
            settings,
            rekeyed,
            resources: &resources,
        },
    )
    .await?;
    super::work::cancel_pending(&mut tx, &resources, true).await?;
    sqlx::query(
        "UPDATE alicloud_resources SET next_power_at=0,cdt_guard_next_at=0 WHERE account_id=$1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn remove_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    input: Option<Json<Revision>>,
) -> ApiResult<StatusCode> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = lock(&state.pool, id).await?;
    let current = account_on(&mut tx, id).await?;
    if input.is_some_and(|Json(input)| input.revision != current.revision) {
        return Err(ApiError::Conflict("账号配置已变化，请刷新后重试".into()));
    }
    let has_resources: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM alicloud_resources WHERE account_id=$1 AND NOT archived)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if has_resources {
        return Err(ApiError::Conflict("请先移除该账号登记的云资源".into()));
    }
    sqlx::query("UPDATE alicloud_accounts SET archived=true,enabled=false,auto_enabled=false,credential_id=NULL WHERE id=$1").bind(id).execute(&mut *tx).await?;
    billing::changed(&mut tx, id, billing::Change::Archived).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn refresh_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = lock(&state.pool, id).await?;
    let account = account_on(&mut tx, id).await?;
    if !account.enabled {
        return Err(ApiError::Conflict("请先启用云账号".into()));
    }
    let last: i64 = sqlx::query_scalar("SELECT last_attempt_at FROM alicloud_accounts WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let now = apsp_protocol::now_timestamp();
    // A bill read refused as rate limited defers the whole pass; a refresh waits for it.
    let throttled = billing::state(&mut tx, id).await?.throttled;
    if last + 60 > now || (throttled && account.next_run_at > now) {
        return Err(ApiError::Busy);
    }
    sqlx::query("UPDATE alicloud_accounts SET next_run_at=0 WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let resources: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM alicloud_resources WHERE account_id=$1 AND NOT archived",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    billing::changed(
        &mut tx,
        id,
        billing::Change::Refreshed {
            resources: &resources,
        },
    )
    .await?;
    sqlx::query(
        "UPDATE alicloud_resources SET next_power_at=0 WHERE account_id=$1 AND NOT archived",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceWrite {
    account_id: Uuid,
    name: String,
    kind: String,
    region: String,
    cloud_id: String,
    auto_enabled: bool,
    cap_mbps: i64,
    revision: Option<i64>,
}
impl ResourceWrite {
    fn validate(&self) -> ApiResult<()> {
        model::label(&self.name)?;
        if !matches!(self.kind.as_str(), "ecs" | "eip")
            || !model::identifier(&self.region, "")
            || !model::identifier(
                &self.cloud_id,
                if self.kind == "ecs" { "i-" } else { "eip-" },
            )
            || !(1..=100).contains(&self.cap_mbps)
        {
            return Err(ApiError::BadRequest(
                "请填写有效的资源类型、地域、资源标识，以及 1–100 Mbps 的自动降速目标".into(),
            ));
        }
        Ok(())
    }
}
async fn create_resource(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ResourceWrite>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    auth::require_admin(&state, &headers).await?;
    input.validate()?;
    let mut tx = lock(&state.pool, input.account_id).await?;
    account_on(&mut tx, input.account_id).await?;
    crate::locks::ALICLOUD_RESOURCES.lock(&mut tx).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM alicloud_resources WHERE NOT archived")
            .fetch_one(&mut *tx)
            .await?;
    if count >= 32 {
        return Err(ApiError::Conflict("最多登记 32 个云资源".into()));
    }
    // An unfinished EIP rotation may still release this address; registering it
    // now would leave a record of a released EIP behind.
    if input.kind == "eip" && super::work::in_rotation(&mut tx, &input.cloud_id).await? {
        return Err(ApiError::Conflict(
            "此 EIP 正在更换公网 IP 的流程中，请在流程结束后再登记".into(),
        ));
    }
    let id = Uuid::new_v4();
    let result=sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id,auto_enabled,cap_mbps) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id).bind(input.account_id).bind(input.name.trim()).bind(input.kind).bind(input.region).bind(input.cloud_id).bind(input.auto_enabled).bind(input.cap_mbps).execute(&mut *tx).await;
    if result.as_ref().is_err_and(|e| {
        e.as_database_error()
            .is_some_and(|e| e.is_unique_violation())
    }) {
        return Err(ApiError::Conflict(
            "该地域和资源已登记，请勿用多个账号重复管理".into(),
        ));
    }
    result?;
    sqlx::query("UPDATE alicloud_accounts SET next_run_at=0 WHERE id=$1")
        .bind(input.account_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(json!({"id":id}))))
}
async fn update_resource(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ResourceWrite>,
) -> ApiResult<StatusCode> {
    auth::require_admin(&state, &headers).await?;
    input.validate()?;
    let initial = resource(&state.pool, id).await?;
    let mut tx = lock(&state.pool, initial.account_id).await?;
    let current: Resource =
        sqlx::query_as("SELECT * FROM alicloud_resources WHERE id=$1 AND NOT archived")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    if Some(current.revision) != input.revision
        || input.account_id != current.account_id
        || input.kind != current.kind
        || input.region != current.region
        || input.cloud_id != current.cloud_id
    {
        return Err(ApiError::Conflict(
            "配置已变化，或尝试修改固定的账号、类型、地域和资源标识；请刷新或另行登记".into(),
        ));
    }
    sqlx::query("UPDATE alicloud_resources SET name=$2,auto_enabled=$3,cap_mbps=$4,revision=revision+1 WHERE id=$1").bind(id).bind(input.name.trim()).bind(input.auto_enabled).bind(input.cap_mbps).execute(&mut *tx).await?;
    super::work::cancel_pending(&mut tx, &[id], true).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn remove_resource(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    input: Option<Json<Revision>>,
) -> ApiResult<StatusCode> {
    auth::require_admin(&state, &headers).await?;
    let resource = resource(&state.pool, id).await?;
    let mut tx = lock(&state.pool, resource.account_id).await?;
    let current: Resource =
        sqlx::query_as("SELECT * FROM alicloud_resources WHERE id=$1 AND NOT archived")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    if input.is_some_and(|Json(input)| input.revision != current.revision) {
        return Err(ApiError::Conflict("资源配置已变化，请刷新后重试".into()));
    }
    super::work::ensure_idle(&mut tx, id).await?;
    sqlx::query("UPDATE alicloud_resources SET archived=true,auto_enabled=false,revision=revision+1 WHERE id=$1").bind(id).execute(&mut *tx).await?;
    super::work::cancel_pending(&mut tx, &[id], false).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn refresh_resource(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<model::Snapshot>> {
    auth::require_admin(&state, &headers).await?;
    let initial = resource(&state.pool, id).await?;
    let mut tx = lock(&state.pool, initial.account_id).await?;
    let account = account_on(&mut tx, initial.account_id).await?;
    let resource: Resource =
        sqlx::query_as("SELECT * FROM alicloud_resources WHERE id=$1 AND NOT archived")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    if !account.enabled {
        return Err(ApiError::Conflict("请先启用云账号".into()));
    }
    let snapshot = Cloud::new(&state.pool)
        .map_err(failure)?
        .snapshot(&account, &resource)
        .await
        .map_err(failure)?;
    super::snapshot_on(&mut tx, id, &snapshot).await?;
    tx.commit().await?;
    Ok(Json(snapshot))
}
