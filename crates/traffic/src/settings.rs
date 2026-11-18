use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgConnection, PgPool, Postgres, Row, Transaction};

/// SQL that holds when `server` (a parameter or qualified column such as `"$1"` or `"s.id"`)
/// is a preserved Nexus compatibility host managed only through the Nexus runtime endpoints.
pub fn read_only_sql(server: &str) -> String {
    format!("EXISTS(SELECT 1 FROM singbox_nexus_runtime_hosts h WHERE h.server_id={server})")
}

/// SQL expression returning `'nexus'`, `'configured'` or `NULL` for `server` based strictly on
/// current non-deleted, non-retired service intent (never historical deployments or profiles).
pub fn source_sql(server: &str) -> String {
    let server_retired = apsp_resources::server_lifecycle::retired_sql(server);
    let server_held = crate::nexus_nodes::held_host_sql(server);
    let es_retired = apsp_resources::server_lifecycle::retired_sql("es.id");
    let es_held = crate::nexus_nodes::held_host_sql("es.id");
    let ms_retired = apsp_resources::server_lifecycle::retired_sql("ms.id");
    let ms_held = crate::nexus_nodes::held_host_sql("ms.id");
    format!(
        "CASE \
            WHEN EXISTS(SELECT 1 FROM servers srv WHERE srv.id={server} AND srv.deleted_at IS NOT NULL) \
                OR {server_retired} \
                OR {server_held} \
            THEN NULL \
            WHEN EXISTS(SELECT 1 FROM singbox_nexus_runtime_hosts h WHERE h.server_id={server} AND h.state='active') \
            THEN 'nexus' \
            WHEN EXISTS( \
                SELECT 1 FROM nodes n \
                WHERE n.server_id={server} \
                  AND n.deleted_at IS NULL \
                  AND n.enabled \
                  AND NOT EXISTS( \
                      SELECT 1 FROM singbox_chains c \
                      WHERE c.entry_node_id=n.id \
                        AND (c.deleted_at IS NULL OR (c.path_kind='ordered' AND c.phase<>'retired')) \
                  ) \
            ) OR EXISTS( \
                SELECT 1 FROM singbox_chains c \
                JOIN nodes en ON en.id=c.entry_node_id \
                JOIN servers es ON es.id=en.server_id \
                WHERE c.path_kind='ordered' \
                  AND c.deleted_at IS NULL \
                  AND c.phase NOT IN ('retiring','retired') \
                  AND (c.route_enabled OR c.candidate_generation IS NOT NULL) \
                  AND en.deleted_at IS NULL \
                  AND en.enabled \
                  AND es.deleted_at IS NULL \
                  AND NOT {es_retired} \
                  AND NOT {es_held} \
                  AND NOT EXISTS( \
                      SELECT 1 FROM singbox_ordered_chain_hops mh \
                      LEFT JOIN nodes mn ON mn.id=mh.managed_node_id \
                      LEFT JOIN servers ms ON ms.id=mh.managed_server_id \
                      WHERE mh.chain_id=c.id \
                        AND mh.kind='managed' \
                        AND mh.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) \
                        AND (mn.id IS NULL OR mn.deleted_at IS NOT NULL OR NOT mn.enabled OR ms.id IS NULL OR ms.deleted_at IS NOT NULL OR {ms_retired} OR {ms_held}) \
                  ) \
                  AND ( \
                      en.server_id={server} \
                      OR EXISTS( \
                          SELECT 1 FROM singbox_ordered_chain_hops h \
                          WHERE h.chain_id=c.id \
                            AND h.kind='managed' \
                            AND h.managed_server_id={server} \
                            AND h.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) \
                      ) \
                  ) \
            ) OR EXISTS( \
                SELECT 1 FROM singbox_chains rc \
                WHERE rc.path_kind='ordered' \
                  AND rc.phase='retiring' \
                  AND ( \
                      EXISTS(SELECT 1 FROM singbox_path_deployment_dependencies d WHERE d.server_id={server} AND d.module='singbox' AND d.chain_id=rc.id) \
                      OR EXISTS(SELECT 1 FROM singbox_path_stage_deployments sd WHERE sd.chain_id=rc.id AND sd.server_id={server}) \
                      OR EXISTS(SELECT 1 FROM singbox_deployment_public_projection p WHERE p.server_id={server} AND p.module='singbox' AND p.node_id=rc.entry_node_id) \
                  ) \
            ) \
            THEN 'configured' \
            ELSE NULL \
        END"
    )
}

