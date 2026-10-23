//! Undoes `apply`: numbered sources become authoritative again and every
//! ordered row of the imported sources is removed, including rows refreshes
//! added after the migration. Refuses while a grant or an ordered chain
//! depends on something numbered sources do not have.

use crate::error::ApiResult;
use apsp_protocol::now_timestamp;
use serde::Serialize;
use sqlx::PgConnection;

#[derive(Default, Serialize)]
pub struct Outcome {
    pub rolled_back: bool,
    pub restored_sources: u64,
    pub restored_accesses: u64,
    pub removed_nodes: u64,
    /// Catalog settings of nodes first seen after the migration.
    pub removed_metadata: u64,
    pub blockers: Vec<String>,
}

/// Ordered-source history is immutable during normal operation; only a
/// rollback removes it, with these triggers disabled inside its transaction.
const IMMUTABLE: [(&str, &str); 3] = [
    (
        "singbox_subscription_revision_nodes",
        "singbox_subscription_membership_immutable",
    ),
    (
        "singbox_ordered_external_node_versions",
        "singbox_external_version_immutable",
    ),
    (
        "singbox_subscription_source_revisions",
        "singbox_subscription_revision_immutable",
    ),
];

async fn triggers(connection: &mut PgConnection, action: &str) -> ApiResult<()> {
    for (table, trigger) in IMMUTABLE {
        sqlx::query(&format!("ALTER TABLE {table} {action} TRIGGER {trigger}"))
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

pub(super) async fn run(tx: &mut PgConnection) -> ApiResult<Outcome> {
    let mut outcome = Outcome::default();
    if !super::migrated(tx).await? {
        outcome.blockers.push("not_migrated".into());
        return Ok(outcome);
    }
    let grants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM singbox_external_accesses a WHERE NOT EXISTS(SELECT 1 FROM singbox_external_node_versions v WHERE v.id=a.node_version_id AND v.external_node_id=a.external_node_id)")
        .fetch_one(&mut *tx).await?;
    if grants > 0 {
        outcome.blockers.push(format!(
            "grants_on_nodes_or_versions_after_migration:{grants}"
        ));
    }
    let chains: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT h.chain_id) FROM singbox_ordered_chain_hops h JOIN singbox_source_id_map m ON m.b_source_id=h.source_id")
        .fetch_one(&mut *tx).await?;
    if chains > 0 {
        outcome
            .blockers
            .push(format!("ordered_chains_on_imported_sources:{chains}"));
    }
    if !outcome.blockers.is_empty() {
        return Ok(outcome);
    }

    sqlx::query("UPDATE singbox_source_migration SET migrated_at=NULL,report=report||jsonb_build_object('rolled_back_at',$1::BIGINT)")
        .bind(now_timestamp())
        .execute(&mut *tx)
        .await?;
    // Catalog adoption changed after the migration is kept.
    sqlx::query("UPDATE singbox_external_nodes a SET adopted=o.adopted FROM singbox_ordered_external_nodes o WHERE o.public_id=a.id AND o.adopted<>a.adopted")
        .execute(&mut *tx)
        .await?;
    outcome.restored_accesses = sqlx::query("UPDATE singbox_external_accesses a SET source_id=m.a_source_id FROM singbox_source_id_map m WHERE a.source_id=m.b_source_id")
        .execute(&mut *tx).await?.rows_affected();
    outcome.removed_metadata = sqlx::query("DELETE FROM singbox_node_metadata d USING singbox_ordered_external_nodes o, singbox_source_id_map m WHERE d.kind='external' AND d.id=o.public_id AND o.source_id=m.b_source_id AND NOT EXISTS(SELECT 1 FROM singbox_external_nodes a WHERE a.id=o.public_id)")
        .execute(&mut *tx).await?.rows_affected();

    let sources: Vec<i64> = sqlx::query_scalar("SELECT b_source_id FROM singbox_source_id_map")
        .fetch_all(&mut *tx)
        .await?;
    sqlx::query("UPDATE singbox_ordered_subscription_sources SET current_success_revision=NULL WHERE id=ANY($1)")
        .bind(&sources).execute(&mut *tx).await?;
    sqlx::query("UPDATE singbox_subscription_source_jobs SET source_revision_id=NULL WHERE source_id=ANY($1)")
        .bind(&sources).execute(&mut *tx).await?;
    sqlx::query("UPDATE singbox_ordered_external_nodes SET latest_version=NULL,last_seen_revision=NULL WHERE source_id=ANY($1)")
        .bind(&sources).execute(&mut *tx).await?;
    triggers(tx, "DISABLE").await?;
    sqlx::query("DELETE FROM singbox_subscription_revision_nodes WHERE source_revision_id IN (SELECT id FROM singbox_subscription_source_revisions WHERE source_id=ANY($1))")
        .bind(&sources).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM singbox_ordered_external_node_versions WHERE node_id IN (SELECT id FROM singbox_ordered_external_nodes WHERE source_id=ANY($1))")
        .bind(&sources).execute(&mut *tx).await?;
    outcome.removed_nodes =
        sqlx::query("DELETE FROM singbox_ordered_external_nodes WHERE source_id=ANY($1)")
            .bind(&sources)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    sqlx::query("DELETE FROM singbox_source_revision_map")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM singbox_subscription_source_revisions WHERE source_id=ANY($1)")
        .bind(&sources)
        .execute(&mut *tx)
        .await?;
    triggers(tx, "ENABLE").await?;
    sqlx::query("DELETE FROM singbox_subscription_source_jobs WHERE source_id=ANY($1)")
        .bind(&sources)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM singbox_subscription_source_requests WHERE source_id=ANY($1)")
        .bind(&sources)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM singbox_source_id_map")
        .execute(&mut *tx)
        .await?;
    outcome.restored_sources =
        sqlx::query("DELETE FROM singbox_ordered_subscription_sources WHERE id=ANY($1)")
            .bind(&sources)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    outcome.rolled_back = true;
    Ok(outcome)
}
