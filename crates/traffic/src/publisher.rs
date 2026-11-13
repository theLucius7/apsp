use crate::{
    AppState,
    business::{NODE_COLUMNS, NodeRow},
    device_channel,
};
use apsp_compiler::{Access, Node};
use apsp_protocol::{Bundle, Envelope, ManifestChanged, now_timestamp};
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeMap;

const MODULE: &str = "singbox";
const DUE: &str = "dirty_at <= FLOOR(EXTRACT(EPOCH FROM clock_timestamp())*1000)::bigint - 5000";

pub async fn publish_due(state: &AppState) -> anyhow::Result<()> {
    let query = format!(
        "SELECT s.id FROM servers s LEFT JOIN server_plugins p ON p.server_id=s.id AND p.plugin='sing-box' WHERE s.deleted_at IS NULL AND {DUE} AND ({}) IS NOT NULL ORDER BY s.id",
        super::settings::SOURCE_SQL
    );
    let ids: Vec<i64> = sqlx::query_scalar(&query).fetch_all(&state.pool).await?;
    super::nexus_nodes::runtime_publish::membership(state).await?;
    let mut first_error = None;
    for server_id in ids {
        if let Err(error) = publish_server(state, server_id, false).await {
            tracing::error!(server_id, %error, "server publication failed");
            first_error.get_or_insert(error);
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn snapshot(
    tx: &mut Transaction<'_, Postgres>,
    server_id: i64,
    at: i64,
) -> anyhow::Result<Vec<Node>> {
    let query = format!(
        "SELECT {NODE_COLUMNS} FROM nodes n WHERE n.server_id=$1 AND n.deleted_at IS NULL ORDER BY n.id"
    );
    let nodes = sqlx::query_as::<_, NodeRow>(&query)
        .bind(server_id)
        .fetch_all(&mut **tx)
        .await?;
    let rows = sqlx::query("SELECT a.node_id,a.user_id,a.uuid,a.credential FROM singbox_eligible_accesses($2) a JOIN nodes n ON n.id=a.node_id WHERE n.server_id=$1 ORDER BY a.node_id,a.user_id")
        .bind(server_id).bind(at).fetch_all(&mut **tx).await?;
    let mut accesses: BTreeMap<i64, Vec<Access>> = BTreeMap::new();
    for row in rows {
        accesses
            .entry(row.get("node_id"))
            .or_default()
            .push(Access {
                user_id: row.get("user_id"),
                uuid: row.get("uuid"),
                credential: row.get("credential"),
            });
    }
    nodes
        .iter()
        .map(|node| node.model(accesses.remove(&node.id).unwrap_or_default()))
        .collect()
}

/// Publishes the current configuration of one server as a new revision even when its bundle
/// is unchanged. An Agent replaces a certified activation only for a higher revision, so this
/// is the administrator's recovery after the service manager restarted the runtime outside
/// the Agent (a host reboot, for example); a still-valid activation applies it without a
/// restart. `None` means nothing was published: the server is being published right now,
/// awaits a deployment preflight, or is not eligible.
pub(crate) async fn redeploy(state: &AppState, server_id: i64) -> anyhow::Result<Option<u64>> {
    publish_server(state, server_id, true).await
}

async fn publish_server(
    state: &AppState,
    server_id: i64,
    force: bool,
) -> anyhow::Result<Option<u64>> {
    let mut retries = 0;
    let revision = loop {
        match publish_server_transaction(state, server_id, force).await {
            Ok(revision) => break revision,
            Err(error)
                if retries < 2
                    && error
                        .chain()
                        .filter_map(|cause| cause.downcast_ref::<sqlx::Error>())
                        .any(|error| {
                            error
                                .as_database_error()
                                .is_some_and(|database| database.code().as_deref() == Some("40001"))
                        }) =>
            {
                // Waiting on the authorization lock can outlive the RR snapshot.
                // Retry the complete uncommitted ledger/configuration transaction;
                // a fresh snapshot may simply find that another publisher finished it.
                retries += 1;
                tokio::task::yield_now().await;
            }
            Err(error) => return Err(error),
        }
    };
    if let Some(rev) = revision {
        device_channel::notify(
            state,
            server_id,
            Envelope::new("manifest.changed", ManifestChanged { rev })?,
        )
        .await;
    }
    Ok(revision)
}

/// One server's desired bundle and the evidence recorded with it.
struct Compiled {
    plan: super::ordered_paths::publication::Plan,
    source: serde_json::Value,
    bundle: String,
    hash: String,
}

async fn compile(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server_id: i64,
    at: i64,
) -> anyhow::Result<Compiled> {
    let nodes = snapshot(tx, server_id, at).await?;
    let plan = super::ordered_paths::publication::plan(state, tx, server_id, at, nodes).await?;
    let source = super::ordered_paths::publication::source(&plan)?;
    let native = apsp_compiler::compile_server_with_paths(
        &plan.nodes,
        &plan.paths,
        &plan.accepts,
        plan.probe_control.as_ref(),
    )?;
    let native = super::server_profiles::compile(tx, server_id, &native).await?;
    let native = super::users::apply_limits(tx, &native).await?;
    let native = super::runtime_controls::attach(tx, &native, server_id).await?;
    let mut files = BTreeMap::from([("config.json".into(), native)]);
    if let Some(probes) = &plan.probe_plan {
        files.insert("runtime-probes.json".into(), serde_json::to_string(probes)?);
    }
    let bundle = serde_json::to_string(&Bundle { files })?;
    let hash = crate::auth::hash_token(&bundle);
    Ok(Compiled {
        plan,
        source,
        bundle,
        hash,
    })
}

async fn publish_server_transaction(
    state: &AppState,
    server_id: i64,
    force: bool,
) -> anyhow::Result<Option<u64>> {
    let mut tx = state.pool.begin().await?;
    // Authorization and relay selection must see the same ledger and clock.
    // Otherwise a reset/expiry between queries could leave an entry with direct routing.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    crate::business::lock_publication(&mut tx).await?;
    let at = now_timestamp();
    let query = format!(
        "SELECT s.manifest_rev FROM servers s LEFT JOIN server_plugins p ON p.server_id=s.id AND p.plugin='sing-box' WHERE s.id=$1 AND s.deleted_at IS NULL AND ({DUE} OR $2) AND ({}) IS NOT NULL FOR UPDATE OF s SKIP LOCKED",
        super::settings::SOURCE_SQL
    );
    let Some(manifest_rev) = sqlx::query_scalar::<_, i64>(&query)
        .bind(server_id)
        .bind(force)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Ok(None);
    };
    // Device declarations may have changed since the due-work scan. Recheck
    // under the server lock before creating legacy deployment evidence.
    if !super::settings::is_enabled(&mut tx, server_id).await? {
        tx.commit().await?;
        return Ok(None);
    }
    if let Some(result) =
        super::nexus_nodes::runtime_publish::publish(state, &mut tx, server_id, manifest_rev, force)
            .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let Compiled {
        plan,
        source,
        bundle,
        hash,
    } = compile(state, &mut tx, server_id, at).await?;
    let previous = sqlx::query("SELECT rev,bundle_sha256 FROM deployments WHERE server_id=$1 AND module=$2 ORDER BY rev DESC LIMIT 1")
        .bind(server_id).bind(MODULE).fetch_optional(&mut *tx).await?;
    let previous_revision = previous.as_ref().map(|row| row.get::<i64, _>("rev"));
    let force_runtime_candidate:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_runtime_rollout_members m JOIN singbox_runtime_rollouts r ON r.id=m.rollout_id WHERE m.server_id=$1 AND m.baseline_revision>=$2 AND r.completed_at IS NULL)").bind(server_id).bind(previous_revision).fetch_one(&mut *tx).await?;
    if let Some(previous) = previous.filter(|row| {
        row.get::<String, _>("bundle_sha256") == hash && !force_runtime_candidate && !force
    }) {
        // Public display metadata is separate from immutable accounting/path evidence.
        super::ordered_paths::publication::record(
            &mut tx,
            server_id,
            previous.get::<i64, _>("rev"),
            &hash,
            &plan,
        )
        .await?;
        // Subscription-only metadata may change without changing native bytes.
        sqlx::query(
            "INSERT INTO singbox_deployment_projections(server_id,rev,source_json) VALUES($1,$2,$3) ON CONFLICT(server_id,rev) DO UPDATE SET source_json=EXCLUDED.source_json",
        )
        .bind(server_id)
        .bind(previous.get::<i64, _>("rev"))
        .bind(&source)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
            .bind(server_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(None);
    }
    let changed =
        crate::runtime_workflows::ordinary_shape_changes_tx(&mut tx, server_id, &plan.nodes)
            .await?;
    if !changed.is_empty()
        && !crate::runtime_workflows::publisher_ready_tx(state, server_id, &mut tx).await?
    {
        // Roll back plan bookkeeping as well as deployment writes. The dirty
        // desired state remains visible until a current preflight is confirmed.
        tx.rollback().await?;
        tracing::debug!(
            server_id,
            changed_node_count = changed.len(),
            "configuration awaits confirmed deployment preflight"
        );
        return Ok(None);
    }
    let rev = manifest_rev
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("revision overflow"))?;
    sqlx::query("INSERT INTO deployments(server_id,module,rev,bundle,bundle_sha256,source_json,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(server_id).bind(MODULE).bind(rev).bind(bundle).bind(&hash).bind(source).bind(now_timestamp()).execute(&mut *tx).await?;
    super::ordered_paths::publication::record(&mut tx, server_id, rev, &hash, &plan).await?;
    sqlx::query("INSERT INTO server_module_status(server_id,module,target_rev,updated_at) VALUES($1,$2,$3,$4) ON CONFLICT(server_id,module) DO UPDATE SET target_rev=EXCLUDED.target_rev,updated_at=EXCLUDED.updated_at")
        .bind(server_id).bind(MODULE).bind(rev).bind(now_timestamp()).execute(&mut *tx).await?;
    sqlx::query("UPDATE servers SET manifest_rev=$2,dirty_at=NULL WHERE id=$1")
        .bind(server_id)
        .bind(rev)
        .execute(&mut *tx)
        .await?;
    let revision = rev.try_into()?;
    tx.commit().await?;
    Ok(Some(revision))
}

#[cfg(test)]
mod tests;

pub(crate) async fn preview_profile(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server_id: i64,
) -> anyhow::Result<()> {
    compile(state, tx, server_id, now_timestamp()).await?;
    Ok(())
}
