//! Work for a device, part of the platform's persistent task infrastructure (ADR 0128
//! section 3.3).
//!
//! Before a domain sends work to a device it asks here whether the device accepts new
//! work, and fixed device operations are submitted here. The owner that executes device
//! work answers (resource lifecycle, maintenance windows, exclusive jobs and outstanding
//! operations decide); the application connects it at startup. Without a connection no
//! work is accepted.

use apsp_protocol::{RuntimeOperationRequest, fleet::Operation};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    error::{ApiError, ApiResult},
    ports::{BoxFuture, Port},
};

pub trait DeviceWork: Send + Sync {
    /// Fails unless the server exists and accepts new work now.
    fn ensure_accepts_tasks_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Queues a fixed operation that an administrator requested.
    fn submit_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        actor: i64,
        operation: Operation,
    ) -> BoxFuture<'a, ApiResult<Uuid>>;

    /// Queues a read-only observation (ports or runtime permissions) for a preflight.
    fn submit_observation_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        actor: i64,
        operation: Operation,
    ) -> BoxFuture<'a, ApiResult<Uuid>>;

    /// Whether the device has a runtime operation in flight, or one requested in the last
    /// few seconds; a new request waits for it instead of piling up.
    fn runtime_busy_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        now: i64,
    ) -> BoxFuture<'a, ApiResult<bool>>;

    /// Queues a runtime operation for the module the request names.
    fn submit_runtime_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        request: &'a RuntimeOperationRequest,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// The latest runtime operation of a module whose session filter names a principal
    /// matching `pattern`, as `{id, requested_at, expires_at, operation, result}`.
    fn latest_runtime_for_principal_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        module: &'a str,
        pattern: &'a str,
    ) -> BoxFuture<'a, ApiResult<Vec<Value>>>;
}

static WORK: Port<dyn DeviceWork> = Port::new();

pub fn install(work: &'static dyn DeviceWork) {
    WORK.install(work);
}

fn required() -> ApiResult<&'static dyn DeviceWork> {
    WORK.get().ok_or_else(|| {
        ApiError::Conflict("设备任务的执行方未登记，不能确认设备能否接收任务".into())
    })
}

pub async fn ensure_accepts_tasks_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<()> {
    required()?.ensure_accepts_tasks_tx(tx, server).await
}

/// Queues an operation in its own transaction.
pub async fn submit_for_actor(
    state: &AppState,
    server: i64,
    actor: i64,
    operation: Operation,
) -> ApiResult<Uuid> {
    let work = required()?;
    let mut tx = state.pool.begin().await?;
    let id = work.submit_tx(&mut tx, server, actor, operation).await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn submit_observation_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    actor: i64,
    operation: Operation,
) -> ApiResult<Uuid> {
    required()?
        .submit_observation_tx(tx, server, actor, operation)
        .await
}

pub async fn runtime_busy_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    now: i64,
) -> ApiResult<bool> {
    required()?.runtime_busy_tx(tx, server, now).await
}

pub async fn submit_runtime_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    request: &RuntimeOperationRequest,
) -> ApiResult<()> {
    required()?.submit_runtime_tx(tx, server, request).await
}

pub async fn latest_runtime_for_principal_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    module: &str,
    pattern: &str,
) -> ApiResult<Vec<Value>> {
    required()?
        .latest_runtime_for_principal_tx(tx, server, module, pattern)
        .await
}
