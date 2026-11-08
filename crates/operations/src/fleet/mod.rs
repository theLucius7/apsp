//! Fixed device operations, their reconciliation and terminals, and whether a server
//! accepts new work: the device's lifecycle and maintenance, exclusive jobs and
//! operations whose result is still unknown.

mod operations;
pub(crate) mod reconciliation;
mod terminal;
pub(crate) use operations::enqueue_automation_tx;
pub use operations::{enqueue_for_actor, enqueue_for_actor_tx, enqueue_readonly_for_actor_tx};

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use axum::{
    Router,
    routing::{get, post},
};
use sqlx::{PgPool, Postgres, Row, Transaction};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/fleet/operations/{id}", get(operations::get))
        .route(
            "/api/fleet/operations/{id}/inspection",
            post(reconciliation::inspect),
        )
        .route(
            "/api/fleet/operations/{id}/reconcile",
            post(reconciliation::complete),
        )
        .route(
            "/api/servers/{id}/fleet/operations",
            get(operations::list).post(operations::create),
        )
        .route(
            "/api/servers/{id}/fleet/config-history",
            get(operations::history),
        )
        .route(
            "/api/servers/{id}/fleet/terminals",
            get(terminal::list).post(terminal::create),
        )
        .route(
            "/api/fleet/terminals/{id}",
            get(terminal::get).delete(terminal::close),
        )
        .route("/api/fleet/terminals/{id}/input", post(terminal::input))
        .route("/api/agent/v1/fleet/work", get(operations::work))
        .route("/api/agent/v1/fleet/results", post(operations::complete))
        .route("/api/agent/v1/fleet/terminal-events", post(terminal::event))
}

pub async fn ensure_accepts_tasks(pool: &PgPool, server: i64) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    ensure_accepts_tasks_tx(&mut tx, server).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn ensure_accepts_tasks_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<()> {
    let exists =
        sqlx::query("SELECT id FROM servers WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
            .bind(server)
            .fetch_optional(&mut **tx)
            .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fleet_profiles WHERE server_id=$1 AND (lifecycle IN ('draining','retired') OR (lifecycle='maintenance' AND COALESCE(maintenance_from,0)<=$2 AND (maintenance_until IS NULL OR maintenance_until>$2)))) OR EXISTS(SELECT 1 FROM server_retirements WHERE server_id=$1) OR EXISTS(SELECT 1 FROM operations_server_locks WHERE server_id=$1) OR EXISTS(SELECT 1 FROM fleet_operations WHERE server_id=$1 AND ((status IN ('dispatched','unknown') AND reconciled_at IS NULL) OR (status='queued' AND expires_at>$2))) OR EXISTS(SELECT 1 FROM operations_maintenance WHERE $1=ANY(targets) AND block_new_tasks AND starts_at<=$2 AND ends_at>$2)")
        .bind(server).bind(apsp_protocol::now_timestamp()).fetch_one(&mut **tx).await?;
    if blocked {
        return Err(ApiError::Conflict(
            "服务器处于维护、停止接收任务或互斥操作中，不能接收新任务".into(),
        ));
    }
    Ok(())
}

pub(super) async fn ensure_terminal_input_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<()> {
    sqlx::query("SELECT id FROM servers WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
        .bind(server)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fleet_profiles WHERE server_id=$1 AND (lifecycle IN ('draining','retired') OR (lifecycle='maintenance' AND COALESCE(maintenance_from,0)<=$2 AND (maintenance_until IS NULL OR maintenance_until>$2)))) OR EXISTS(SELECT 1 FROM server_retirements WHERE server_id=$1) OR EXISTS(SELECT 1 FROM operations_maintenance WHERE $1=ANY(targets) AND block_new_tasks AND starts_at<=$2 AND ends_at>$2)").bind(server).bind(apsp_protocol::now_timestamp()).fetch_one(&mut **tx).await?;
    if blocked {
        return Err(ApiError::Conflict(
            "终端所在设备已进入维护或退役；仍可请求强制关闭".into(),
        ));
    }
    Ok(())
}

pub(super) async fn ensure_accepts_fleet_tasks_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<()> {
    ensure_terminal_input_tx(tx, server).await?;
    let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM operations_server_locks WHERE server_id=$1) OR EXISTS(SELECT 1 FROM fleet_operations WHERE server_id=$1 AND (reconciled_at IS NULL AND (status='unknown' OR (status='dispatched' AND expires_at<=$2))))").bind(server).bind(apsp_protocol::now_timestamp()).fetch_one(&mut **tx).await?;
    if blocked {
        return Err(ApiError::Conflict(
            "设备存在互斥自动化作业或未知结果；核对后再创建日常操作".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn policy(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
) -> ApiResult<apsp_protocol::fleet::AccessPolicy> {
    let value = sqlx::query("SELECT policy FROM fleet_profiles WHERE server_id=$1")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    value
        .map(|row| serde_json::from_value(row.get("policy")))
        .transpose()
        .map_err(|error| ApiError::Internal(error.into()))
        .map(|value| value.unwrap_or_default())
}