/// SQL boolean expression answering whether `server` currently desires a running `sing-box`.
pub fn configured_sql(server: &str) -> String {
    format!("(({}) IS NOT NULL)", source_sql(server))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceRuntimeState {
    #[default]
    Unconfigured,
    WaitingAgent,
    UpgradeRequired,
    Offline,
    Pending,
    Ready,
    SyncBlocked,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RuntimeStatus {
    pub state: ServiceRuntimeState,
    pub reason: String,
    pub target_rev: i64,
    pub applied_rev: i64,
    pub current_healthy: bool,
}

#[derive(Serialize, FromRow)]
pub struct RuntimeServer {
    pub id: i64,
    pub name: String,
    #[sqlx(default)]
    pub configured: bool,
    #[sqlx(default)]
    pub online: bool,
    #[sqlx(default)]
    pub agent_supported: bool,
    #[sqlx(default)]
    pub read_only: bool,
    pub source: Option<String>,
    #[sqlx(skip)]
    pub runtime_version: Option<String>,
    #[sqlx(skip)]
    pub status: RuntimeStatus,
    #[serde(skip)]
    pub registered: bool,
    #[serde(skip)]
    pub last_seen: Option<i64>,
    #[serde(skip)]
    pub capabilities: Value,
    #[serde(skip)]
    pub static_info: Value,
    #[serde(skip)]
    pub pending: bool,
    #[serde(skip)]
    pub target_rev: Option<i64>,
    #[serde(skip)]
    pub applied_rev: Option<i64>,
    #[serde(skip)]
    pub last_result_rev: Option<i64>,
    #[serde(skip)]
    pub healthy: Option<bool>,
    #[serde(skip)]
    pub last_error: Option<String>,
    #[serde(skip)]
    pub below_recovery_floor: bool,
    #[serde(skip)]
    pub lifecycle_blocked: bool,
    #[serde(skip)]
    pub checkpoint_required: bool,
}

impl RuntimeServer {
    fn view(mut self) -> Self {
        self.configured = self.source.is_some();
        self.agent_supported = crate::agent::supports_self_contained_singbox(&self.capabilities);
        self.online = self
            .last_seen
            .is_some_and(|seen| now_timestamp().saturating_sub(seen) <= 60);
        self.runtime_version = crate::agent::reported_runtime_version(&self.static_info);
        self.status = self.status_view();
        self
    }

    fn status_view(&self) -> RuntimeStatus {
        let target = self.target_rev.filter(|revision| *revision > 0);
        let target_num = target.unwrap_or(0).max(0);
        let applied_num = self.applied_rev.unwrap_or(0).max(0);
        let incompatible_component = self
            .static_info
            .get("components")
            .and_then(|components| components.get("sing-box"))
            .and_then(Value::as_str)
            .filter(|version| !version.is_empty())
            .is_some_and(|version| version != crate::RUNTIME_VERSION);
        let (state, reason) = if !self.configured {
            if self.read_only {
                (
                    ServiceRuntimeState::Unconfigured,
                    "兼容接管主机尚未激活或已暂停本面板代理服务".into(),
                )
            } else if self.pending || applied_num > 0 {
                (
                    ServiceRuntimeState::Unconfigured,
                    "当前无启用的节点或链路，等待设备停止受管 sing-box".into(),
                )
            } else {
                (
                    ServiceRuntimeState::Unconfigured,
                    "尚未配置代理节点或链路，纯监控模式不运行 sing-box".into(),
                )
            }
        } else if !self.registered {
            (
                ServiceRuntimeState::WaitingAgent,
                "服务配置已保存，等待服务器安装并接入一体化 Agent".into(),
            )
        } else if self.below_recovery_floor {
            (
                ServiceRuntimeState::SyncBlocked,
                "目标配置含低于设备已确认恢复边界的路径代数，等待发布安全的较高代数".into(),
            )
        } else if self.online && (!self.agent_supported || incompatible_component) {
            (
                ServiceRuntimeState::UpgradeRequired,
                if incompatible_component {
                    format!(
                        "当前 Agent 内置 sing-box 版本与面板固定版本 {} 不一致，请升级 Agent",
                        crate::RUNTIME_VERSION
                    )
                } else {
                    "当前连接的 Agent 为旧版或监控受限模式，需升级为带内置 sing-box 的一体化 Agent"
                        .into()
                },
            )
        } else if !self.online {
            if self.last_seen.is_none() {
                (
                    ServiceRuntimeState::WaitingAgent,
                    "设备已完成注册，等待首次连接并同步服务配置".into(),
                )
            } else if !self.agent_supported || incompatible_component {
                (
                    ServiceRuntimeState::UpgradeRequired,
                    "上次连接的 Agent 尚未具备匹配的内置 sing-box 一体化能力，请升级 Agent".into(),
                )
            } else {
                (
                    ServiceRuntimeState::Offline,
                    "服务配置已保存，设备当前离线，重连后自动同步应用".into(),
                )
            }
        } else if self.lifecycle_blocked
            && (self.pending || target.is_none() || self.applied_rev != target)
        {
            (
                ServiceRuntimeState::SyncBlocked,
                "服务器处于维护或排空窗口，暂缓同步新目标配置，保留现有健康运行状态".into(),
            )
        } else if target.is_some()
            && self.applied_rev == target
            && self.healthy == Some(true)
            && !self.pending
        {
            (
                ServiceRuntimeState::Ready,
                "Agent 已应用最新目标配置，运行时健康检查通过".into(),
            )
        } else if let Some(error) = self.last_error.as_ref().filter(|error| {
            !error.is_empty() && target.is_some() && self.last_result_rev.unwrap_or(0) >= target_num
        }) {
            (
                ServiceRuntimeState::Failed,
                format!("Agent 应用目标配置失败：{error}"),
            )
        } else if self.checkpoint_required
            && target.is_some()
            && self.last_result_rev.unwrap_or(0) >= target_num
            && self.healthy == Some(false)
        {
            (
                ServiceRuntimeState::Pending,
                "设备已回报应用，等待核对实际配置文件与受管进程身份".into(),
            )
        } else if target.is_some()
            && self.last_result_rev.unwrap_or(0) >= target_num
            && self.healthy == Some(false)
        {
            (
                ServiceRuntimeState::Failed,
                "设备未通过运行时健康检查".into(),
            )
        } else {
            (
                ServiceRuntimeState::Pending,
                if self.pending || target.is_none() {
                    "服务配置待合并发布并下发至一体化 Agent".into()
                } else {
                    "目标配置已下发，等待一体化 Agent 自动启动应用并回报健康".into()
                },
            )
        };
        RuntimeStatus {
            state,
            reason,
            target_rev: target_num,
            applied_rev: applied_num,
            current_healthy: self.configured
                && self.online
                && applied_num > 0
                && self.healthy == Some(true),
        }
    }
}

fn query() -> String {
    format!(
        "SELECT s.id,s.name,s.device_public_key IS NOT NULL AS registered,s.last_seen,s.capabilities,s.static_info,{} AS pending,{} AS read_only,{} AS source,m.target_rev,m.applied_rev,m.last_result_rev,m.healthy,m.last_error,EXISTS(SELECT 1 FROM singbox_path_deployment_dependencies d JOIN singbox_chains c ON c.id=d.chain_id WHERE d.server_id=s.id AND d.module='singbox' AND d.revision=m.target_rev AND d.route_active AND d.generation<c.minimum_generation) AS below_recovery_floor,{} AS lifecycle_blocked,COALESCE(r.checkpoint_required,false) AS checkpoint_required FROM servers s LEFT JOIN server_module_status m ON m.server_id=s.id AND m.module='singbox' LEFT JOIN runtime_control_devices r ON r.server_id=s.id WHERE s.deleted_at IS NULL",
        crate::publications::pending_sql("s.id"),
        read_only_sql("s.id"),
        source_sql("s.id"),
        apsp_resources::server_lifecycle::blocks_work_sql("s.id", "$1"),
    )
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<RuntimeServer>>> {
    auth::require_admin(&state, &headers).await?;
    let rows = sqlx::query_as::<_, RuntimeServer>(&format!("{} ORDER BY s.id", query()))
        .bind(now_timestamp())
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(rows.into_iter().map(RuntimeServer::view).collect()))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<RuntimeServer>> {
    auth::require_admin(&state, &headers).await?;
    let row = sqlx::query_as::<_, RuntimeServer>(&format!("{} AND s.id=$2", query()))
        .bind(now_timestamp())
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(row.view()))
}

pub async fn is_configured_on(connection: &mut PgConnection, id: i64) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar(&format!("SELECT {}", configured_sql("$1")))
            .bind(id)
            .fetch_one(connection)
            .await?,
    )
}

