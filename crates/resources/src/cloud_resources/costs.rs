//! What costs records about a cloud resource from its link form (ADR 0137 step 2): the
//! monthly budget its bill is compared with and, while the resource is not linked to a
//! server, what was paid for it and until when. A linked resource's price and expiry are its
//! server's asset settings, so costs keeps no purchase for it. Costs answers and the
//! application connects it; without a connection the caller's transaction fails, so no link
//! changes without its costs following.

use apsp_panel_host::ports::{BoxFuture, Port};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

/// The purchase of a resource that is not linked to a server, as the form gives it.
pub struct Purchase<'a> {
    pub reference: &'a str,
    pub amount: Option<&'a str>,
    /// The currency of `amount`.
    pub currency: Option<&'a str>,
    pub expires_at: Option<i64>,
}

/// The currency of the monthly budget.
pub enum BudgetCurrency<'a> {
    /// As the form gives it.
    Given(Option<&'a str>),
    /// The form gave the budget no currency of its own and its one currency is the linked
    /// server's purchase currency (a console from before budgets had their own): the budget
    /// keeps the currency it has, and takes this one only when it has none.
    Kept(Option<&'a str>),
}

/// What the link form records for costs.
pub struct Recorded<'a> {
    /// `None` for a resource linked to a server: whatever costs kept as its purchase goes.
    pub purchase: Option<Purchase<'a>>,
    pub monthly_budget: Option<&'a str>,
    pub budget_currency: BudgetCurrency<'a>,
}

pub trait ResourceCosts: Send + Sync {
    /// Checks and records the form's values in the caller's transaction.
    fn record<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        resource: Uuid,
        recorded: Recorded<'a>,
    ) -> BoxFuture<'a, ApiResult<()>>;
}

static COSTS: Port<dyn ResourceCosts> = Port::new();

pub fn install(costs: &'static dyn ResourceCosts) {
    COSTS.install(costs);
}

pub async fn record(
    tx: &mut Transaction<'_, Postgres>,
    resource: Uuid,
    recorded: Recorded<'_>,
) -> ApiResult<()> {
    COSTS
        .get()
        .ok_or_else(|| ApiError::Conflict("云资源的费用记录未登记，不能修改资源关联".into()))?
        .record(tx, resource, recorded)
        .await
}
