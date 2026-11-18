use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use apsp_protocol::{RuntimeOperation, RuntimeOperationRequest};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, Row};

#[derive(Serialize, FromRow)]
pub struct DeploymentStatus {
    pub module: String,
    pub target_rev: i64,
    pub applied_rev: i64,
    pub last_result_rev: i64,
    pub healthy: bool,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

#[derive(Serialize, FromRow)]
pub struct DeploymentHistory {
    pub module: String,
    pub rev: i64,
    pub bundle_sha256: String,
    pub created_at: i64,
}

#[derive(Serialize)]
pub struct DeploymentView {
    pub status: Option<DeploymentStatus>,
    pub history: Vec<DeploymentHistory>,
    pub pending: bool,
    pub enabled_nodes: i64,
    pub authorized_nodes: i64,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<DeploymentView>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    crate::servers::lock(&mut tx, id).await?;
    if !super::settings::is_configured(&mut tx, id).await? {
        tx.commit().await?;
        return Ok(Json(DeploymentView {
            status: None,
            history: Vec::new(),
            pending: false,
            enabled_nodes: 0,
            authorized_nodes: 0,
        }));
    }
    let pending = crate::publications::pending(&mut tx, id).await?;
    let enabled_nodes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM nodes WHERE server_id=$1 AND deleted_at IS NULL AND enabled",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let authorized_nodes: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT n.id) FROM nodes n JOIN singbox_eligible_accesses($2) a ON a.node_id=n.id WHERE n.server_id=$1 AND n.deleted_at IS NULL",
    )
    .bind(id)
    .bind(apsp_protocol::now_timestamp())
    .fetch_one(&mut *tx)
    .await?;
    let status = sqlx::query_as(
        "SELECT module,target_rev,applied_rev,last_result_rev,healthy,last_error,updated_at FROM server_module_status WHERE server_id=$1 AND module='singbox' AND target_rev>0",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let history = sqlx::query_as(
        "SELECT module,rev,bundle_sha256,created_at FROM deployments WHERE server_id=$1 AND module='singbox' ORDER BY rev DESC LIMIT 100",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(DeploymentView {
        status,
        history,
        pending,
        enabled_nodes,
        authorized_nodes,
    }))
}

#[derive(Serialize)]
pub struct Readiness {
    pub ready: bool,
    pub checks: Vec<Check>,
}

#[derive(Serialize)]
pub struct Check {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

pub async fn check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Readiness>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    crate::servers::lock(&mut tx, id).await?;
    let row = sqlx::query(
        "SELECT device_public_key IS NOT NULL AS registered,last_seen,capabilities,static_info FROM servers WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let registered: bool = row.get("registered");
    let online = row
        .get::<Option<i64>, _>("last_seen")
        .is_some_and(|at| apsp_protocol::now_timestamp().saturating_sub(at) <= 60);
    let capabilities: Value = row.get("capabilities");
    let static_info: Value = row.get("static_info");
    let supported = super::agent::supports_self_contained_singbox(&capabilities);
    let raw_component = static_info
        .get("components")
        .and_then(|components| components.get("sing-box"))
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty());
    let component_mismatch = raw_component.is_some_and(|version| {
        super::agent::embedded_runtime_version(&static_info).as_deref()
            != Some(crate::RUNTIME_VERSION)
            || version != crate::RUNTIME_VERSION
    });
    let embedded = super::agent::embedded_runtime_version(&static_info);
    let active = super::agent::reported_runtime_version(&static_info);
    let builtin_ready = supported && !component_mismatch;
    let builtin_detail = if builtin_ready {
        match (embedded.as_deref(), active.as_deref()) {
            (Some(embedded), Some(running)) => {
                format!("设备已声明一体化内置 sing-box（内置 {embedded}，当前运行 {running}）")
            }
            (Some(embedded), None) => {
                format!("设备已声明一体化内置 sing-box（内置 {embedded}，当前尚未运行）")
            }
            (None, Some(running)) => {
                format!("设备已声明一体化内置 sing-box 能力（当前运行 {running}）")
            }
            (None, None) => "设备已声明一体化内置 sing-box 能力".into(),
        }
    } else if component_mismatch {
        format!(
            "设备上报的内置 sing-box 版本与面板固定版本 {} 不一致，请升级 Agent",
            crate::RUNTIME_VERSION
        )
    } else if registered {
        "当前 Agent 未声明内置 sing-box 一体化能力（旧版或仅监控模式），请升级或调整 Agent".into()
    } else {
        "等待设备接入后确认内置 sing-box 能力".into()
    };
    let checks = vec![
        Check {
            name: "Agent 接入",
            passed: registered,
            detail: if registered {
                "设备已接入并绑定身份公钥".into()
            } else {
                "先在服务器详情页安装并接入一体化 Agent".into()
            },
        },
        Check {
            name: "设备在线",
            passed: online,
            detail: if online {
                "近期已收到设备连接".into()
            } else {
                "设备离线，配置将在重新连接后自动下发应用".into()
            },
        },
        Check {
            name: "内置运行时能力",
            passed: builtin_ready,
            detail: builtin_detail,
        },
    ];
    Ok(Json(Readiness {
        ready: checks.iter().all(|item| item.passed),
        checks,
    }))
}

#[derive(Serialize)]
pub struct Redeployed {
    pub revision: u64,
}

/// Publishes the server's current configuration again as a new revision (see
/// `publisher::redeploy`) as an explicit administrator recovery action.
pub async fn redeploy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Redeployed>> {
    crate::control_center::require_capability(&state, &headers, "proxy:write").await?;
    crate::control_center::require_server(&state, &headers, id, "proxy:write").await?;
    crate::control_center::require_recent_proof(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    crate::servers::lock(&mut tx, id).await?;
    super::settings::require_configured(&mut tx, id).await?;
    let deployed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM deployments WHERE server_id=$1 AND module='singbox')",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    if !deployed {
        return Err(ApiError::Conflict(
            "此服务器还没有部署过配置，不需要重新部署".into(),
        ));
    }
    let revision = super::publisher::redeploy(&state, id)
        .await?
        .ok_or_else(|| {
            ApiError::Conflict(
                "暂时不能生成新版本：配置正在合并发布或服务器当前无活跃服务配置，请稍后重试".into(),
            )
        })?;
    Ok(Json(Redeployed { revision }))
}

