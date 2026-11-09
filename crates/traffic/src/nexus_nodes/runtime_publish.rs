use super::runtime_model::RuntimeSnapshot;
use crate::{AppState, control_center, error::ApiResult};
use apsp_compiler::preserved::{SessionControl, compile_preserved_server_with, nexus_access};
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
    compile_snapshot(tx, server, snapshot).await
}

/// Compiles a given snapshot for the host, with users from the current policy
/// groups; used for the stored snapshot and for a proposed extension.
pub(super) async fn compile_snapshot(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    snapshot: RuntimeSnapshot,
) -> ApiResult<Candidate> {
    let at = now_timestamp();
    let mut prepared = Vec::new();
    let mut accounting = Vec::new();
    for mut entry in snapshot.nodes {
        let found: Option<i64> = sqlx::query_scalar("SELECT r.node_id FROM singbox_nexus_runtime_nodes r JOIN singbox_nexus_nodes i ON i.id=r.inventory_id WHERE r.server_id=$1 AND i.legacy_node_id=$2")
            .bind(server).bind(entry.legacy_node_id).fetch_optional(&mut **tx).await?;
        let Some(id) = found else {
            // A retired line keeps its snapshot entry as history but loses its
            // inventory row. Any other missing record is still a fault.
            let imported: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_nodes WHERE source_id=$1 AND legacy_node_id=$2)")
                .bind(&snapshot.source_id).bind(entry.legacy_node_id).fetch_one(&mut **tx).await?;
            if imported {
                return Err(sqlx::Error::RowNotFound.into());
            }
            continue;
        };
        entry.preserved.node.id = id;
        // Policy groups decide who may use the line (ADR 0115); members keep
        // their original client identity on it.
        let members: Vec<(i64,Uuid)> = sqlx::query_as("SELECT i.user_id,i.legacy_uuid FROM singbox_nexus_identities i JOIN singbox_entitlements($1) e ON e.user_id=i.user_id AND e.allowed WHERE EXISTS(SELECT 1 FROM singbox_user_policies up JOIN singbox_policy_nodes pn ON pn.group_id=up.group_id WHERE up.user_id=i.user_id AND pn.node_id=$2) ORDER BY i.user_id")
            .bind(at).bind(id).fetch_all(&mut **tx).await?;
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
    // Agents that control sessions get the runtime facilities for device limits.
    let capable: bool = sqlx::query_scalar(
        "SELECT COALESCE(capabilities @> jsonb_build_array($2::text),false) FROM servers WHERE id=$1",
    )
    .bind(server)
    .bind(apsp_protocol::RUNTIME_ADMISSION_CAPABILITY)
    .fetch_one(&mut **tx)
    .await?;
    let control = if capable {
        Some(SessionControl {
            secret: session_secret(tx, server).await?,
        })
    } else {
        None
    };
    let config =
        compile_preserved_server_with(&prepared, control.as_ref()).map_err(anyhow::Error::from)?;
    let config = crate::server_profiles::compile(tx, server, &config).await?;
    let config = crate::users::apply_limits(tx, &config).await?;
    let config = crate::runtime_controls::attach(tx, &config, server).await?;
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
    force: bool,
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
    // An administrator's redeploy publishes the unchanged bundle as a higher revision.
    if !force && previous.as_deref() == Some(&candidate.hash) {
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
    crate::business::lock_publication(&mut tx).await?;
    // One evaluation of every user's entitlement serves all hosts; only hosts
    // whose membership changed are locked and dirtied. The per-host member list
    // and its digest are the same as when each host was read on its own.
    let hosts: Vec<(i64, Value)> = sqlx::query_as("WITH e AS MATERIALIZED (SELECT user_id FROM singbox_entitlements($1) WHERE allowed) SELECT h.server_id,COALESCE((SELECT jsonb_agg(jsonb_build_array(m.node_id,m.user_id,m.legacy_uuid) ORDER BY m.node_id,m.user_id) FROM (SELECT DISTINCT r.node_id,i.user_id,i.legacy_uuid FROM singbox_nexus_runtime_nodes r JOIN singbox_policy_nodes pn ON pn.node_id=r.node_id JOIN singbox_user_policies up ON up.group_id=pn.group_id JOIN e ON e.user_id=up.user_id JOIN singbox_nexus_identities i ON i.user_id=e.user_id WHERE r.server_id=h.server_id) m),'[]'::jsonb) FROM singbox_nexus_runtime_hosts h WHERE h.state='active' ORDER BY h.server_id")
        .bind(now_timestamp()).fetch_all(&mut *tx).await?;
    let digests: Vec<String> = hosts
        .iter()
        .map(|(_, members)| crate::auth::hash_token(&members.to_string()))
        .collect();
    let servers: Vec<i64> = hosts.iter().map(|(server, _)| *server).collect();
    let stale: Vec<i64> = sqlx::query_scalar("SELECT h.server_id FROM singbox_nexus_runtime_hosts h JOIN UNNEST($1::bigint[],$2::text[]) AS d(server_id,digest) USING(server_id) WHERE h.membership_digest IS DISTINCT FROM d.digest ORDER BY h.server_id")
        .bind(&servers).bind(&digests).fetch_all(&mut *tx).await?;
    for (server, digest) in servers.iter().zip(&digests) {
        if stale.binary_search(server).is_err() {
            continue;
        }
        crate::business::lock_server(&mut tx, *server).await?;
        let changed = sqlx::query("UPDATE singbox_nexus_runtime_hosts SET membership_digest=$2 WHERE server_id=$1 AND membership_digest IS DISTINCT FROM $2").bind(server).bind(digest).execute(&mut *tx).await?.rows_affected();
        if changed > 0 {
            crate::business::mark_dirty(&mut tx, &[*server]).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// The host's session control secret, fixed when the host was prepared.
async fn session_secret(tx: &mut Transaction<'_, Postgres>, server: i64) -> anyhow::Result<String> {
    Ok(sqlx::query_scalar(
        "SELECT session_secret FROM singbox_nexus_runtime_hosts WHERE server_id=$1",
    )
    .bind(server)
    .fetch_one(&mut **tx)
    .await?)
}

/// The usage rate of each preserved line on a host, as exact decimal text. Access applies
/// it to the usage it delivers to the bridge.
pub async fn runtime_rates(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> anyhow::Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as(
        "SELECT node_id,rate::text FROM singbox_nexus_runtime_nodes WHERE server_id=$1 ORDER BY node_id",
    )
    .bind(server)
    .fetch_all(&mut **tx)
    .await?)
}
