//! Fixed device operations, their reconciliation and terminals, and whether a server
//! accepts new work: the device's lifecycle and maintenance, exclusive jobs and
//! operations whose result is still unknown.

mod operations;
pub(crate) mod reconciliation;
mod terminal;
pub(crate) use operations::enqueue_automation_tx;
pub use operations::{enqueue_for_actor, enqueue_for_actor_tx, enqueue_readonly_for_actor_tx};

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use axum::{
    Router,
    routing::{get, post},
};
use sqlx::{PgPool, Postgres, Row, Transaction};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/fleet/operations/{id}", get(operations::get))
        .route(
            "/api/fleet/operations/{id}/inspection",
            post(reconciliation::inspect),
        )
        .route(
            "/api/fleet/operations/{id}/reconcile",
            post(reconciliation::complete),
        )
        .route(
            "/api/servers/{id}/fleet/operations",
            get(operations::list).post(operations::create),
        )
        .route(
            "/api/servers/{id}/fleet/config-history",
            get(operations::history),
        )
        .route(
            "/api/servers/{id}/fleet/terminals",
            get(terminal::list).post(terminal::create),
        )
        .route(
            "/api/fleet/terminals/{id}",
            get(terminal::get).delete(terminal::close),
        )
        .route("/api/fleet/terminals/{id}/input", post(terminal::input))
        .route("/api/agent/v1/fleet/work", get(operations::work))
        .route("/api/agent/v1/fleet/results", post(operations::complete))
        .route("/api/agent/v1/fleet/terminal-events", post(terminal::event))
}

pub async fn ensure_accepts_tasks(pool: &PgPool, server: i64) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    ensure_accepts_tasks_tx(&mut tx, server).await?;
    tx.commit().await?;
    Ok(())
}

/// SQL that holds while the server `server` takes no new work at `at` (both SQL, as for
/// `server_lifecycle::blocks_work_sql`): its own lifecycle blocks work, or a maintenance
/// window that blocks new tasks covers it. Every check before work goes to a device
/// includes this.
pub(crate) fn closed_sql(server: &str, at: &str) -> String {
    format!(
        "({} OR EXISTS(SELECT 1 FROM operations_maintenance closing WHERE {server}=ANY(closing.targets) \
         AND closing.block_new_tasks AND closing.starts_at<={at} AND closing.ends_at>{at}))",
        crate::server_lifecycle::blocks_work_sql(server, at)
    )
}

/// SQL that holds while the server refuses new device work at `at`: it is closed
/// ([`closed_sql`]), an exclusive job holds it, or a fixed operation is in flight, of unknown
/// result, or queued and not expired. The device work interface answers with this.
pub(crate) fn refuses_tasks_sql(server: &str, at: &str) -> String {
    format!(
        "({} OR EXISTS(SELECT 1 FROM operations_server_locks job_lock WHERE job_lock.server_id={server}) \
         OR EXISTS(SELECT 1 FROM fleet_operations fleet_op WHERE fleet_op.server_id={server} \
         AND ((fleet_op.status IN ('dispatched','unknown') AND fleet_op.reconciled_at IS NULL) \
         OR (fleet_op.status='queued' AND fleet_op.expires_at>{at}))))",
        closed_sql(server, at)
    )
}

pub async fn ensure_accepts_tasks_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<()> {
    crate::servers::lock(tx, server).await?;
    let blocked: bool = sqlx::query_scalar(&format!("SELECT {}", refuses_tasks_sql("$1", "$2")))
        .bind(server)
        .bind(apsp_protocol::now_timestamp())
        .fetch_one(&mut **tx)
        .await?;
    if blocked {
        return Err(ApiError::Conflict(
            "服务器处于维护、停止接收任务或互斥操作中，不能接收新任务".into(),
        ));
    }
    Ok(())
}

