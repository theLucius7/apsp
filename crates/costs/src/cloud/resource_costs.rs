//! What costs records about a cloud resource from its link form (ADR 0137 step 2), in
//! `cloud_resource_costs`: the monthly budget its instance bill is compared with, in the
//! budget's own currency, and, while the resource is not linked to a server, what was paid
//! for it, in which currency and until when (the purchase). A linked resource's price and
//! expiry are its server's asset settings, so no purchase is kept for it; linking a resource
//! drops the one it had. Costs answers resources' `ResourceCosts` interface, adds these
//! values to the operations page's links and raises the budget and expiry incidents.

use crate::error::{ApiError, ApiResult};
use apsp_panel_host::{evidence, money, ports::BoxFuture, text};
use apsp_protocol::now_timestamp;
use apsp_resources::{
    cloud_resources::{
        Observed,
        costs::{BudgetCurrency, Recorded, ResourceCosts},
    },
    server_assets::AssetSettings,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::HashMap;
use uuid::Uuid;

/// An amount as the form writes it: at most 18 whole and 6 fraction digits.
fn amount(value: Option<&str>) -> bool {
    value.is_none_or(|value| money::decimal(value, 6).is_some_and(|(whole, _)| whole.len() <= 18))
}

async fn record(
    tx: &mut Transaction<'_, Postgres>,
    resource: Uuid,
    recorded: Recorded<'_>,
) -> ApiResult<()> {
    let (reference, purchase_amount, currency, expires_at) =
        recorded
            .purchase
            .as_ref()
            .map_or(("", None, None, None), |purchase| {
                (
                    purchase.reference,
                    purchase.amount,
                    purchase.currency,
                    purchase.expires_at,
                )
            });
    let budget = recorded.monthly_budget;
    let budget_currency = match recorded.budget_currency {
        BudgetCurrency::Given(currency) => currency.map(str::to_owned),
        BudgetCurrency::Kept(otherwise) => sqlx::query_scalar::<_, Option<String>>(
            "SELECT budget_currency FROM cloud_resource_costs WHERE resource_id=$1 FOR UPDATE",
        )
        .bind(resource)
        .fetch_optional(&mut **tx)
        .await?
        .flatten()
        .or_else(|| otherwise.map(str::to_owned)),
    };
    // Only a budget has a budget currency.
    let budget_currency = budget.and(budget_currency);
    if reference.chars().count() > 512
        || !amount(purchase_amount)
        || !amount(budget)
        || [currency, budget_currency.as_deref()]
            .into_iter()
            .flatten()
            .any(|currency| !money::currency(currency))
        || (purchase_amount.is_some() && currency.is_none())
        || (budget.is_some() && budget_currency.is_none())
    {
        return Err(ApiError::BadRequest(
            "采购引用、金额、币种或备注无效".into(),
        ));
    }
    if !reference.is_empty() {
        text::label(reference, 512)?;
    }
    if reference.is_empty()
        && purchase_amount.is_none()
        && currency.is_none()
        && expires_at.is_none()
        && budget.is_none()
    {
        sqlx::query("DELETE FROM cloud_resource_costs WHERE resource_id=$1")
            .bind(resource)
            .execute(&mut **tx)
            .await?;
    } else {
        sqlx::query("INSERT INTO cloud_resource_costs(resource_id,purchase_reference,purchase_amount,currency,monthly_budget,budget_currency,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(resource_id) DO UPDATE SET purchase_reference=EXCLUDED.purchase_reference,purchase_amount=EXCLUDED.purchase_amount,currency=EXCLUDED.currency,monthly_budget=EXCLUDED.monthly_budget,budget_currency=EXCLUDED.budget_currency,expires_at=EXCLUDED.expires_at")
            .bind(resource).bind(reference).bind(purchase_amount).bind(currency).bind(budget).bind(&budget_currency).bind(expires_at).execute(&mut **tx).await?;
    }
    Ok(())
}

/// The keys costs adds to the operations page's link of each resource: the budget and its
/// currency, and for a resource without a server (`linked` false) its purchase. Resources
/// whose link costs has nothing for get the empty values.
pub async fn link_values(
    pool: &PgPool,
    resources: &[(Uuid, bool)],
) -> ApiResult<HashMap<Uuid, Value>> {
    let (ids, linked): (Vec<Uuid>, Vec<bool>) = resources.iter().copied().unzip();
    let rows: Vec<(Uuid, Value)> = sqlx::query_as("SELECT r.id,jsonb_build_object('monthly_budget',c.monthly_budget,'budget_currency',c.budget_currency)||CASE WHEN r.linked THEN '{}'::jsonb ELSE jsonb_build_object('purchase_reference',COALESCE(c.purchase_reference,''),'purchase_amount',c.purchase_amount,'currency',c.currency,'expires_at',c.expires_at) END FROM unnest($1::uuid[],$2::boolean[]) AS r(id,linked) LEFT JOIN cloud_resource_costs c ON c.resource_id=r.id")
        .bind(&ids).bind(&linked).fetch_all(pool).await?;
    Ok(rows.into_iter().collect())
}

/// A decimal amount scaled by 10^12, for comparing bills with budgets exactly.
fn decimal(value: &str) -> Option<i128> {
    let (negative, value) = value
        .strip_prefix('-')
        .map_or((false, value), |v| (true, v));
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || whole.len() > 18
        || fraction.len() > 12
        || !whole.bytes().all(|v| v.is_ascii_digit())
        || !fraction.bytes().all(|v| v.is_ascii_digit())
    {
        return None;
    }
    let whole = whole.parse::<i128>().ok()?.checked_mul(1_000_000_000_000)?;
    let tail = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<i128>()
            .ok()?
            .checked_mul(10_i128.pow(12 - fraction.len() as u32))?
    };
    let total = whole.checked_add(tail)?;
    Some(if negative { -total } else { total })
}

