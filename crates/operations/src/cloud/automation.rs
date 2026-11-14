//! What operations keeps about each cloud account and resource (ADR 0137 step 7): an
//! account's automatic bandwidth reduction (whether it is on and its monthly threshold), a
//! resource's (whether it is on and its target), and a resource's power automation: its
//! policy, the power state last read and why a read failed, when it is checked next, the
//! manual and threshold holds and the CDT guard's reading. Resources keeps the accounts and
//! resources themselves and edits their forms; operations answers its `CloudAutomation`
//! interface so the forms' settings and edits reach these rows. No row means the defaults:
//! nothing automatic, no hold, nothing read, checked at once.

use super::power::model::{DEFAULT_POLICY, GuardState, Policy, State};
use crate::error::{ApiError, ApiResult};
use apsp_panel_host::ports::BoxFuture;
use apsp_resources::cloud::{
    automation::{AccountChange, AccountSettings, CloudAutomation, ResourceChange},
    inventory,
    model::Resource,
};
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgPool, Postgres, Transaction, types::Json};
use std::collections::HashMap;
use uuid::Uuid;

/// An account's automatic bandwidth reduction. It serializes under the keys the cloud page
/// reads beside the account's own.
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct AccountAutomation {
    pub auto_enabled: bool,
    pub limit_gb: i64,
}

impl Default for AccountAutomation {
    fn default() -> Self {
        Self {
            auto_enabled: false,
            limit_gb: 100,
        }
    }
}

/// What operations keeps about a resource. It serializes under the keys the cloud page reads
/// beside the resource's own.
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct ResourceAutomation {
    pub auto_enabled: bool,
    pub cap_mbps: i64,
    pub power_policy: Json<Policy>,
    pub power_state: Option<Json<State>>,
    pub power_checked_at: Option<i64>,
    pub power_error: Option<String>,
    pub next_power_at: i64,
    pub manual_hold: bool,
    pub threshold_hold: bool,
    pub cdt_guard_state: Option<Json<GuardState>>,
    pub cdt_guard_error: Option<String>,
    pub cdt_guard_next_at: i64,
}

impl Default for ResourceAutomation {
    fn default() -> Self {
        Self {
            auto_enabled: false,
            cap_mbps: 1,
            power_policy: Json(Policy::default()),
            power_state: None,
            power_checked_at: None,
            power_error: None,
            next_power_at: 0,
            manual_hold: false,
            threshold_hold: false,
            cdt_guard_state: None,
            cdt_guard_error: None,
            cdt_guard_next_at: 0,
        }
    }
}

/// A resource as operations' cloud work reads it under the account's lock: the resource as
/// resources keeps it, with what operations keeps about it. It reads as the resource.
pub struct Automated {
    pub resource: Resource,
    pub automation: ResourceAutomation,
}

impl std::ops::Deref for Automated {
    type Target = Resource;
    fn deref(&self) -> &Resource {
        &self.resource
    }
}

