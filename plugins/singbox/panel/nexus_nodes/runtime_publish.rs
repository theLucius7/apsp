use super::runtime_model::RuntimeSnapshot;
use crate::{AppState, control_center, error::ApiResult};
use apsp_compiler::preserved::{compile_preserved_server, nexus_access};
use apsp_protocol::{Bundle, now_timestamp};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

pub(crate) struct Candidate {
    pub bundle: String,
    pub hash: String,
    pub config_hash: String,
    pub source: Value,
}

pub(crate) async fn compile(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<Candidate> {
    let credential: Uuid = sqlx::query_scalar(
        "SELECT credential_id FROM singbox_nexus_runtime_hosts WHERE server_id=$1",
    )
    .bind(server)
    .fetch_one(&mut **tx)
    .await?;
    let raw = control_center::credentials::resolve_reference(
        state,
        credential,
        "backup",
        "nexus-runtime-publisher",
    )
    .await?;
    let snapshot: RuntimeSnapshot = serde_json::from_value(raw).map_err(anyhow::Error::from)?;
    let at = now_timestamp();
    let mut prepared = Vec::new();
    let mut accounting = Vec::new();
    for mut entry in snapshot.nodes {
        let (id, groups): (i64, Vec<i64>) = sqlx::query_as("SELECT r.node_id,r.legacy_group_ids FROM singbox_nexus_runtime_nodes r JOIN singbox_nexus_nodes i ON i.id=r.inventory_id WHERE r.server_id=$1 AND i.legacy_node_id=$2")
            .bind(server).bind(entry.legacy_node_id).fetch_one(&mut **tx).await?;
        entry.preserved.node.id = id;
        let members: Vec<(i64,Uuid)> = sqlx::query_as("SELECT i.user_id,i.legacy_uuid FROM singbox_nexus_identities i JOIN singbox_entitlements($1) e ON e.user_id=i.user_id JOIN singbox_nexus_plan_mappings p ON p.package_group_id=e.package_group_id AND p.import_id=i.import_id WHERE e.allowed AND p.legacy_group_id=ANY($2) ORDER BY i.user_id")
            .bind(at).bind(groups).fetch_all(&mut **tx).await?;
        for (user, uuid) in members {
            entry.preserved.node.users.push(nexus_access(
                user,
                uuid,
                &entry.preserved.node.protocol_config,
            ));
            if entry.preserved.node.enabled {
                accounting.push(json!({"user_id":user,"node_id":id,"stat_name":apsp_compiler::stat_name(user,id)}));
            }
        }
        prepared.push(entry.preserved);
    }
    let config = compile_preserved_server(&prepared).map_err(anyhow::Error::from)?;
    let config_hash = crate::auth::hash_token(&config);
    let bundle = serde_json::to_string(&Bundle {
        files: BTreeMap::from([("config.json".into(), config)]),
    })
    .map_err(anyhow::Error::from)?;
    let hash = crate::auth::hash_token(&bundle);
    Ok(Candidate {
        bundle,
        hash,
        config_hash,
        source: json!({"schema_version":1,"public_nodes":[],"path_dependencies":[],"accounting_users":accounting}),
    })
}

/// None means ordinary publication; Some(None) means deliberately held.
pub(crate) async fn publish(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    manifest: i64,
) -> anyhow::Result<Option<Option<u64>>> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT state FROM singbox_nexus_runtime_hosts WHERE server_id=$1")
            .bind(server)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(status) = status else {
        return Ok(None);
    };
    if status != "active" {
        return Ok(Some(None));
    }
    let candidate = compile(state, tx, server).await?;
    let previous: Option<String> = sqlx::query_scalar("SELECT bundle_sha256 FROM deployments WHERE server_id=$1 AND module='singbox' ORDER BY rev DESC LIMIT 1").bind(server).fetch_optional(&mut **tx).await?;
    if previous.as_deref() == Some(&candidate.hash) {
        sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
            .bind(server)
            .execute(&mut **tx)
            .await?;
        return Ok(Some(None));
    }
    let revision = manifest
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("revision overflow"))?;
    sqlx::query("INSERT INTO deployments(server_id,module,rev,bundle,bundle_sha256,source_json,created_at) VALUES($1,'singbox',$2,$3,$4,$5,$6)")
        .bind(server).bind(revision).bind(candidate.bundle).bind(candidate.hash).bind(candidate.source).bind(now_timestamp()).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO server_module_status(server_id,module,target_rev,updated_at) VALUES($1,'singbox',$2,$3) ON CONFLICT(server_id,module) DO UPDATE SET target_rev=EXCLUDED.target_rev,updated_at=EXCLUDED.updated_at")
        .bind(server).bind(revision).bind(now_timestamp()).execute(&mut **tx).await?;
    sqlx::query("UPDATE servers SET manifest_rev=$2,dirty_at=NULL WHERE id=$1")
        .bind(server)
        .bind(revision)
        .execute(&mut **tx)
        .await?;
    Ok(Some(Some(revision.try_into()?)))
}