/// Opens the incidents of the resources just observed: a recent instance bill that reached
/// the monthly budget, or holds amounts in another currency than the budget's, and a purchase
/// expiry within seven days either side of now: the one recorded for a resource without a
/// server, its server's (after automatic renewal) for a linked one.
pub async fn check(pool: &PgPool, observed: &[Observed]) -> ApiResult<()> {
    if observed.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    if !crate::locks::CLOUD_RESOURCE_CHECKS
        .try_lock(&mut tx)
        .await?
    {
        return Ok(());
    }
    let (ids, servers): (Vec<Uuid>, Vec<Option<i64>>) = observed
        .iter()
        .map(|resource| (resource.id, resource.server_id))
        .unzip();
    let rows = sqlx::query("SELECT r.id,c.budget_currency,c.monthly_budget,c.expires_at,b.instance_bill,b.bill_error,s.asset_settings FROM unnest($1::uuid[],$2::bigint[]) AS r(id,server_id) LEFT JOIN cloud_resource_costs c ON c.resource_id=r.id LEFT JOIN cloud_resource_billing b ON b.resource_id=r.id LEFT JOIN servers s ON s.id=r.server_id")
        .bind(&ids)
        .bind(&servers)
        .fetch_all(&mut *tx)
        .await?;
    let mut values = HashMap::with_capacity(rows.len());
    for row in rows {
        values.insert(row.try_get::<Uuid, _>("id")?, row);
    }
    for resource in observed {
        let Some(row) = values.get(&resource.id) else {
            continue;
        };
        let id = resource.id;
        let bill: Value = row
            .try_get::<Option<Value>, _>("instance_bill")?
            .unwrap_or(Value::Null);
        let bill_error: Option<String> = row.try_get("bill_error")?;
        let now = now_timestamp();
        let currency: Option<String> = row.try_get("budget_currency")?;
        let budget: Option<String> = row.try_get("monthly_budget")?;
        let queried = bill["queried_at"].as_i64();
        if bill_error.is_none()
            && queried.is_some_and(|v| v <= now && now - v <= 21600)
            && let (Some(currency), Some(budget), Some(entries)) =
                (currency, budget, bill["rows"].as_array())
        {
            // Bills in another currency are not converted; say so instead of
            // silently treating the budget as never reached.
            if entries
                .iter()
                .any(|v| v["currency"].as_str() != Some(currency.as_str()))
            {
                evidence::open_incident_tx(&mut tx, evidence::IncidentReport { key: &format!("cloud:budget-currency:{id}:{}",bill["month"].as_str().unwrap_or("unknown")), server: resource.server_id, title: &format!("云资源 {} 的账单含预算币种以外的金额，未计入预算比较",resource.name), severity: "info", evidence: json!({"provider":"alicloud","resource_id":id,"queried_at":queried,"budget_currency":currency,"bill":bill,"converted":false}), observed_at: queried.unwrap() }).await?;
            }
            let actual = entries
                .iter()
                .filter(|v| v["currency"].as_str() == Some(currency.as_str()))
                .try_fold(0_i128, |sum, entry| {
                    sum.checked_add(decimal(entry["amount"].as_str()?)?)
                });
            if let (Some(actual), Some(budget_number)) = (actual, decimal(&budget))
                && actual >= budget_number
                && entries
                    .iter()
                    .any(|v| v["currency"].as_str() == Some(currency.as_str()))
            {
                evidence::open_incident_tx(&mut tx, evidence::IncidentReport { key: &format!("cloud:budget:{id}:{}",bill["month"].as_str().unwrap_or("unknown")), server: resource.server_id, title: &format!("云资源 {} 的账单缓存达到月预算",resource.name), severity: "warning", evidence: json!({"provider":"alicloud","resource_id":id,"queried_at":queried,"currency":currency,"budget":budget,"bill":bill,"real_time_limit":false}), observed_at: queried.unwrap() }).await?;
            }
        }
        let expires = match resource.server_id {
            Some(_) => row
                .try_get::<Option<Value>, _>("asset_settings")?
                .map(serde_json::from_value::<AssetSettings>)
                .transpose()
                .map_err(anyhow::Error::from)?
                .and_then(|mut settings| {
                    settings.renew(now);
                    settings.expires_at
                }),
            None => row.try_get("expires_at")?,
        };
        if let Some(expires) = expires
            && expires - now <= 7 * 86400
            && now - expires <= 7 * 86400
        {
            evidence::open_incident_tx(&mut tx, evidence::IncidentReport { key: &format!("cloud:expiry:{id}:{expires}"), server: resource.server_id, title: &format!("云资源 {} 的采购到期记录需要处理",resource.name), severity: "warning", evidence: json!({"resource_id":id,"expires_at":expires,"source":"人工采购台账，非实时云计费状态"}), observed_at: now }).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Costs answers what resources' link form records.
struct Costs;

impl ResourceCosts for Costs {
    fn record<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        resource: Uuid,
        recorded: Recorded<'a>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(record(tx, resource, recorded))
    }
}

static COSTS: Costs = Costs;

/// Connects the answer to resources' link form.
pub fn install() {
    apsp_resources::cloud_resources::costs::install(&COSTS);
}

#[cfg(test)]
mod tests {
    #[test]
    fn budget_comparison_keeps_decimal_precision_and_refunds() {
        assert_eq!(
            super::decimal("100.000000000001"),
            Some(100_000_000_000_001)
        );
        assert_eq!(super::decimal("-0.01"), Some(-10_000_000_000));
        assert_eq!(super::decimal("1e3"), None);
        assert_eq!(super::decimal("1.1234567890123"), None);
    }

    #[test]
    fn form_amounts_keep_their_limits() {
        assert!(super::amount(None));
        assert!(super::amount(Some("123456789012345678.123456")));
        assert!(!super::amount(Some("1234567890123456789")));
        assert!(!super::amount(Some("1.1234567")));
        assert!(!super::amount(Some("-1")));
    }
}
