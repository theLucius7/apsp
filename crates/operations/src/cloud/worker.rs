use crate::cloud::{automation, bandwidth};
use crate::error::ApiResult;
use apsp_costs::cloud::{accounts, billing, cost_cache, traffic_history};
use apsp_panel_host::maintenance::{self, Cadence, Failure, Pass};
use apsp_resources::cloud::{client::Cloud, inventory, lock, model::Target};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

/// Refreshes one account in three steps so the account lock is never held
/// across cloud calls, which can take tens of seconds and would hold back the
/// CDT guard, power jobs and edits: claim the pass, call the cloud without a
/// transaction, then store the results unless the account or a resource was
/// edited meanwhile. An edit resets the schedule, so its refresh follows at once.
/// The account's revision covers its billing and automatic bandwidth settings too, so the
/// bill read here is discarded with the rest when they change.
pub async fn refresh(pool: &PgPool, id: Uuid, cloud: &Cloud) -> ApiResult<()> {
    let now = apsp_protocol::now_timestamp();
    let Some((account, resources)) = inventory::claim_refresh(pool, id, now).await? else {
        return Ok(());
    };
    let bill = if accounts::account(pool, id).await?.billing_enabled {
        Some(billing::bill(cloud, &account, now).await)
    } else {
        None
    };
    // Compatibility counters are display-only: no trustworthy period field is supplied.
    let traffic = match cloud.keys(&account).await {
        Ok(keys) => billing::traffic(cloud, &keys, now)
            .await
            .map(|traffic| (traffic, traffic_history::identity(&account, &keys))),
        Err(error) => Err(error),
    };
    let mut snapshots = Vec::with_capacity(resources.len());
    for resource in resources {
        let snapshot = cloud.snapshot(&account, &resource).await;
        snapshots.push((resource, snapshot));
    }

    let mut tx = lock(pool, id).await?;
    let current = inventory::current_account(&mut tx, id).await?;
    let Some(account) = current.filter(|current| current.revision == account.revision) else {
        // Edited or archived meanwhile: these results describe the old settings.
        return Ok(());
    };
    let mut costs = accounts::account_on(&mut tx, id).await?;
    let automatic = automation::account_on(&mut tx, id).await?;
    if let Some(bill) = bill {
        cost_cache::store_bill(&mut tx, id, &mut costs, bill, now).await?;
    }
    traffic_history::store_traffic(&mut tx, id, traffic).await?;
    for (polled, snapshot) in snapshots {
        let resource = inventory::current_resource(&mut tx, polled.id).await?;
        // The snapshot describes the cloud, not local settings, so it stays valid
        // across a resource edit; automatic action uses the settings read here.
        let Some(resource) = resource else {
            continue;
        };
        // A bandwidth change, preview or manual refresh stores its own snapshot
        // under the lock. One stored since this pass began is newer than ours,
        // so ours must neither replace it nor drive automatic action.
        if resource.checked_at.is_some_and(|checked| checked >= now) {
            continue;
        }
        // A running EIP rotation is moving this instance between addresses; a
        // snapshot read meanwhile can show a candidate about to be released.
        // The rotation stores the final address itself when it ends.
        let rotating: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alicloud_eip_rotations WHERE resource_id=$1 AND status='running')")
            .bind(resource.id).fetch_one(&mut *tx).await?;
        if rotating {
            continue;
        }
        let snapshot = match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => {
                inventory::snapshot_failed(&mut tx, resource.id, error.code).await?;
                continue;
            }
        };
        apsp_resources::cloud::snapshot_on(&mut tx, resource.id, &snapshot).await?;
        let settings = automation::resource_on(&mut tx, resource.id).await?;
        if !settings.auto_enabled
            || snapshot.attached_eip_id.is_some()
            || !bandwidth::exceeded(&account, &automatic, &costs, apsp_protocol::now_timestamp())
            || snapshot.charge_type != "PayByTraffic"
            || snapshot.bandwidth_mbps <= settings.cap_mbps
        {
            continue;
        }
        let cycle = billing::month(now);
        let existing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alicloud_operations WHERE resource_id=$1 AND (status IN ('queued','running','uncertain') OR (source='automatic' AND billing_cycle=$2 AND account_revision=$3 AND resource_revision=$4))) OR EXISTS(SELECT 1 FROM alicloud_power_jobs WHERE resource_id=$1 AND status IN ('queued','running','uncertain'))")
            .bind(resource.id).bind(&cycle).bind(account.revision).bind(resource.revision).fetch_one(&mut *tx).await?;
        if existing {
            continue;
        }
        let target = Target {
            bandwidth_mbps: settings.cap_mbps,
            charge_type: "PayByTraffic".into(),
        };
        bandwidth::prepare(
            &mut tx,
            &account,
            &resource,
            &snapshot,
            &target,
            Some(&cycle),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn run(pool: PgPool) {
    tokio::join!(
        maintenance::periodic(
            "alicloud",
            pool.clone(),
            Cadence::every(Duration::from_secs(5)),
            pool.clone(),
            |pool| Box::pin(scheduler(pool)),
        ),
        super::power::run(pool.clone()),
        super::eip_rotation::run(pool.clone()),
        maintenance::periodic(
            "alicloud-cost-cache",
            pool.clone(),
            Cadence::every(Duration::from_secs(10)),
            pool,
            |pool| Box::pin(ancillary(pool)),
        )
    );
}

async fn ancillary(pool: &PgPool) -> Pass {
    let costs = match Cloud::new(pool) {
        Ok(cloud) => cost_cache::tick(pool, &cloud).await,
        Err(error) => Err(apsp_resources::cloud::failure(error)),
    };
    let deliveries = apsp_observability::cloud_notices::dispatch(pool).await;
    let mut errors = Vec::new();
    if let Err(error) = costs {
        tracing::warn!(%error,"Cloud cost cache failed");
        errors.push(serde_json::json!({"stage":"cost_cache","error":error.to_string()}));
    }
    if let Err(error) = deliveries {
        tracing::warn!(%error,"Cloud notification delivery failed");
        errors.push(serde_json::json!({"stage":"notification_dispatch","error":error.to_string()}));
    }
    let details = serde_json::json!({"scope":"cost_cache_and_notification_cycle","completed":errors.is_empty(),"errors":errors,"remote_resource_health_confirmed":false,"bill_freshness":"以资源实际查询时间为准"});
    if errors.is_empty() {
        Ok(details)
    } else {
        Err(Failure::logged(details))
    }
}

async fn scheduler(pool: &PgPool) -> Pass {
    match tick(pool).await {
        Ok(()) => Ok(
            serde_json::json!({"scope":"scheduler_and_reconciliation_cycle","completed":true,"remote_resource_health_confirmed":false}),
        ),
        Err(error) => Err(Failure::new(
            &error,
            serde_json::json!({"scope":"scheduler_and_reconciliation_cycle","completed":false,"error":error.to_string(),"remote_resource_health_confirmed":false}),
        )),
    }
}

static LAST_PRUNE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

async fn tick(pool: &PgPool) -> ApiResult<()> {
    let cloud = Cloud::new(pool).map_err(apsp_resources::cloud::failure)?;
    let now = apsp_protocol::now_timestamp();
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM alicloud_operations WHERE status IN ('queued','running','uncertain') AND next_check_at<=$1 ORDER BY next_check_at,created_at LIMIT 8")
        .bind(now).fetch_all(pool).await?;
    let mut first_error = None;
    for id in ids {
        if let Err(error) = bandwidth::process(pool, id, &cloud).await {
            tracing::warn!(%error,%id,"Cloud operation reconciliation failed");
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    for id in inventory::due_accounts(pool, now).await? {
        refresh(pool, id, &cloud).await?;
    }
    sqlx::query("UPDATE alicloud_operations SET status='cancelled',updated_at=$1 WHERE status='preview' AND expires_at<=$1").bind(now).execute(pool).await?;
    // Retention is measured in months; checking it hourly instead of on every
    // five-second tick avoids scanning both tables 17,000 times a day.
    if apsp_resources::cloud::prune_due(&LAST_PRUNE, now) {
        apsp_costs::cloud::traffic_history::prune(pool, now - 32 * 86400).await?;
        sqlx::query("DELETE FROM alicloud_operations WHERE status IN ('preview','cancelled','dismissed','failed','succeeded') AND created_at<$1")
            .bind(now-180*86400).execute(pool).await?;
        apsp_observability::cloud_notices::prune(pool, now - 180 * 86400).await?;
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(())
}
