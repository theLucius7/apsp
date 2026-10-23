use crate::error::{ApiError, ApiResult};
use apsp_compiler::external::{ExternalCapabilities, ExternalOutbound};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgConnection, PgPool};
use uuid::Uuid;

#[derive(Serialize, FromRow)]
pub struct Source {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub source_host: Option<String>,
    pub url_configured: bool,
    pub authorization_configured: bool,
    pub content_configured: bool,
    pub settings_revision: i64,
    pub identity_epoch: i64,
    pub refresh_interval_seconds: i64,
    pub auto_refresh: bool,
    pub user_agent: String,
    pub traffic: Value,
    pub changes: Value,
    pub stale: bool,
    pub archived: bool,
    pub current_revision_id: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub supported_count: i32,
    pub unsupported_count: i32,
    pub active_job_id: Option<Uuid>,
    pub dependency_ids: Vec<i64>,
    /// The ordered source this numbered source moved to, once migrated.
    pub migrated_to: Option<i64>,
}

const SOURCE_SELECT: &str = "SELECT s.id,s.name,s.kind,s.source_host,s.secret_url IS NOT NULL AS url_configured,s.secret_authorization IS NOT NULL AS authorization_configured,s.secret_content IS NOT NULL AS content_configured,s.settings_revision,s.identity_epoch,s.refresh_interval_seconds,s.auto_refresh,s.user_agent,s.traffic,s.changes,(s.last_error IS NOT NULL OR COALESCE(r.identity_epoch<>s.identity_epoch,FALSE)) AS stale,s.archived,s.current_revision_id,s.last_attempt_at,s.last_success_at,s.last_error,COALESCE(r.supported_count,0) AS supported_count,COALESCE(r.unsupported_count,0) AS unsupported_count,(SELECT j.id FROM singbox_source_jobs j WHERE j.source_id=s.id AND j.state IN ('queued','running')) AS active_job_id,ARRAY(SELECT DISTINCT h.chain_id FROM singbox_chain_hops h JOIN singbox_chains c ON c.id=h.chain_id WHERE h.source_id=s.id AND c.deleted_at IS NULL ORDER BY h.chain_id) AS dependency_ids,(SELECT m.b_source_id FROM singbox_source_id_map m WHERE m.a_source_id=s.id) AS migrated_to FROM singbox_subscription_sources s LEFT JOIN singbox_source_revisions r ON r.id=s.current_revision_id";

pub(super) async fn list_on(pool: &PgPool) -> ApiResult<Vec<Source>> {
    Ok(sqlx::query_as(&format!(
        "{SOURCE_SELECT} WHERE s.deleted_at IS NULL ORDER BY s.id"
    ))
    .fetch_all(pool)
    .await?)
}

