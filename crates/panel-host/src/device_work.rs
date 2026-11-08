//! Work for a device, part of the platform's persistent task infrastructure (ADR 0128
//! section 3.3).
//!
//! Before a domain sends work to a device it asks here whether the device accepts new
//! work, and fixed device operations are submitted here. The owner that executes device
//! work answers (resource lifecycle, maintenance windows, exclusive jobs and outstanding
//! operations decide); the application connects it at startup. Without a connection no
//! work is accepted.

use apsp_protocol::fleet::Operation;
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
