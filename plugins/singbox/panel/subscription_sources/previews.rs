//! Preview a source before saving it, then keep only the selected nodes adopted.
//!
//! A preview stores the fetched body for ten minutes. Committing re-parses
//! that exact body, so the saved revision is the one the administrator saw.

use super::{
    fetch::{self, FetchConfig, FetchOutcome},
    models::*,
    mutations, snapshots,
};
use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
    subscription_parser::{
        self, FormatHint, MAX_NODES, NodePreview, PARSER_VERSION, ParseStatus, ParsedSubscription,
        SubscriptionFormat,
    },
};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use std::{
    collections::BTreeSet,
    sync::{Arc, OnceLock},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

const PREVIEW_SECS: i64 = 600;
const PREVIEWS_PER_ADMIN: i64 = 8;
const PARSE_PERMITS: usize = 2;

static PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn admit() -> ApiResult<OwnedSemaphorePermit> {
    PERMITS
        .get_or_init(|| Arc::new(Semaphore::new(PARSE_PERMITS)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::Conflict("正在处理其他来源预览，请稍后重试".into()))
}

/// Parses on a blocking thread that owns the permit, so a cancelled request
/// still counts against the limit until parsing actually ends.
async fn parse(
    body: Vec<u8>,
    permit: OwnedSemaphorePermit,
) -> ApiResult<(Vec<u8>, ParsedSubscription)> {
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let parsed = subscription_parser::parse_subscription(&body, FormatHint::Auto);
        (body, parsed)
    })
    .await
    .map_err(|_| ApiError::Conflict("来源解析中断，请重新预览".into()))
    .and_then(|(body, parsed)| {
        parsed
            .map(|parsed| (body, parsed))
            .map_err(|error| ApiError::BadRequest(format!("解析来源失败：{}", error.message)))
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePreview {
    pub input: SourceInput,
    pub user_agent: Option<String>,
}

#[derive(Serialize)]
pub struct PreviewNodeView {
    pub key: String,
    pub selectable: bool,
    pub identity_state: &'static str,
    pub capabilities: Value,
    #[serde(flatten)]
    pub preview: NodePreview,
}

#[derive(Serialize)]
pub struct PreviewView {
    pub id: Uuid,
    pub expires_at: i64,
    pub format: SubscriptionFormat,
    pub supported_count: usize,
    pub unsupported_count: usize,
    pub warnings: Vec<subscription_parser::ParseReason>,
    pub nodes: Vec<PreviewNodeView>,
}

fn key(ordinal: usize) -> String {
    format!("node-{ordinal}")
}

fn projection(id: Uuid, expires_at: i64, parsed: &ParsedSubscription) -> PreviewView {
    let keys = snapshots::identity_keys(parsed);
    let nodes = parsed
        .nodes
        .iter()
        .zip(&keys)
        .map(|(node, identity)| {
            let supported = node.preview.parse_status == ParseStatus::Supported;
            PreviewNodeView {
                key: key(node.preview.ordinal),
                selectable: supported && identity.is_some(),
                identity_state: if identity.is_some() {
                    "unique"
                } else if node.identity_fingerprint.is_some() || node.provider_metadata_id.is_some()
                {
                    "ambiguous"
                } else {
                    "unresolved"
                },
                capabilities: node
                    .outbound
                    .as_ref()
                    .map(|outbound| json!({"tcp":outbound.tcp(),"udp":outbound.udp()}))
                    .unwrap_or_else(|| json!({"tcp":false,"udp":false})),
                preview: node.preview.clone(),
            }
        })
        .collect();
    PreviewView {
        id,
        expires_at,
        format: parsed.format,
        supported_count: parsed.supported_count,
        unsupported_count: parsed.unsupported_count,
        warnings: parsed.warnings.clone(),
        nodes,
    }
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    ProtectedJson(mut input): ProtectedJson<CreatePreview>,
) -> ApiResult<(StatusCode, Json<PreviewView>)> {
    let admin_id = auth::require_admin(&state, &headers).await?;
    mutations::normalize_input(&mut input.input)?;
    if let Some(value) = &input.user_agent {
        mutations::user_agent(value)?;
    }
    let permit = admit()?;
    let (body, traffic) = match &input.input {
        SourceInput::Inline { content } => (content.as_bytes().to_vec(), None),
        SourceInput::Url { url, auth_headers } => {
            let config = FetchConfig {
                url: url.clone(),
                auth_headers: auth_headers.clone(),
                etag: None,
                last_modified: None,
                user_agent: input.user_agent.clone(),
            };
            match fetch::fetch(&config).await {
                Ok(FetchOutcome::Modified { body, traffic, .. }) => (body, traffic),
                Ok(FetchOutcome::NotModified { .. }) => {
                    return Err(ApiError::BadRequest("来源没有返回配置内容".into()));
                }
                Err(error) => {
                    return Err(ApiError::BadRequest(format!(
                        "获取来源失败：{}",
                        error.message
                    )));
                }
            }
        }
    };
    let (body, parsed) = parse(body, permit).await?;
    let now = now_timestamp();
    let id = Uuid::new_v4();
    let expires_at = now + PREVIEW_SECS;
    let view = projection(id, expires_at, &parsed);
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(73402904,1)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM singbox_ordered_source_previews WHERE expires_at<=$1")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM singbox_ordered_source_previews WHERE admin_id=$1",
    )
    .bind(admin_id)
    .fetch_one(&mut *tx)
    .await?;
    if count >= PREVIEWS_PER_ADMIN {
        return Err(ApiError::Conflict(
            "最多保留 8 份待确认预览，请先保存或等待预览过期".into(),
        ));
    }
    sqlx::query("INSERT INTO singbox_ordered_source_previews(id,admin_id,input_config,body,user_agent,parser_version,traffic,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(id).bind(admin_id).bind(json!(input.input)).bind(body).bind(&input.user_agent).bind(PARSER_VERSION).bind(traffic.unwrap_or_else(|| json!({}))).bind(now).bind(expires_at).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(view)))
}

pub async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let admin_id = auth::require_admin(&state, &headers).await?;
    sqlx::query("DELETE FROM singbox_ordered_source_previews WHERE id=$1 AND admin_id=$2")
        .bind(id)
        .bind(admin_id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommitPreview {
    pub request_id: Uuid,
    pub name: String,
    pub selected: Vec<String>,
    pub refresh_interval_secs: Option<i64>,
    pub auto_refresh: Option<bool>,
}

#[derive(Clone, PartialEq, FromRow)]
struct StoredPreview {
    input_config: Value,
    body: Vec<u8>,
    user_agent: Option<String>,
    parser_version: String,
    traffic: Value,
    created_at: i64,
    expires_at: i64,
}

const PREVIEW_SELECT: &str = "SELECT input_config,body,user_agent,parser_version,traffic,created_at,expires_at FROM singbox_ordered_source_previews WHERE id=$1 AND admin_id=$2";

impl StoredPreview {
    fn current(&self, now: i64) -> bool {
        self.expires_at > now && self.parser_version == PARSER_VERSION
    }
}

pub async fn commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ProtectedJson(mut input): ProtectedJson<CommitPreview>,
) -> ApiResult<(StatusCode, Json<MutationReceipt>)> {
    let admin_id = auth::require_admin(&state, &headers).await?;
    input.name = mutations::source_name(&input.name)?;
    let selected: BTreeSet<&str> = input.selected.iter().map(String::as_str).collect();
    if selected.is_empty() || input.selected.len() > MAX_NODES {
        return Err(ApiError::BadRequest(
            "请选择 1 至 5000 个可采用的节点".into(),
        ));
    }
    if selected.len() != input.selected.len() {
        return Err(ApiError::BadRequest("不能重复选择同一个节点".into()));
    }
    let hash = mutations::digest(&json!({"operation":"preview_commit","preview":id,"body":input}))?;
    // A replayed commit returns its receipt even though the preview is gone.
    let mut tx = state.pool.begin().await?;
    if let Some(previous) = mutations::receipt(&mut tx, input.request_id, &hash).await? {
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(previous)));
    }
    tx.rollback().await?;
    let missing = || ApiError::Conflict("预览已保存或不存在，请重新解析".into());
    let captured: StoredPreview = sqlx::query_as(PREVIEW_SELECT)
        .bind(id)
        .bind(admin_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(missing)?;
    if !captured.current(now_timestamp()) {
        return Err(ApiError::Conflict(
            "预览已过期或解析器已更新，请重新解析".into(),
        ));
    }
    let source_input: SourceInput =
        serde_json::from_value(captured.input_config.clone()).map_err(anyhow::Error::from)?;
    let refresh_interval_secs = mutations::interval(&source_input, input.refresh_interval_secs)?;
    let auto_refresh = input.auto_refresh.unwrap_or(true);
    // Parsing never holds a transaction; the captured row is compared again below.
    let (_, parsed) = parse(captured.body.clone(), admit()?).await?;
    let keys = snapshots::identity_keys(&parsed);
    let chosen: Vec<usize> = parsed
        .nodes
        .iter()
        .zip(&keys)
        .enumerate()
        .filter(|(_, (node, identity))| {
            selected.contains(key(node.preview.ordinal).as_str())
                && identity.is_some()
                && node.preview.parse_status == ParseStatus::Supported
        })
        .map(|(index, _)| index)
        .collect();
    if chosen.len() != selected.len() {
        return Err(ApiError::BadRequest(
            "所选节点不属于此预览，或不受支持、身份不唯一".into(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(previous) = mutations::receipt(&mut tx, input.request_id, &hash).await? {
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(previous)));
    }
    let stored: StoredPreview = sqlx::query_as(&format!("{PREVIEW_SELECT} FOR UPDATE"))
        .bind(id)
        .bind(admin_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    let now = now_timestamp();
    if stored != captured || !stored.current(now) {
        return Err(ApiError::Conflict(
            "预览已变化、过期或解析器已更新，请重新解析".into(),
        ));
    }
    sqlx::query("SELECT pg_advisory_xact_lock(73402902,1)")
        .execute(&mut *tx)
        .await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM singbox_ordered_subscription_sources WHERE deleted_at IS NULL",
    )
    .fetch_one(&mut *tx)
    .await?;
    if count >= 128 {
        return Err(ApiError::Conflict(
            "最多保留 128 个未删除的订阅来源，请先整理来源".into(),
        ));
    }
    let (kind, host) = match &source_input {
        SourceInput::Url { url, .. } => (
            "url",
            fetch::validate_url(url)
                .map_err(|error| ApiError::BadRequest(error.message))?
                .host_str()
                .map(str::to_owned),
        ),
        SourceInput::Inline { .. } => ("inline", None),
    };
    let next_refresh_at = (kind == "url" && auto_refresh).then_some(now + refresh_interval_secs);
    let source_id: i64 = sqlx::query_scalar("INSERT INTO singbox_ordered_subscription_sources(name,kind,host,input_config,refresh_interval_secs,next_refresh_at,last_attempt_at,last_success_at,created_at,updated_at,user_agent,auto_refresh) VALUES($1,$2,$3,$4,$5,$6,$7,$7,$7,$7,$8,$9) RETURNING id")
        .bind(&input.name).bind(kind).bind(host).bind(&stored.input_config).bind(refresh_interval_secs).bind(next_refresh_at).bind(now).bind(&stored.user_agent).bind(auto_refresh).fetch_one(&mut *tx).await?;
    // Every revision belongs to a job; a committed preview records a finished one.
    let job_id = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_subscription_source_jobs(id,source_id,settings_revision,identity_epoch,parser_version,status,stage,created_at,started_at,finished_at) VALUES($1,$2,1,1,$3,'succeeded','done',$4,$4,$4)")
        .bind(job_id).bind(source_id).bind(PARSER_VERSION).bind(now).execute(&mut *tx).await?;
    let target = snapshots::Target {
        source_id,
        job_id,
        settings_revision: 1,
        identity_epoch: 1,
        parser_version: PARSER_VERSION,
    };
    let persisted = snapshots::persist(&mut tx, &target, parsed, now).await?;
    sqlx::query("UPDATE singbox_subscription_source_jobs SET source_revision_id=$2 WHERE id=$1")
        .bind(job_id)
        .bind(persisted.revision)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE singbox_ordered_subscription_sources SET current_success_revision=$2 WHERE id=$1",
    )
    .bind(source_id)
    .bind(persisted.revision)
    .execute(&mut *tx)
    .await?;
    let adopted: Vec<Uuid> = chosen.iter().map(|index| persisted.nodes[*index]).collect();
    sqlx::query("UPDATE singbox_ordered_external_nodes SET adopted=TRUE WHERE id=ANY($1)")
        .bind(&adopted)
        .execute(&mut *tx)
        .await?;
    let traffic = stored
        .traffic
        .as_object()
        .is_some_and(|fields| !fields.is_empty())
        .then(|| stored.traffic.clone());
    snapshots::save_traffic(&mut tx, source_id, traffic, stored.created_at).await?;
    sqlx::query("DELETE FROM singbox_ordered_source_previews WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let receipt = MutationReceipt {
        source_id,
        settings_revision: 1,
        identity_epoch: 1,
        job_id: Some(job_id),
    };
    mutations::save_receipt(&mut tx, input.request_id, "preview_commit", &hash, &receipt).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(receipt)))
}

#[derive(FromRow)]
struct AdoptionRow {
    public_id: i64,
    adopted: bool,
    metadata_revision: i64,
    metadata_deleted: bool,
    identity_epoch: i64,
    identity_state: String,
    latest_version: Option<Uuid>,
    last_seen_revision: Option<Uuid>,
    supported: Option<bool>,
}

/// Adopting puts a node into the catalog; it never changes chain selection.
/// Adoption and catalog removal share the catalog metadata revision, like
/// numbered sources, so an old page cannot revive a removed node.
pub async fn adopt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((source_id, node_id)): Path<(i64, Uuid)>,
    ProtectedJson(input): ProtectedJson<AdoptNode>,
) -> ApiResult<Json<NodeView>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    let source = super::service::load_source(&mut tx, source_id, true).await?;
    let AdoptionRow {
        public_id,
        adopted,
        metadata_revision,
        metadata_deleted,
        identity_epoch: epoch,
        identity_state,
        latest_version,
        last_seen_revision: last_seen,
        supported,
    } = sqlx::query_as("SELECT n.public_id,n.adopted,COALESCE(m.revision,0) AS metadata_revision,m.deleted_at IS NOT NULL AS metadata_deleted,n.identity_epoch,n.identity_state,n.latest_version,n.last_seen_revision,v.supported FROM singbox_ordered_external_nodes n LEFT JOIN singbox_ordered_external_node_versions v ON v.id=n.latest_version LEFT JOIN singbox_node_metadata m ON m.kind='external' AND m.id=n.public_id WHERE n.id=$1 AND n.source_id=$2 FOR UPDATE OF n")
        .bind(node_id).bind(source_id).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    if source.settings_revision != input.settings_revision
        || epoch != input.identity_epoch
        || latest_version != Some(input.node_version_id)
        || metadata_revision != input.metadata_revision
    {
        return Err(ApiError::Conflict(
            "来源、节点版本或管理设置已变化，请刷新后重新确认采用状态".into(),
        ));
    }
    if input.adopted
        && (source.archived
            || epoch != source.identity_epoch
            || identity_state != "unique"
            || last_seen.is_none()
            || last_seen != source.current_success_revision
            || supported != Some(true))
    {
        return Err(ApiError::Conflict(
            "此节点当前不可采用：须属于未归档来源的最近成功批次，且身份唯一、受支持".into(),
        ));
    }
    if !input.adopted {
        super::super::catalog::ensure_external_removable(&mut tx, public_id).await?;
    }
    if adopted != input.adopted || (input.adopted && metadata_deleted) {
        let next = metadata_revision
            .checked_add(1)
            .filter(|next| *next <= super::super::business::MAX_SAFE_INTEGER)
            .ok_or_else(|| ApiError::Conflict("节点管理修订号已达上限".into()))?;
        sqlx::query("INSERT INTO singbox_node_metadata(kind,id,revision) VALUES('external',$1,$2) ON CONFLICT(kind,id) DO UPDATE SET revision=$2,deleted_at=CASE WHEN $3 THEN NULL ELSE singbox_node_metadata.deleted_at END")
            .bind(public_id).bind(next).bind(input.adopted).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE singbox_ordered_external_nodes SET adopted=$2 WHERE id=$1")
        .bind(node_id)
        .bind(input.adopted)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut connection = state.pool.acquire().await?;
    let source = super::service::load_source(&mut connection, source_id, false).await?;
    super::service::node_page(&mut connection, source, None)
        .await?
        .nodes
        .into_iter()
        .find(|node| node.id == node_id)
        .map(Json)
        .ok_or(ApiError::NotFound)
}