pub async fn is_configured(tx: &mut Transaction<'_, Postgres>, id: i64) -> ApiResult<bool> {
    is_configured_on(&mut **tx, id).await
}

pub async fn require_configured(tx: &mut Transaction<'_, Postgres>, id: i64) -> ApiResult<()> {
    if !is_configured(tx, id).await? {
        return Err(ApiError::Conflict(
            "此服务器当前尚未配置启用的代理节点或链路服务".into(),
        ));
    }
    Ok(())
}

pub async fn is_read_only(connection: &mut PgConnection, id: i64) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar(&format!("SELECT {}", read_only_sql("$1")))
            .bind(id)
            .fetch_one(connection)
            .await?,
    )
}

/// Checks that `id` is a live, non-retiring, non-Nexus-preserved server where ordinary
/// nodes, ordered chains or server profiles may be saved (even before any configuration exists).
pub async fn require_writable(tx: &mut Transaction<'_, Postgres>, id: i64) -> ApiResult<()> {
    let row = sqlx::query(&format!(
        "SELECT s.deleted_at IS NOT NULL AS deleted,{} AS retired,{} AS read_only FROM servers s WHERE s.id=$1",
        apsp_resources::server_lifecycle::retired_sql("s.id"),
        read_only_sql("s.id"),
    ))
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    if row.get::<bool, _>("deleted") {
        return Err(ApiError::NotFound);
    }
    if row.get::<bool, _>("retired") {
        return Err(ApiError::Conflict(
            "服务器已退役或正在退役，不能修改网络服务配置".into(),
        ));
    }
    if row.get::<bool, _>("read_only") {
        return Err(ApiError::Conflict(
            "此服务器为兼容接管保留主机，不能直接修改普通网络服务配置".into(),
        ));
    }
    Ok(())
}

