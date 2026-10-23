use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
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
    super::business::lock_server(&mut tx, id).await?;
    super::settings::require_enabled(&mut tx, id).await?;
    let pending: bool = sqlx::query_scalar("SELECT dirty_at IS NOT NULL FROM servers WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let enabled_nodes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM nodes WHERE server_id=$1 AND deleted_at IS NULL AND enabled",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let authorized_nodes: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT n.id) FROM nodes n JOIN singbox_eligible_accesses($2) a ON a.node_id=n.id WHERE n.server_id=$1 AND n.deleted_at IS NULL").bind(id).bind(apsp_protocol::now_timestamp()).fetch_one(&mut *tx).await?;
    let status = sqlx::query_as("SELECT module,target_rev,applied_rev,last_result_rev,healthy,last_error,updated_at FROM server_module_status WHERE server_id=$1 AND module='singbox'").bind(id).fetch_optional(&mut *tx).await?;
    let history = sqlx::query_as("SELECT module,rev,bundle_sha256,created_at FROM deployments WHERE server_id=$1 AND module='singbox' ORDER BY rev DESC LIMIT 100").bind(id).fetch_all(&mut *tx).await?;
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

// Explicit inspection avoids re-verifying archives on every status poll.
pub async fn check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Readiness>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    super::business::lock_server(&mut tx, id).await?;
    super::settings::require_enabled(&mut tx, id).await?;
    let row = sqlx::query("SELECT device_public_key IS NOT NULL AS registered,last_seen,capabilities,static_info FROM servers WHERE id=$1").bind(id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    let registered: bool = row.get("registered");
    let online = row
        .get::<Option<i64>, _>("last_seen")
        .is_some_and(|at| apsp_protocol::now_timestamp().saturating_sub(at) <= 60);
    let supported = row
        .get::<Value, _>("capabilities")
        .as_array()
        .is_some_and(|caps| caps.iter().any(|cap| cap == "singbox"));
    let signature_supported = row
        .get::<Value, _>("capabilities")
        .as_array()
        .is_some_and(|caps| {
            caps.iter().any(|cap| {
                cap.as_str() == Some(apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY)
            })
        });
    let mut checks = vec![
        Check {
            name: "Agent 接入",
            passed: registered,
            detail: if registered {
                "已接入"
            } else {
                "先在服务器详情页安装并接入 Agent"
            }
            .into(),
        },
        Check {
            name: "设备在线",
            passed: online,
            detail: if online {
                "近期已收到设备连接"
            } else {
                "设备离线，配置将在重新连接后下发"
            }
            .into(),
        },
        Check {
            name: "运行时能力",
            passed: supported,
            detail: if supported {
                "设备已声明 sing-box 支持"
            } else {
                "尚未收到 sing-box 能力声明，请检查 Agent 版本与安装参数"
            }
            .into(),
        },
        Check {
            name: "制品验签能力",
            passed: signature_supported,
            detail: if signature_supported {
                "设备支持独立制品验签"
            } else {
                "此 Agent 尚不支持制品验签，请先使用可信安装器升级；已运行配置和流量上报继续保留"
            }
            .into(),
        },
    ];
    let artifact = if registered {
        match super::agent::runtime_artifact(&state, &row.get("static_info")).await {
            Ok(_) => Check { name: "签名运行时", passed: true, detail: "已找到与设备平台匹配的 sing-box 1.14.2 制品，签名验证通过".into() },
            Err(ApiError::NotFound) => Check { name: "签名运行时", passed: false, detail: "面板缺少此平台的签名运行时，请维护者按部署文档导入匹配 Release；插件目录可查看已收录版本".into() },
            Err(ApiError::BadRequest(message)) => Check { name: "签名运行时", passed: false, detail: message },
            Err(error) => return Err(error),
        }
    } else {
        Check {
            name: "签名运行时",
            passed: false,
            detail: "等待设备上报平台后检查制品".into(),
        }
    };
    checks.push(artifact);
    Ok(Json(Readiness {
        ready: checks.iter().all(|item| item.passed),
        checks,
    }))
}
