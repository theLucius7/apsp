//! What costs keeps about a cloud account follows the account's configuration, which
//! resources edits (ADR 0137 step 6). Costs keeps the bills, balance and CDT readings in its
//! own tables; the account form saves the billing settings with the account, and an edit,
//! an archive or a manual refresh tells costs what to discard or read again. Costs answers
//! and the application connects it. Without a connection the caller's transaction fails, so
//! no account changes without its billing state following.

use apsp_panel_host::ports::{BoxFuture, Port};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

/// Which of the account's bills costs reads: the monthly CDT bill (and instance bills), and
/// the account balance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub billing_enabled: bool,
    pub balance_enabled: bool,
}

/// The account's billing settings, and whether its last bill read was refused as rate
/// limited (a manual refresh then waits for the deferred pass).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub settings: Settings,
    pub throttled: bool,
}

/// A change to an account that costs follows, inside the caller's transaction.
pub enum Change<'a> {
    /// A new account with its settings.
    Created(Settings),
    /// An edited account with its new settings. Every bill, balance and CDT reading taken
    /// under the old configuration is discarded, those of `resources` (all the account's
    /// resources, archived ones included) too, and the CDT history as well when the
    /// account's site or credentials changed (`rekeyed`).
    Edited {
        settings: Settings,
        rekeyed: bool,
        resources: &'a [Uuid],
    },
    /// An archived account: its bill, CDT reading and balance are discarded.
    Archived,
    /// A manual refresh: the balance and the instance bills of `resources` are read again
    /// at once, unless their last read failed.
    Refreshed { resources: &'a [Uuid] },
}

pub trait CloudBilling: Send + Sync {
    /// The account's billing state; defaults when costs has none.
    fn state<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        account: Uuid,
    ) -> BoxFuture<'a, ApiResult<State>>;

    /// Records a change to the account.
    fn changed<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        account: Uuid,
        change: Change<'a>,
    ) -> BoxFuture<'a, ApiResult<()>>;
}

static BILLING: Port<dyn CloudBilling> = Port::new();

pub fn install(billing: &'static dyn CloudBilling) {
    BILLING.install(billing);
}

fn required() -> ApiResult<&'static dyn CloudBilling> {
    BILLING
        .get()
        .ok_or_else(|| ApiError::Conflict("云账号的账单状态未登记，不能修改账号".into()))
}

pub async fn state(tx: &mut Transaction<'_, Postgres>, account: Uuid) -> ApiResult<State> {
    required()?.state(tx, account).await
}

pub async fn changed(
    tx: &mut Transaction<'_, Postgres>,
    account: Uuid,
    change: Change<'_>,
) -> ApiResult<()> {
    required()?.changed(tx, account, change).await
}