/// What a collection migration leaves behind on `server`: the runtimes currently configured on
/// it by the names the console shows (`"sing-box"`), and each module's deployment state.
pub async fn references(pool: &PgPool, server: i64) -> sqlx::Result<(Vec<String>, Vec<Value>)> {
    let configured: bool = sqlx::query_scalar(&format!("SELECT {}", configured_sql("$1")))
        .bind(server)
        .fetch_one(pool)
        .await?;
    let modules = if configured {
        vec!["sing-box".into()]
    } else {
        Vec::new()
    };
    let deployments = sqlx::query_scalar(
        "SELECT jsonb_build_object('module',module,'target_rev',target_rev,'applied_rev',applied_rev,'healthy',healthy) FROM server_module_status WHERE server_id=$1 ORDER BY module",
    )
    .bind(server)
    .fetch_all(pool)
    .await?;
    Ok((modules, deployments))
}

#[cfg(test)]
mod runtime_status_tests {
    use super::*;
    use serde_json::json;

    fn sample() -> RuntimeServer {
        RuntimeServer {
            id: 1,
            name: "TEST_ONLY runtime state".into(),
            configured: false,
            online: false,
            agent_supported: false,
            read_only: false,
            source: Some("configured".into()),
            runtime_version: None,
            status: RuntimeStatus::default(),
            registered: true,
            last_seen: Some(now_timestamp()),
            capabilities: json!([apsp_protocol::SELF_CONTAINED_AGENT_CAPABILITY, "singbox",]),
            static_info: json!({
                "agent_version": "0.4.0",
                "components": { "sing-box": crate::RUNTIME_VERSION }
            }),
            pending: false,
            target_rev: Some(2),
            applied_rev: Some(1),
            last_result_rev: Some(1),
            healthy: Some(true),
            last_error: None,
            below_recovery_floor: false,
            lifecycle_blocked: false,
            checkpoint_required: false,
        }
    }

