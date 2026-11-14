//! What operations keeps about a cloud account and resource follows their configuration,
//! which resources edits (ADR 0137 step 7). Operations keeps the automatic bandwidth
//! reduction, the power policy, the holds and the CDT guard's readings in its own tables;
//! the account and resource forms save the bandwidth settings with the account or resource,
//! and an edit, an archive or a manual refresh tells operations what to switch off or check
//! again. Operations answers and the application connects it. Without a connection the
//! caller's transaction fails, so no account or resource changes without its automation
//! following.

use apsp_panel_host::ports::{BoxFuture, Port};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

/// An account's automatic bandwidth reduction: whether it is on, and the account's billed
/// CDT usage this month, in GB, from which it applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountSettings {
    pub auto_enabled: bool,
    pub limit_gb: i64,
}

/// A resource's automatic bandwidth reduction: whether it is on, and the bandwidth in Mbps
/// it lowers the resource to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceSettings {
    pub auto_enabled: bool,
    pub cap_mbps: i64,
}

/// A change to an account that operations follows, inside the caller's transaction.
pub enum AccountChange<'a> {
    /// A new account with its settings.
    Created(AccountSettings),
    /// An edited account with its new settings. The power and CDT guard checks of
    /// `resources` (all the account's resources, archived ones included) are due at once.
    Edited {
        settings: AccountSettings,
        resources: &'a [Uuid],
    },
    /// An archived account: its automatic bandwidth reduction is off.
    Archived,
    /// A manual refresh: the power checks of `resources` are due at once.
    Refreshed { resources: &'a [Uuid] },
}

/// A change to a resource that operations follows, inside the caller's transaction.
pub enum ResourceChange {
    /// A new resource with its settings.
    Created(ResourceSettings),
    /// An edited resource with its new settings.
    Edited(ResourceSettings),
    /// An archived resource: its automatic bandwidth reduction is off.
    Archived,
}

pub trait CloudAutomation: Send + Sync {
    /// Whether any of these resources has an enabled power policy whose bill threshold
    /// notifies or stops; the account's bills must stay readable for them.
    fn uses_bills<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        resources: &'a [Uuid],
    ) -> BoxFuture<'a, ApiResult<bool>>;

    /// Records a change to the account.
    fn account_changed<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        account: Uuid,
        change: AccountChange<'a>,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Records a change to the resource.
    fn resource_changed<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        resource: Uuid,
        change: ResourceChange,
    ) -> BoxFuture<'a, ApiResult<()>>;
}

static AUTOMATION: Port<dyn CloudAutomation> = Port::new();

pub fn install(automation: &'static dyn CloudAutomation) {
    AUTOMATION.install(automation);
}

fn required() -> ApiResult<&'static dyn CloudAutomation> {
    AUTOMATION
        .get()
        .ok_or_else(|| ApiError::Conflict("云资源的自动控制状态未登记，不能修改账号或资源".into()))
}

pub async fn uses_bills(tx: &mut Transaction<'_, Postgres>, resources: &[Uuid]) -> ApiResult<bool> {
    required()?.uses_bills(tx, resources).await
}

pub async fn account_changed(
    tx: &mut Transaction<'_, Postgres>,
    account: Uuid,
    change: AccountChange<'_>,
) -> ApiResult<()> {
    required()?.account_changed(tx, account, change).await
}

pub async fn resource_changed(
    tx: &mut Transaction<'_, Postgres>,
    resource: Uuid,
    change: ResourceChange,
) -> ApiResult<()> {
    required()?.resource_changed(tx, resource, change).await
}
