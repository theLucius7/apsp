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

/// `allowed` is the publication cycle's evaluation of which users are allowed now
/// (the eligibility refresh that starts the cycle); without it the host membership
/// check evaluates entitlements itself.
pub async fn publish_due(state: &AppState, allowed: Option<&[i64]>) -> anyhow::Result<()> {
    let ids = crate::publications::due(&state.pool).await?;
    super::nexus_nodes::runtime_publish::membership(state, allowed).await?;
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
/// is unchanged. `None` means nothing was published: the server is being published right now
/// or currently has no active service configuration.
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
    let Some(manifest_rev) = crate::publications::claim_tx(&mut tx, server_id, force).await? else {
        return Ok(None);
    };
    let current_target: Option<i64> = sqlx::query_scalar(
        "SELECT target_rev FROM server_module_status WHERE server_id=$1 AND module=$2",
    )
    .bind(server_id)
    .bind(MODULE)
    .fetch_optional(&mut *tx)
    .await?;
    if !super::settings::is_configured(&mut tx, server_id).await? {
        if force {
            return Ok(None);
        }
        let had_active_target = current_target.is_some_and(|rev| rev > 0);
        if had_active_target {
            let rev = manifest_rev
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("revision overflow"))?;
            sqlx::query(
                "UPDATE server_module_status SET target_rev=0,healthy=false,last_error=NULL,updated_at=$3 WHERE server_id=$1 AND module=$2",
            )
            .bind(server_id)
            .bind(MODULE)
            .bind(at)
            .execute(&mut *tx)
            .await?;
            crate::publications::record_tx(&mut tx, server_id, rev).await?;
            let revision = rev.try_into()?;
            tx.commit().await?;
            return Ok(Some(revision));
        }
        crate::publications::settle_tx(&mut tx, server_id).await?;
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
    let previous = sqlx::query(
        "SELECT rev,bundle_sha256 FROM deployments WHERE server_id=$1 AND module=$2 ORDER BY rev DESC LIMIT 1",
    )
    .bind(server_id)
    .bind(MODULE)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(previous) = previous.filter(|row| {
        let rev: i64 = row.get("rev");
        row.get::<String, _>("bundle_sha256") == hash
            && current_target == Some(rev)
            && rev == manifest_rev
            && !force
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
        crate::publications::settle_tx(&mut tx, server_id).await?;
        tx.commit().await?;
        return Ok(None);
    }
    let rev = manifest_rev
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("revision overflow"))?;
    sqlx::query("INSERT INTO deployments(server_id,module,rev,bundle,bundle_sha256,source_json,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(server_id).bind(MODULE).bind(rev).bind(bundle).bind(&hash).bind(source).bind(at).execute(&mut *tx).await?;
    super::ordered_paths::publication::record(&mut tx, server_id, rev, &hash, &plan).await?;
    sqlx::query("INSERT INTO server_module_status(server_id,module,target_rev,updated_at) VALUES($1,$2,$3,$4) ON CONFLICT(server_id,module) DO UPDATE SET target_rev=EXCLUDED.target_rev,updated_at=EXCLUDED.updated_at")
        .bind(server_id).bind(MODULE).bind(rev).bind(at).execute(&mut *tx).await?;
    crate::publications::record_tx(&mut tx, server_id, rev).await?;
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