    #[test]
    fn unconfigured_server_never_claims_ready_or_current_health() {
        let mut row = sample();
        row.source = None;
        row.applied_rev = Some(2);
        let viewed = row.view();
        assert!(!viewed.configured);
        assert_eq!(viewed.status.state, ServiceRuntimeState::Unconfigured);
        assert!(!viewed.status.current_healthy);
    }

    #[test]
    fn blocked_sync_preserves_existing_device_health() {
        let mut row = sample();
        row.below_recovery_floor = true;
        let status = row.view().status;
        assert_eq!(status.state, ServiceRuntimeState::SyncBlocked);
        assert!(status.current_healthy);
        assert_eq!((status.target_rev, status.applied_rev), (2, 1));
    }

    #[test]
    fn legacy_agent_requires_upgrade_without_discarding_existing_health() {
        let mut row = sample();
        row.capabilities = json!(["singbox"]);
        let viewed = row.view();
        assert!(!viewed.agent_supported);
        assert_eq!(viewed.status.state, ServiceRuntimeState::UpgradeRequired);
        assert!(viewed.status.current_healthy);
    }

    #[test]
    fn offline_or_unapplied_runtime_does_not_claim_current_health() {
        for applied in [Some(0), Some(1)] {
            let mut row = sample();
            row.applied_rev = applied;
            row.last_seen = Some(0);
            let viewed = row.view();
            assert_eq!(viewed.status.state, ServiceRuntimeState::Offline);
            assert!(!viewed.status.current_healthy);
        }
        let mut row = sample();
        row.applied_rev = Some(0);
        assert!(!row.view().status.current_healthy);
    }

    #[test]
    fn unapplied_embedded_component_never_masquerades_as_active_runtime_version() {
        let viewed = sample().view();
        assert_eq!(viewed.runtime_version, None);

        let mut active = sample();
        active.static_info["runtime_version"] = json!(crate::RUNTIME_VERSION);
        assert_eq!(
            active.view().runtime_version.as_deref(),
            Some(crate::RUNTIME_VERSION)
        );
    }

    #[test]
    fn runtime_server_serializes_exact_public_contract_keys() {
        let mut row = sample();
        row.applied_rev = Some(2);
        let value = serde_json::to_value(row.view()).expect("serialize RuntimeServer");
        let keys: Vec<_> = value.as_object().expect("object").keys().cloned().collect();
        assert_eq!(
            keys,
            vec![
                "id",
                "name",
                "configured",
                "online",
                "agent_supported",
                "read_only",
                "source",
                "runtime_version",
                "status",
            ]
        );
        let status_keys: Vec<_> = value["status"]
            .as_object()
            .expect("status object")
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            status_keys,
            vec![
                "state",
                "reason",
                "target_rev",
                "applied_rev",
                "current_healthy",
            ]
        );
    }
}
