//! Cloud work in progress, which only operations knows (ADR 0128 section 5.5). Resources
//! asks before it registers, removes or re-keys what the work runs on, and cancels the
//! work a changed configuration invalidates; security groups ask before they change a
//! resource. Operations answers and the application connects it. Without a connection the
//! caller's transaction fails, so nothing is removed or changed while its state is unknown.

use apsp_panel_host::ports::{BoxFuture, Port};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

pub trait CloudWork: Send + Sync {
    /// Fails while the resource has a bandwidth operation, power job, security group
    /// change or address rotation that is pending or uncertain.
    fn ensure_idle<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        resource: Uuid,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Whether an address rotation in progress involves this allocation.
    fn in_rotation<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        allocation: &'a str,
    ) -> BoxFuture<'a, ApiResult<bool>>;

    /// Whether any of these resources has a bandwidth operation, power job or security
    /// group change that was sent and whose result is still unknown.
    fn unresolved<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        resources: &'a [Uuid],
    ) -> BoxFuture<'a, ApiResult<bool>>;

    /// Cancels the resources' previews, and their queued bandwidth operations and power
    /// jobs when `queued` is set.
    fn cancel_pending<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        resources: &'a [Uuid],
        queued: bool,
    ) -> BoxFuture<'a, ApiResult<()>>;
}

static WORK: Port<dyn CloudWork> = Port::new();

pub fn install(work: &'static dyn CloudWork) {
    WORK.install(work);
}

fn required() -> ApiResult<&'static dyn CloudWork> {
    WORK.get().ok_or_else(|| {
        ApiError::Conflict("云资源的进行中操作查询未登记，不能修改或删除资源".into())
    })
}

pub(super) async fn ensure_idle(
    tx: &mut Transaction<'_, Postgres>,
    resource: Uuid,
) -> ApiResult<()> {
    required()?.ensure_idle(tx, resource).await
}

pub(super) async fn in_rotation(
    tx: &mut Transaction<'_, Postgres>,
    allocation: &str,
) -> ApiResult<bool> {
    required()?.in_rotation(tx, allocation).await
}

pub(super) async fn unresolved(
    tx: &mut Transaction<'_, Postgres>,
    resources: &[Uuid],
) -> ApiResult<bool> {
    required()?.unresolved(tx, resources).await
}

pub(super) async fn cancel_pending(
    tx: &mut Transaction<'_, Postgres>,
    resources: &[Uuid],
    queued: bool,
) -> ApiResult<()> {
    required()?.cancel_pending(tx, resources, queued).await
}
