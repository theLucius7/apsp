//! What costs keeps about each cloud account and resource (ADR 0137 step 6): which of the
//! account's bills it reads, the last CDT bill, CDT traffic and balance it read and why a
//! read failed, and each resource's monthly instance bill. Resources keeps the accounts and
//! resources themselves and edits the account form; costs answers its `CloudBilling`
//! interface so the form's billing settings and edits reach these rows. No row means the
//! defaults: nothing read, no bill or balance enabled.

use crate::cloud::billing;
use crate::error::ApiResult;
use apsp_panel_host::ports::BoxFuture;
use apsp_resources::cloud::billing::{Change, CloudBilling, Settings, State};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool, Postgres, Transaction, types::Json};
use std::collections::HashMap;
use uuid::Uuid;

// The stored shapes of the bill, traffic and balance columns.

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BillRow {
    pub instance_id: String,
    pub region: String,
    pub product_type: String,
    pub billing_item: String,
    pub usage: String,
    pub unit: String,
    pub amount: String,
    pub currency: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bill {
    pub month: String,
    pub queried_at: i64,
    pub rows: Vec<BillRow>,
    // None means the complete response cannot be used for automatic control.
    pub usage_micro_gb: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Traffic {
    pub queried_at: i64,
    pub mainland_bytes: String,
    pub overseas_bytes: String,
    pub regions: Vec<TrafficRegion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrafficRegion {
    pub region: String,
    pub bytes: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Balance {
    pub available: String,
    pub currency: String,
    pub queried_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CostRow {
    pub item: String,
    pub amount: String,
    pub currency: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstanceBill {
    pub month: String,
    pub queried_at: i64,
    pub rows: Vec<CostRow>,
}

/// An account's billing state. It serializes under the keys the cloud page reads beside the
/// account's own (the bill's error is the account's `error_code` there).
#[derive(Clone, Debug, Default, Serialize, FromRow)]
pub struct AccountBilling {
    pub billing_enabled: bool,
    pub balance_enabled: bool,
    pub bill: Option<Json<Bill>>,
    #[serde(rename = "error_code")]
    pub bill_error: Option<String>,
    pub traffic: Option<Json<Traffic>>,
    pub traffic_error: Option<String>,
    pub balance: Option<Json<Balance>>,
    pub balance_error: Option<String>,
    pub balance_next_at: i64,
}

impl AccountBilling {
    /// The account's billed CDT usage this month in micro-GB, when it may drive automatic
    /// control: bills are read, the last read succeeded with this month's complete bill,
    /// read within the last fifteen minutes.
    pub fn usage(&self, now: i64) -> Option<u64> {
        if !self.billing_enabled || self.bill_error.is_some() {
            return None;
        }
        let bill = self.bill.as_ref()?;
        if bill.month != billing::month(now) || bill.queried_at > now || bill.queried_at + 900 < now
        {
            return None;
        }
        bill.usage_micro_gb
    }
}

/// A resource's monthly instance bill, under the keys the cloud page reads beside the
/// resource's own.
#[derive(Clone, Debug, Default, Serialize, FromRow)]
pub struct ResourceBilling {
    pub instance_bill: Option<Json<InstanceBill>>,
    pub bill_error: Option<String>,
    pub bill_next_at: i64,
}

pub async fn account_on(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<AccountBilling> {
    Ok(
        sqlx::query_as("SELECT * FROM cloud_account_billing WHERE account_id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .unwrap_or_default(),
    )
}

pub async fn account(pool: &PgPool, id: Uuid) -> ApiResult<AccountBilling> {
    Ok(
        sqlx::query_as("SELECT * FROM cloud_account_billing WHERE account_id=$1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .unwrap_or_default(),
    )
}

pub async fn resource_on(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> ApiResult<ResourceBilling> {
    Ok(
        sqlx::query_as("SELECT * FROM cloud_resource_billing WHERE resource_id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .unwrap_or_default(),
    )
}

#[derive(FromRow)]
struct AccountRow {
    account_id: Uuid,
    #[sqlx(flatten)]
    billing: AccountBilling,
}

#[derive(FromRow)]
struct ResourceRow {
    resource_id: Uuid,
    #[sqlx(flatten)]
    billing: ResourceBilling,
}

/// The billing state of these accounts and resources, for the cloud page.
pub async fn listed(
    pool: &PgPool,
    accounts: &[Uuid],
    resources: &[Uuid],
) -> ApiResult<(
    HashMap<Uuid, AccountBilling>,
    HashMap<Uuid, ResourceBilling>,
)> {
    let account_rows: Vec<AccountRow> =
        sqlx::query_as("SELECT * FROM cloud_account_billing WHERE account_id=ANY($1)")
            .bind(accounts)
            .fetch_all(pool)
            .await?;
    let resource_rows: Vec<ResourceRow> =
        sqlx::query_as("SELECT * FROM cloud_resource_billing WHERE resource_id=ANY($1)")
            .bind(resources)
            .fetch_all(pool)
            .await?;
    Ok((
        account_rows
            .into_iter()
            .map(|row| (row.account_id, row.billing))
            .collect(),
        resource_rows
            .into_iter()
            .map(|row| (row.resource_id, row.billing))
            .collect(),
    ))
}

/// The bill columns of the operations page's cloud resources, by resource: the instance
/// bill as stored, its error, the amounts per currency and whether it is older than six
/// hours, and for `global` readers the account balance. `resources` pairs each resource
/// with its account.
pub async fn resource_costs(
    pool: &PgPool,
    now: i64,
    global: bool,
    resources: &[(Uuid, Uuid)],
) -> ApiResult<HashMap<Uuid, Value>> {
    let (ids, accounts): (Vec<Uuid>, Vec<Uuid>) = resources.iter().copied().unzip();
    let rows: Vec<(Uuid, Value)> = sqlx::query_as("SELECT r.id,jsonb_build_object('instance_bill',b.instance_bill,'bill_error',b.bill_error,'balance',CASE WHEN $2 THEN a.balance ELSE NULL END,'balance_error',CASE WHEN $2 THEN a.balance_error ELSE NULL END,'balance_scope',CASE WHEN $2 THEN 'cloud_account_global' ELSE 'resource_only' END,'actual_costs_by_currency',COALESCE((SELECT jsonb_object_agg(currency,total) FROM (SELECT e->>'currency' AS currency,sum((e->>'amount')::numeric)::text AS total FROM jsonb_array_elements(COALESCE(b.instance_bill->'rows','[]'::jsonb)) e GROUP BY e->>'currency') costs),'{}'::jsonb),'bill_stale',b.instance_bill IS NULL OR (b.instance_bill->>'queried_at')::bigint<$1-21600) FROM unnest($3::uuid[],$4::uuid[]) AS r(id,account_id) LEFT JOIN cloud_resource_billing b ON b.resource_id=r.id LEFT JOIN cloud_account_billing a ON a.account_id=r.account_id")
        .bind(now).bind(global).bind(&ids).bind(&accounts).fetch_all(pool).await?;
    Ok(rows.into_iter().collect())
}

/// Each resource's monthly bill as stored and why the last read failed, for the
/// resources' observation history.
pub async fn instance_bills(
    pool: &PgPool,
) -> ApiResult<HashMap<Uuid, apsp_resources::cloud_resources::Bill>> {
    let rows: Vec<(Uuid, Option<Value>, Option<String>)> =
        sqlx::query_as("SELECT resource_id,instance_bill,bill_error FROM cloud_resource_billing")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, bill, error)| (id, apsp_resources::cloud_resources::Bill { bill, error }))
        .collect())
}

async fn changed(
    tx: &mut Transaction<'_, Postgres>,
    account: Uuid,
    change: Change<'_>,
) -> ApiResult<()> {
    match change {
        Change::Created(settings) => {
            sqlx::query("INSERT INTO cloud_account_billing(account_id,billing_enabled,balance_enabled) VALUES($1,$2,$3) ON CONFLICT(account_id) DO UPDATE SET billing_enabled=EXCLUDED.billing_enabled,balance_enabled=EXCLUDED.balance_enabled")
                .bind(account).bind(settings.billing_enabled).bind(settings.balance_enabled).execute(&mut **tx).await?;
        }
        Change::Edited {
            settings,
            rekeyed,
            resources,
        } => {
            // A new site or credential is another CDT pool: its history starts again.
            if rekeyed {
                sqlx::query("DELETE FROM alicloud_traffic_history WHERE account_id=$1")
                    .bind(account)
                    .execute(&mut **tx)
                    .await?;
            }
            sqlx::query("INSERT INTO cloud_account_billing(account_id,billing_enabled,balance_enabled) VALUES($1,$2,$3) ON CONFLICT(account_id) DO UPDATE SET billing_enabled=EXCLUDED.billing_enabled,balance_enabled=EXCLUDED.balance_enabled,bill=NULL,bill_error=NULL,traffic=NULL,traffic_error=NULL,balance=NULL,balance_error=NULL,balance_next_at=0")
                .bind(account).bind(settings.billing_enabled).bind(settings.balance_enabled).execute(&mut **tx).await?;
            sqlx::query("UPDATE cloud_resource_billing SET instance_bill=NULL,bill_error=NULL,bill_next_at=0 WHERE resource_id=ANY($1)")
                .bind(resources).execute(&mut **tx).await?;
        }
        Change::Archived => {
            sqlx::query("UPDATE cloud_account_billing SET bill=NULL,traffic=NULL,balance=NULL WHERE account_id=$1")
                .bind(account).execute(&mut **tx).await?;
        }
        Change::Refreshed { resources } => {
            sqlx::query("UPDATE cloud_account_billing SET balance_next_at=0 WHERE account_id=$1 AND balance_error IS NULL")
                .bind(account).execute(&mut **tx).await?;
            sqlx::query("UPDATE cloud_resource_billing SET bill_next_at=0 WHERE resource_id=ANY($1) AND bill_error IS NULL")
                .bind(resources).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

/// Costs answers what resources' account form asks about the account's billing state.
struct Billing;

impl CloudBilling for Billing {
    fn state<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        account: Uuid,
    ) -> BoxFuture<'a, ApiResult<State>> {
        Box::pin(async move {
            let billing = account_on(tx, account).await?;
            Ok(State {
                settings: Settings {
                    billing_enabled: billing.billing_enabled,
                    balance_enabled: billing.balance_enabled,
                },
                throttled: billing.bill_error.as_deref() == Some("rate_limited"),
            })
        })
    }

    fn changed<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        account: Uuid,
        change: Change<'a>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(changed(tx, account, change))
    }
}

static BILLING: Billing = Billing;

/// Connects the answer to the resources' account form.
pub fn install() {
    apsp_resources::cloud::billing::install(&BILLING);
}