/// What a server last reported for its sing-box module, with the bundle of the applied revision
/// while it is still stored. Other domains read deployment state through this.
pub struct AppliedState {
    pub applied_rev: i64,
    pub target_rev: i64,
    pub healthy: bool,
    pub bundle: Option<String>,
}

pub async fn applied_state(
    connection: &mut sqlx::PgConnection,
    server_id: i64,
) -> sqlx::Result<Option<AppliedState>> {
    let row = sqlx::query(
        "SELECT m.applied_rev,m.target_rev,m.healthy,d.bundle FROM server_module_status m LEFT JOIN deployments d ON d.server_id=m.server_id AND d.module=m.module AND d.rev=m.applied_rev WHERE m.server_id=$1 AND m.module='singbox'",
    )
    .bind(server_id)
    .fetch_optional(connection)
    .await?;
    Ok(row.map(|row| AppliedState {
        applied_rev: row.get("applied_rev"),
        target_rev: row.get("target_rev"),
        healthy: row.get("healthy"),
        bundle: row.get("bundle"),
    }))
}

#[derive(FromRow)]
struct RuntimeDeployment {
    target_rev: i64,
    applied_rev: i64,
    last_result_rev: i64,
    last_error: Option<String>,
}

/// Whether a runtime operation fits the server's current sing-box deployment, for the
/// runtime operations queues. A read always does; anything else needs a positive expected
/// target revision with no publication pending, a restart needs the target applied and a
/// retry needs a failed deployment of the target.
pub async fn require_runtime_operation_fits(
    connection: &mut sqlx::PgConnection,
    server_id: i64,
    request: &RuntimeOperationRequest,
    publication_pending: bool,
) -> ApiResult<()> {
    if request.operation == RuntimeOperation::Inspect {
        return Ok(());
    }
    let deployment: RuntimeDeployment = sqlx::query_as(
        "SELECT target_rev,applied_rev,last_result_rev,last_error FROM server_module_status WHERE server_id=$1 AND module='singbox'",
    )
    .bind(server_id)
    .fetch_optional(connection)
    .await?
    .filter(|row: &RuntimeDeployment| row.target_rev > 0)
    .ok_or_else(|| ApiError::Conflict("尚无可操作的运行时部署".into()))?;
    if publication_pending || request.expected_revision != u64::try_from(deployment.target_rev).ok()
    {
        return Err(ApiError::Conflict(
            "期望版本已改变或正在发布，请刷新后重试".into(),
        ));
    }
    match request.operation {
        RuntimeOperation::Restart if deployment.applied_rev != deployment.target_rev => {
            Err(ApiError::Conflict("目标版本尚未应用，请先完成部署".into()))
        }
        RuntimeOperation::RetryDeployment
            if deployment.last_result_rev != deployment.target_rev
                || deployment.last_error.is_none() =>
        {
            Err(ApiError::Conflict("当前期望版本没有失败部署可重试".into()))
        }
        _ => Ok(()),
    }
}
