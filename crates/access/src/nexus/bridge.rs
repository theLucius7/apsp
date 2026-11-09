//! The compatibility bridge to the retained Nexus renderer (ADR 0115): which proxy users
//! are its members, where their subscription is served, which packages map to original
//! groups, and the usage it delivers back. Nothing else in access names the bridge's
//! tables, so the bridge exits with this module.

use apsp_protocol::UsageBatch;
use sqlx::{PgConnection, Postgres, Transaction};

/// The members of the enabled bridge, joinable as `i` (their Nexus identity) and `b` (the
/// bridge): each user with an identity whose import the bridge currently serves.
pub const MEMBERS: &str = "singbox_nexus_identities i JOIN singbox_nexus_bridge b ON b.import_id=i.import_id AND b.enabled";

/// SQL that is true when the user named by `user` (a column or a parameter) is a member of
/// the enabled bridge.
pub fn member_sql(user: &str) -> String {
    format!("EXISTS(SELECT 1 FROM {MEMBERS} WHERE i.user_id={user})")
}

/// SQL for the bridge's subscription base of the user named by `user`; NULL for users the
/// bridge does not serve.
pub fn subscription_base_sql(user: &str) -> String {
    format!("(SELECT b.subscription_base FROM {MEMBERS} WHERE i.user_id={user})")
}

/// Whether the user is a member of the enabled bridge.
pub async fn is_member(connection: &mut PgConnection, user: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(&format!("SELECT {}", member_sql("$1")))
        .bind(user)
        .fetch_one(connection)
        .await
}

/// Whether the bridge runs at all.
pub async fn enabled(connection: &mut PgConnection) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_bridge WHERE enabled)")
        .fetch_one(connection)
        .await
}

/// Whether the enabled bridge has synchronized since the given time.
pub async fn synced_since(connection: &mut PgConnection, since: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_nexus_bridge WHERE enabled AND last_sync_at>=$1)",
    )
    .bind(since)
    .fetch_one(connection)
    .await
}

/// Whether the enabled bridge lacks an original group for this package. While the bridge
/// runs, its members can only hold packages it can translate.
pub async fn package_unmapped(
    connection: &mut PgConnection,
    package_group_id: i64,
) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_bridge b WHERE b.enabled AND NOT EXISTS(SELECT 1 FROM singbox_nexus_plan_mappings p WHERE p.import_id=b.import_id AND p.package_group_id=$1 AND p.legacy_group_id IS NOT NULL))")
        .bind(package_group_id)
        .fetch_one(connection)
        .await
}

/// The enabled bridge's packages and the original groups they stand for:
/// (original group, package group, package name).
pub async fn plan_mappings(connection: &mut PgConnection) -> sqlx::Result<Vec<(i64, i64, String)>> {
    sqlx::query_as("SELECT p.legacy_group_id,p.package_group_id,g.name FROM singbox_nexus_plan_mappings p JOIN singbox_nexus_bridge b ON b.import_id=p.import_id AND b.enabled JOIN singbox_package_groups g ON g.id=p.package_group_id WHERE p.legacy_group_id IS NOT NULL ORDER BY g.name,g.id")
        .fetch_all(connection)
        .await
}

/// Queues the members' usage of a batch for the bridge, multiplied by each preserved
/// line's rate. Ingest has already stored the raw records in this transaction; the rate is
/// fixed with each delivery so every retry reports the same amount.
pub async fn enqueue_usage(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    batch: &UsageBatch,
) -> anyhow::Result<()> {
    let (nodes, rates): (Vec<i64>, Vec<String>) =
        apsp_traffic::nexus_nodes::runtime_rates(tx, server)
            .await?
            .into_iter()
            .unzip();
    if nodes.is_empty() {
        return Ok(());
    }
    sqlx::query("INSERT INTO singbox_nexus_usage_deliveries(server_id,epoch,seq,stat_name,user_id,uplink,downlink,rate,period_start,period_end) SELECT u.server_id,u.epoch,u.seq,u.stat_name,u.user_id,u.uplink,u.downlink,r.rate::numeric,u.period_start,u.period_end FROM usage_records u JOIN unnest($4::bigint[],$5::text[]) AS r(node_id,rate) ON r.node_id=u.node_id WHERE u.server_id=$1 AND u.epoch=$2 AND u.seq=$3::text::numeric ON CONFLICT(server_id,epoch,seq,stat_name) DO NOTHING")
        .bind(server)
        .bind(batch.epoch)
        .bind(batch.seq.to_string())
        .bind(&nodes)
        .bind(&rates)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