pub(super) async fn get_on(pool: &PgPool, id: i64) -> ApiResult<Source> {
    sqlx::query_as(&format!(
        "{SOURCE_SELECT} WHERE s.id=$1 AND s.deleted_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

#[derive(Clone, Serialize, FromRow)]
pub struct Job {
    pub id: Uuid,
    pub source_id: i64,
    pub settings_revision: i64,
    pub identity_epoch: i64,
    pub state: String,
    pub phase: String,
    pub error_code: Option<String>,
    pub result_revision_id: Option<i64>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

pub(super) const JOB_COLUMNS: &str = "id,source_id,settings_revision,identity_epoch,state,phase,error_code,result_revision_id,created_at,started_at,finished_at";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSource {
    pub name: String,
    pub kind: String,
    pub url: Option<String>,
    pub authorization: Option<String>,
    pub content: Option<String>,
    pub refresh_interval_seconds: Option<i64>,
    pub auto_refresh: Option<bool>,
    pub user_agent: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchSource {
    pub settings_revision: i64,
    pub name: Option<String>,
    pub url: Option<String>,
    pub authorization: Option<String>,
    #[serde(default)]
    pub clear_authorization: bool,
    pub content: Option<String>,
    #[serde(default)]
    pub replace_source: bool,
    pub refresh_interval_seconds: Option<i64>,
    pub auto_refresh: Option<bool>,
    pub user_agent: Option<String>,
    pub archived: Option<bool>,
}

#[derive(Serialize)]
pub struct NodePreview {
    pub id: Option<i64>,
    pub source_id: i64,
    pub node_version_id: Option<i64>,
    pub source_revision_id: Option<i64>,
    pub identity_epoch: i64,
    pub name: String,
    pub protocol: Option<String>,
    pub server: Option<String>,
    pub port: Option<u16>,
    pub transport: Option<String>,
    pub tcp: bool,
    pub udp: bool,
    pub selectable: bool,
    pub present: bool,
    pub identity_unique: bool,
    pub adopted: bool,
    pub metadata_revision: i64,
    pub reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FrozenExternalVersion {
    pub source_id: i64,
    pub external_node_id: i64,
    pub node_version_id: i64,
    pub source_revision_id: i64,
    pub identity_epoch: i64,
    pub name: String,
    pub outbound: ExternalOutbound,
    pub config_sha256: String,
    pub capabilities: ExternalCapabilities,
}

#[derive(FromRow)]
struct VersionRow {
    source_id: i64,
    external_node_id: i64,
    id: i64,
    source_revision_id: i64,
    identity_epoch: i64,
    name: String,
    config_json: Value,
    config_sha256: String,
}

/// Call after acquiring the plugin topology lock. Current identity checks also
/// apply to pinned historical versions being used in a new candidate.
pub async fn load_version_on(
    connection: &mut PgConnection,
    source_id: i64,
    node_id: i64,
    version_id: i64,
    require_current: bool,
) -> ApiResult<FrozenExternalVersion> {
    let row: VersionRow = sqlx::query_as("SELECT v.source_id,v.external_node_id,v.id,v.source_revision_id,v.identity_epoch,v.name,v.config_json,v.config_sha256 FROM singbox_external_node_versions v JOIN singbox_external_nodes n ON n.id=v.external_node_id JOIN singbox_subscription_sources s ON s.id=v.source_id WHERE v.id=$1 AND v.source_id=$2 AND v.external_node_id=$3 AND (NOT $4 OR (s.deleted_at IS NULL AND NOT s.archived AND n.present AND n.identity_unique AND n.identity_epoch=s.identity_epoch AND v.identity_epoch=s.identity_epoch)) FOR SHARE OF s,n")
        .bind(version_id).bind(source_id).bind(node_id).bind(require_current).fetch_optional(&mut *connection).await?.ok_or_else(|| ApiError::Conflict("来源节点已缺失、归档、身份不明确或版本不属于所选节点".into()))?;
    let outbound = ExternalOutbound(row.config_json);
    let capabilities = outbound
        .capabilities()
        .map_err(|_| ApiError::Conflict("来源节点版本无法由当前编译器安全表达".into()))?;
    let actual = super::parse::digest(&serde_json::to_vec(&outbound).map_err(anyhow::Error::from)?);
    if actual != row.config_sha256 {
        return Err(ApiError::Conflict("来源节点版本摘要不匹配".into()));
    }
    Ok(FrozenExternalVersion {
        source_id: row.source_id,
        external_node_id: row.external_node_id,
        node_version_id: row.id,
        source_revision_id: row.source_revision_id,
        identity_epoch: row.identity_epoch,
        name: row.name,
        outbound,
        config_sha256: row.config_sha256,
        capabilities,
    })
}

pub async fn latest_follow_version_on(
    connection: &mut PgConnection,
    source_id: i64,
    node_id: i64,
    identity_epoch: i64,
) -> ApiResult<Option<FrozenExternalVersion>> {
    let version: Option<i64> = sqlx::query_scalar("SELECT n.current_version_id FROM singbox_external_nodes n JOIN singbox_subscription_sources s ON s.id=n.source_id WHERE n.id=$1 AND n.source_id=$2 AND n.identity_epoch=$3 AND s.identity_epoch=$3 AND NOT s.archived AND s.deleted_at IS NULL AND n.present AND n.identity_unique FOR SHARE OF s,n")
        .bind(node_id).bind(source_id).bind(identity_epoch).fetch_optional(&mut *connection).await?.flatten();
    match version {
        Some(id) => Ok(Some(
            load_version_on(connection, source_id, node_id, id, true).await?,
        )),
        None => Ok(None),
    }
}

pub(super) async fn nodes_on(pool: &PgPool, id: i64) -> ApiResult<Vec<NodePreview>> {
    let source = get_on(pool, id).await?;
    #[derive(FromRow)]
    struct PreviewRow {
        id: i64,
        current_version_id: Option<i64>,
        last_seen_revision_id: Option<i64>,
        identity_epoch: i64,
        name: String,
        present: bool,
        identity_unique: bool,
        adopted: bool,
        metadata_revision: i64,
        config_json: Option<Value>,
    }
    let rows: Vec<PreviewRow> = sqlx::query_as("SELECT n.id,n.current_version_id,n.last_seen_revision_id,n.identity_epoch,n.name,n.present,n.identity_unique,n.adopted,COALESCE(m.revision,0) AS metadata_revision,v.config_json FROM singbox_external_nodes n LEFT JOIN singbox_external_node_versions v ON v.id=n.current_version_id LEFT JOIN singbox_node_metadata m ON m.kind='external' AND m.id=n.id WHERE n.source_id=$1 ORDER BY n.identity_epoch DESC,n.id")
        .bind(id).fetch_all(pool).await?;
    let mut previews = Vec::new();
    for row in rows {
        let outbound = row.config_json.map(ExternalOutbound);
        let caps = outbound.as_ref().and_then(|v| v.capabilities().ok());
        let reason = if source.archived {
            Some("source_archived")
        } else if row.identity_epoch != source.identity_epoch {
            Some("source_replaced")
        } else if !row.identity_unique {
            Some("ambiguous_node_identity")
        } else if !row.present {
            Some("node_missing")
        } else if caps.is_none() {
            Some("unsupported_or_invalid_proxy_parameter")
        } else {
            None
        };
        previews.push(NodePreview {
            id: Some(row.id),
            source_id: id,
            node_version_id: row.current_version_id,
            source_revision_id: row.last_seen_revision_id,
            identity_epoch: row.identity_epoch,
            name: row.name,
            protocol: outbound.as_ref().map(|v| v.protocol().into()),
            server: outbound.as_ref().map(|v| v.server().into()),
            port: outbound.as_ref().map(|v| v.port()),
            transport: outbound.as_ref().map(|v| {
                v.0.pointer("/transport/type")
                    .and_then(Value::as_str)
                    .unwrap_or(
                        if caps.as_ref().is_some_and(|caps| {
                            caps.required_transport
                                == apsp_compiler::external::ExternalTransport::Udp
                        }) {
                            "quic"
                        } else {
                            "tcp"
                        },
                    )
                    .into()
            }),
            tcp: caps.as_ref().is_some_and(|v| v.tcp),
            udp: caps.as_ref().is_some_and(|v| v.udp),
            selectable: reason.is_none(),
            present: row.present,
            identity_unique: row.identity_unique,
            adopted: row.adopted,
            metadata_revision: row.metadata_revision,
            reason: reason.map(str::to_owned),
        });
    }
    if let Some(revision) = source.current_revision_id {
        let rejected: Value =
            sqlx::query_scalar("SELECT rejected_nodes FROM singbox_source_revisions WHERE id=$1")
                .bind(revision)
                .fetch_one(pool)
                .await?;
        for node in rejected.as_array().into_iter().flatten() {
            previews.push(NodePreview {
                id: None,
                source_id: id,
                node_version_id: None,
                source_revision_id: Some(revision),
                identity_epoch: source.identity_epoch,
                name: node["name"].as_str().unwrap_or("不支持的节点").into(),
                protocol: None,
                server: None,
                port: None,
                transport: None,
                tcp: false,
                udp: false,
                selectable: false,
                present: true,
                identity_unique: false,
                adopted: false,
                metadata_revision: 0,
                reason: Some(
                    node["reason"]
                        .as_str()
                        .unwrap_or("unsupported_or_invalid_proxy_parameter")
                        .into(),
                ),
            });
        }
    }
    Ok(previews)
}