pub(super) async fn ensure_terminal_input_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<()> {
    crate::servers::lock(tx, server).await?;
    let blocked: bool = sqlx::query_scalar(&format!("SELECT {}", closed_sql("$1", "$2")))
        .bind(server)
        .bind(apsp_protocol::now_timestamp())
        .fetch_one(&mut **tx)
        .await?;
    if blocked {
        return Err(ApiError::Conflict(
            "终端所在设备已进入维护或退役；仍可请求强制关闭".into(),
        ));
    }
    Ok(())
}

pub(super) async fn ensure_accepts_fleet_tasks_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<()> {
    ensure_terminal_input_tx(tx, server).await?;
    let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM operations_server_locks WHERE server_id=$1) OR EXISTS(SELECT 1 FROM fleet_operations WHERE server_id=$1 AND (reconciled_at IS NULL AND (status='unknown' OR (status='dispatched' AND expires_at<=$2))))").bind(server).bind(apsp_protocol::now_timestamp()).fetch_one(&mut **tx).await?;
    if blocked {
        return Err(ApiError::Conflict(
            "设备存在互斥自动化作业或未知结果；核对后再创建日常操作".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn policy(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
) -> ApiResult<apsp_protocol::fleet::AccessPolicy> {
    let value = sqlx::query("SELECT policy FROM fleet_profiles WHERE server_id=$1")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    value
        .map(|row| serde_json::from_value(row.get("policy")))
        .transpose()
        .map_err(|error| ApiError::Internal(error.into()))
        .map(|value| value.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        fs,
        path::{Path, PathBuf},
    };

    /// Files that match maintenance windows by hand: their owners come before operations in
    /// the dependency order, so they cannot call `closed_sql`.
    const OTHER_OWNERS: [(&str, &str); 2] = [
        (
            "traffic/src/runtime_workflows/preflight.rs",
            "the sing-box deployment preflight reports whether the server accepts new tasks; \
             it moves to operations with the deployment glue (architecture.md section 12.1)",
        ),
        (
            "observability/src/incidents.rs",
            "notifications are held while a window that suppresses them covers the server",
        ),
    ];

    /// The panel's Rust sources below `crates/`, without build output, tests or the Agent.
    fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).expect("readable source directory") {
            let entry = entry.expect("directory entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if entry.file_type().expect("file type").is_dir() {
                if !matches!(
                    name.as_str(),
                    "target" | "tests" | "agent" | "agent-core" | "tcp-probe"
                ) && !name.starts_with("adapter-")
                {
                    sources(&path, found);
                }
            } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
                found.push(path);
            }
        }
    }

    /// Whether a maintenance window covers a server is written once, in [`super::closed_sql`]
    /// for windows that block new tasks; another copy would decide the window on its own.
    #[test]
    fn maintenance_windows_are_matched_here() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates directory");
        let mut files = Vec::new();
        sources(crates, &mut files);
        assert!(files.len() > 100, "the workspace sources were not found");
        let (mut misplaced, mut used) = (Vec::new(), BTreeSet::new());
        for path in &files {
            let relative = path
                .strip_prefix(crates)
                .expect("path under crates/")
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "operations/src/fleet/mod.rs" {
                continue;
            }
            let source = fs::read_to_string(path).expect("readable source");
            for (index, line) in source.lines().enumerate() {
                if !line.contains("block_new_tasks AND")
                    && !line.contains("suppress_notifications AND")
                {
                    continue;
                }
                if OTHER_OWNERS.iter().any(|(file, _)| *file == relative) {
                    used.insert(relative.clone());
                } else {
                    misplaced.push(format!("{relative}:{}: {}", index + 1, line.trim()));
                }
            }
        }
        assert!(
            misplaced.is_empty(),
            "ask operations' fleet::closed_sql whether a maintenance window closes a server:\n{}",
            misplaced.join("\n")
        );
        let listed: BTreeSet<_> = OTHER_OWNERS
            .iter()
            .map(|(file, _)| (*file).to_owned())
            .collect();
        assert_eq!(
            used, listed,
            "remove exceptions that no longer match windows"
        );
    }
}