/// Group changes must also dirty migrated hosts even when eligibility stays true.
pub(crate) async fn membership(state: &AppState) -> anyhow::Result<()> {
    // Every publisher tick calls this; avoid the entitlement lock until a host
    // has actually been handed over.
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_nexus_runtime_hosts WHERE state='active')",
    )
    .fetch_one(&state.pool)
    .await?;
    if !active {
        return Ok(());
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let hosts: Vec<i64> = sqlx::query_scalar(
        "SELECT server_id FROM singbox_nexus_runtime_hosts WHERE state='active' ORDER BY server_id",
    )
    .fetch_all(&mut *tx)
    .await?;
    for server in hosts {
        crate::business::lock_server(&mut tx, server).await?;
        let members: Value = sqlx::query_scalar("SELECT COALESCE(jsonb_agg(jsonb_build_array(r.node_id,i.user_id,i.legacy_uuid) ORDER BY r.node_id,i.user_id),'[]'::jsonb) FROM singbox_nexus_runtime_nodes r JOIN singbox_nexus_plan_mappings p ON p.legacy_group_id=ANY(r.legacy_group_ids) JOIN singbox_entitlements($2) e ON e.package_group_id=p.package_group_id AND e.allowed JOIN singbox_nexus_identities i ON i.user_id=e.user_id AND i.import_id=p.import_id WHERE r.server_id=$1")
            .bind(server).bind(now_timestamp()).fetch_one(&mut *tx).await?;
        let digest = crate::auth::hash_token(&members.to_string());
        let changed = sqlx::query("UPDATE singbox_nexus_runtime_hosts SET membership_digest=$2 WHERE server_id=$1 AND membership_digest IS DISTINCT FROM $2").bind(server).bind(digest).execute(&mut *tx).await?.rows_affected();
        if changed > 0 {
            crate::business::mark_dirty(&mut tx, &[server]).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn enqueue_usage(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    batch: &apsp_protocol::UsageBatch,
) -> anyhow::Result<()> {
    {
        // Ingest already validates the published identity and inserts its raw
        // record in this transaction. Preserve the multiplier for every retry.
        sqlx::query("INSERT INTO singbox_nexus_usage_deliveries(server_id,epoch,seq,stat_name,user_id,uplink,downlink,rate,period_start,period_end) SELECT u.server_id,u.epoch,u.seq,u.stat_name,u.user_id,u.uplink,u.downlink,r.rate,u.period_start,u.period_end FROM usage_records u JOIN singbox_nexus_runtime_nodes r ON r.node_id=u.node_id AND r.server_id=u.server_id WHERE u.server_id=$1 AND u.epoch=$2 AND u.seq=$3::text::numeric ON CONFLICT(server_id,epoch,seq,stat_name) DO NOTHING")
            .bind(server).bind(batch.epoch).bind(batch.seq.to_string()).execute(&mut **tx).await?;
    }
    Ok(())
}