pub async fn account_on(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> ApiResult<AccountAutomation> {
    Ok(
        sqlx::query_as("SELECT * FROM cloud_account_automation WHERE account_id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .unwrap_or_default(),
    )
}

pub async fn account(pool: &PgPool, id: Uuid) -> ApiResult<AccountAutomation> {
    Ok(
        sqlx::query_as("SELECT * FROM cloud_account_automation WHERE account_id=$1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .unwrap_or_default(),
    )
}

pub async fn resource_on(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> ApiResult<ResourceAutomation> {
    Ok(
        sqlx::query_as("SELECT * FROM cloud_resource_automation WHERE resource_id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .unwrap_or_default(),
    )
}

pub async fn resource(pool: &PgPool, id: Uuid) -> ApiResult<ResourceAutomation> {
    Ok(
        sqlx::query_as("SELECT * FROM cloud_resource_automation WHERE resource_id=$1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .unwrap_or_default(),
    )
}

/// The resource with what operations keeps about it, unless it is archived.
pub async fn current(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<Automated> {
    let resource = inventory::current_resource(tx, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let automation = resource_on(tx, id).await?;
    Ok(Automated {
        resource,
        automation,
    })
}

/// Gives the resource its row, holding the defaults, unless it has one; the caller then
/// updates it.
pub(crate) async fn row(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<()> {
    sqlx::query("INSERT INTO cloud_resource_automation(resource_id) VALUES($1) ON CONFLICT(resource_id) DO NOTHING")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Pauses the resource's automation once an operation whose result stays unknown is
/// dismissed (closing the tracking never undoes a cloud action): automatic bandwidth
/// reduction and the power policy are off, a manual hold keeps automatic starts away, and
/// work prepared under the old revision is discarded.
pub(crate) async fn pause(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<()> {
    row(tx, id).await?;
    sqlx::query("UPDATE cloud_resource_automation SET power_policy=jsonb_set(power_policy,'{enabled}','false'),auto_enabled=false,manual_hold=true WHERE resource_id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    apsp_resources::cloud::revise(tx, id).await
}

#[derive(FromRow)]
struct AccountRow {
    account_id: Uuid,
    #[sqlx(flatten)]
    automation: AccountAutomation,
}

#[derive(FromRow)]
struct ResourceRow {
    resource_id: Uuid,
    #[sqlx(flatten)]
    automation: ResourceAutomation,
}

/// What operations keeps about these accounts and resources, for the cloud page.
pub async fn listed(
    pool: &PgPool,
    accounts: &[Uuid],
    resources: &[Uuid],
) -> ApiResult<(
    HashMap<Uuid, AccountAutomation>,
    HashMap<Uuid, ResourceAutomation>,
)> {
    let account_rows: Vec<AccountRow> =
        sqlx::query_as("SELECT * FROM cloud_account_automation WHERE account_id=ANY($1)")
            .bind(accounts)
            .fetch_all(pool)
            .await?;
    let resource_rows: Vec<ResourceRow> =
        sqlx::query_as("SELECT * FROM cloud_resource_automation WHERE resource_id=ANY($1)")
            .bind(resources)
            .fetch_all(pool)
            .await?;
    Ok((
        account_rows
            .into_iter()
            .map(|row| (row.account_id, row.automation))
            .collect(),
        resource_rows
            .into_iter()
            .map(|row| (row.resource_id, row.automation))
            .collect(),
    ))
}

/// The power columns of the operations page's cloud resources, by resource: the manual
/// hold, the policy and power state as stored and why the last power read failed.
pub async fn resource_views(pool: &PgPool, resources: &[Uuid]) -> ApiResult<HashMap<Uuid, Value>> {
    let rows: Vec<(Uuid, Value)> = sqlx::query_as("SELECT r.id,jsonb_build_object('manual_hold',COALESCE(a.manual_hold,false),'power_policy',COALESCE(a.power_policy,$2::jsonb),'power_state',a.power_state,'power_error',a.power_error) FROM unnest($1::uuid[]) AS r(id) LEFT JOIN cloud_resource_automation a ON a.resource_id=r.id")
        .bind(resources).bind(DEFAULT_POLICY).fetch_all(pool).await?;
    Ok(rows.into_iter().collect())
}

async fn uses_bills(tx: &mut Transaction<'_, Postgres>, resources: &[Uuid]) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM cloud_resource_automation WHERE resource_id=ANY($1) AND power_policy->>'enabled'='true' AND power_policy->>'threshold_action' IN ('notify','stop'))")
        .bind(resources).fetch_one(&mut **tx).await?)
}

async fn save_account(
    tx: &mut Transaction<'_, Postgres>,
    account: Uuid,
    settings: AccountSettings,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO cloud_account_automation(account_id,auto_enabled,limit_gb) VALUES($1,$2,$3) ON CONFLICT(account_id) DO UPDATE SET auto_enabled=EXCLUDED.auto_enabled,limit_gb=EXCLUDED.limit_gb")
        .bind(account).bind(settings.auto_enabled).bind(settings.limit_gb).execute(&mut **tx).await?;
    Ok(())
}

async fn account_changed(
    tx: &mut Transaction<'_, Postgres>,
    account: Uuid,
    change: AccountChange<'_>,
) -> ApiResult<()> {
    match change {
        AccountChange::Created(settings) => save_account(tx, account, settings).await?,
        AccountChange::Edited {
            settings,
            resources,
        } => {
            save_account(tx, account, settings).await?;
            sqlx::query("UPDATE cloud_resource_automation SET next_power_at=0,cdt_guard_next_at=0 WHERE resource_id=ANY($1)")
                .bind(resources).execute(&mut **tx).await?;
        }
        AccountChange::Archived => {
            sqlx::query(
                "UPDATE cloud_account_automation SET auto_enabled=false WHERE account_id=$1",
            )
            .bind(account)
            .execute(&mut **tx)
            .await?;
        }
        AccountChange::Refreshed { resources } => {
            sqlx::query(
                "UPDATE cloud_resource_automation SET next_power_at=0 WHERE resource_id=ANY($1)",
            )
            .bind(resources)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

async fn resource_changed(
    tx: &mut Transaction<'_, Postgres>,
    resource: Uuid,
    change: ResourceChange,
) -> ApiResult<()> {
    match change {
        ResourceChange::Created(settings) | ResourceChange::Edited(settings) => {
            sqlx::query("INSERT INTO cloud_resource_automation(resource_id,auto_enabled,cap_mbps) VALUES($1,$2,$3) ON CONFLICT(resource_id) DO UPDATE SET auto_enabled=EXCLUDED.auto_enabled,cap_mbps=EXCLUDED.cap_mbps")
                .bind(resource).bind(settings.auto_enabled).bind(settings.cap_mbps).execute(&mut **tx).await?;
        }
        ResourceChange::Archived => {
            sqlx::query(
                "UPDATE cloud_resource_automation SET auto_enabled=false WHERE resource_id=$1",
            )
            .bind(resource)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

/// Operations answers what resources' account and resource forms ask about automation.
struct Automation;

impl CloudAutomation for Automation {
    fn uses_bills<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        resources: &'a [Uuid],
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(uses_bills(tx, resources))
    }

    fn account_changed<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        account: Uuid,
        change: AccountChange<'a>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(account_changed(tx, account, change))
    }

    fn resource_changed<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        resource: Uuid,
        change: ResourceChange,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(resource_changed(tx, resource, change))
    }
}

static AUTOMATION: Automation = Automation;

/// Connects the answer to the resources' account and resource forms.
pub fn install() {
    apsp_resources::cloud::automation::install(&AUTOMATION);
}
