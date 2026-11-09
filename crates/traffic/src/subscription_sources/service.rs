use super::models::*;
use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde_json::Value;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

pub(super) async fn load_source(
    connection: &mut PgConnection,
    id: i64,
    lock: bool,
) -> ApiResult<SourceRow> {
    sqlx::query_as::<_, SourceRow>(&format!("SELECT {SOURCE_COLUMNS} FROM singbox_ordered_subscription_sources WHERE id=$1 AND deleted_at IS NULL{}", if lock { " FOR UPDATE" } else { "" }))
        .bind(id).fetch_optional(connection).await?.ok_or(ApiError::NotFound)
}

async fn snapshot(state: &AppState) -> ApiResult<Transaction<'_, Postgres>> {
    Ok(crate::read_snapshot(&state.pool).await?)
}

pub(super) async fn revision(connection: &mut PgConnection, id: Uuid) -> ApiResult<RevisionView> {
    sqlx::query_as::<_, RevisionView>(&format!(
        "SELECT {REVISION_COLUMNS} FROM singbox_subscription_source_revisions WHERE id=$1"
    ))
    .bind(id)
    .fetch_optional(connection)
    .await?
    .ok_or(ApiError::NotFound)
}

pub(super) async fn active_job(
    connection: &mut PgConnection,
    source: i64,
) -> ApiResult<Option<JobView>> {
    Ok(sqlx::query_as::<_, JobView>(&format!("SELECT {JOB_COLUMNS} FROM singbox_subscription_source_jobs WHERE source_id=$1 AND status IN ('queued','running','cancelling')"))
        .bind(source).fetch_optional(connection).await?)
}

async fn view(connection: &mut PgConnection, row: SourceRow) -> ApiResult<SourceView> {
    let latest_success = match row.current_success_revision {
        Some(id) => Some(revision(connection, id).await?),
        None => None,
    };
    let active_job = active_job(connection, row.id).await?;
    let last_error = row
        .last_error
        .map(serde_json::from_value)
        .transpose()
        .map_err(anyhow::Error::from)?;
    let stale_reason = if latest_success
        .as_ref()
        .is_some_and(|revision| revision.identity_epoch != row.identity_epoch)
    {
        Some("来源已更换，显示原来源的历史结果；新来源需要重新解析及选点".into())
    } else if latest_success.is_some() && last_error.is_some() {
        Some("更新失败，当前显示上次成功解析的结果；缓存不代表节点网络健康".into())
    } else {
        None
    };
    let counts = latest_success
        .as_ref()
        .map(|revision| revision.counts.clone())
        .unwrap_or_default();
    Ok(SourceView {
        id: row.id,
        name: row.name,
        kind: row.kind,
        host: row.host,
        configured: row.input_config.get("kind").is_some(),
        auth_configured: row
            .input_config
            .get("auth_headers")
            .and_then(Value::as_object)
            .is_some_and(|headers| !headers.is_empty()),
        settings_revision: row.settings_revision,
        identity_epoch: row.identity_epoch,
        archived: row.archived,
        refresh_interval_secs: row.refresh_interval_secs,
        user_agent: row.user_agent,
        auto_refresh: row.auto_refresh,
        traffic: row.traffic,
        changes: row.changes,
        last_attempt_at: row.last_attempt_at,
        last_success_at: row.last_success_at,
        latest_success,
        active_job,
        last_error,
        stale_reason,
        counts,
        dependencies: super::super::ordered_paths::source_dependencies(connection, row.id)
            .await?
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(anyhow::Error::from)?,
    })
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<SourceView>>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = snapshot(&state).await?;
    let rows = sqlx::query_as::<_, SourceRow>(&format!("SELECT {SOURCE_COLUMNS} FROM singbox_ordered_subscription_sources WHERE deleted_at IS NULL ORDER BY id LIMIT 128"))
        .fetch_all(&mut *tx).await?;
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        views.push(view(&mut tx, row).await?);
    }
    tx.commit().await?;
    Ok(Json(views))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<SourceView>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = snapshot(&state).await?;
    let source = load_source(&mut tx, id, false).await?;
    let result = view(&mut tx, source).await?;
    tx.commit().await?;
    Ok(Json(result))
}

#[derive(FromRow)]
struct NodeRow {
    id: Uuid,
    public_id: i64,
    adopted: bool,
    metadata_revision: i64,
    source_id: i64,
    identity_epoch: i64,
    version_id: Uuid,
    source_revision_id: Uuid,
    present_in_latest: bool,
    identity_state: String,
    supported: bool,
    reasons: Value,
    capabilities: Value,
    public_preview: Value,
}

pub(super) async fn node_page(
    connection: &mut PgConnection,
    source: SourceRow,
    history: Option<Uuid>,
) -> ApiResult<NodePage> {
    let success_revision = match history.or(source.current_success_revision) {
        Some(id) => {
            let result = revision(connection, id).await?;
            if result.source_id != source.id {
                return Err(ApiError::NotFound);
            }
            Some(result)
        }
        None => None,
    };
    let rows = if let Some(revision) = history {
        sqlx::query_as::<_, NodeRow>("SELECT n.id,n.public_id,n.adopted,COALESCE(c.revision,0) AS metadata_revision,n.source_id,n.identity_epoch,v.id AS version_id,v.source_revision_id,TRUE AS present_in_latest,m.identity_state,v.supported,v.reasons,v.capabilities,m.public_preview FROM singbox_subscription_revision_nodes m JOIN singbox_ordered_external_nodes n ON n.id=m.node_id JOIN singbox_ordered_external_node_versions v ON v.id=m.version_id LEFT JOIN singbox_node_metadata c ON c.kind='external' AND c.id=n.public_id WHERE m.source_revision_id=$1 ORDER BY m.ordinal LIMIT 5000")
            .bind(revision).fetch_all(&mut *connection).await?
    } else {
        sqlx::query_as::<_, NodeRow>("SELECT n.id,n.public_id,n.adopted,COALESCE(c.revision,0) AS metadata_revision,n.source_id,n.identity_epoch,v.id AS version_id,v.source_revision_id,COALESCE(n.last_seen_revision=$3,FALSE) AS present_in_latest,n.identity_state,v.supported,v.reasons,v.capabilities,m.public_preview FROM singbox_ordered_external_nodes n JOIN singbox_ordered_external_node_versions v ON v.id=n.latest_version JOIN singbox_subscription_revision_nodes m ON m.node_id=n.id AND m.source_revision_id=n.last_seen_revision LEFT JOIN singbox_node_metadata c ON c.kind='external' AND c.id=n.public_id WHERE n.source_id=$1 AND n.identity_epoch=$2 AND (n.identity_state='unique' OR n.last_seen_revision=$3) ORDER BY COALESCE(n.last_seen_revision=$3,FALSE) DESC,n.created_at,n.id LIMIT 5000")
            .bind(source.id).bind(source.identity_epoch).bind(source.current_success_revision).fetch_all(&mut *connection).await?
    };
    let mut nodes = Vec::with_capacity(rows.len());
    for row in rows {
        let mut reasons: Vec<String> =
            serde_json::from_value(row.reasons).map_err(anyhow::Error::from)?;
        if history.is_some() {
            reasons.push("历史批次仅供查看，不能直接用于新路径".into());
        }
        if source.archived {
            reasons.push("来源已归档，禁止新增引用".into());
        }
        if row.identity_epoch != source.identity_epoch {
            reasons.push("来源身份已更换，需要重新选点".into());
        }
        if !row.present_in_latest {
            reasons.push("此节点在最近成功批次中缺失，保留历史版本".into());
        }
        if row.identity_state == "ambiguous" {
            reasons.push("节点身份不唯一，不能自动匹配已有绑定".into());
        }
        if row.identity_state == "unresolved" {
            reasons.push("尚未建立可校验的节点身份，不能新增引用".into());
        }
        let selectable = history.is_none()
            && !source.archived
            && row.identity_epoch == source.identity_epoch
            && row.present_in_latest
            && row.identity_state == "unique"
            && row.supported;
        nodes.push(NodeView {
            id: row.id,
            public_id: row.public_id,
            adopted: row.adopted,
            metadata_revision: row.metadata_revision,
            source_id: row.source_id,
            identity_epoch: row.identity_epoch,
            version_id: row.version_id,
            source_revision_id: row.source_revision_id,
            present_in_latest: row.present_in_latest,
            identity_state: row.identity_state,
            supported: row.supported,
            selectable,
            reasons,
            capabilities: row.capabilities,
            preview: row.public_preview,
        });
    }
    Ok(NodePage {
        source_id: source.id,
        current_settings_revision: source.settings_revision,
        current_identity_epoch: source.identity_epoch,
        success_revision,
        nodes,
    })
}

pub async fn nodes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<NodePage>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = snapshot(&state).await?;
    let source = load_source(&mut tx, id, false).await?;
    let page = node_page(&mut tx, source, None).await?;
    tx.commit().await?;
    Ok(Json(page))
}

pub async fn historical_nodes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, batch)): Path<(i64, Uuid)>,
) -> ApiResult<Json<NodePage>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = snapshot(&state).await?;
    let source = load_source(&mut tx, id, false).await?;
    let page = node_page(&mut tx, source, Some(batch)).await?;
    tx.commit().await?;
    Ok(Json(page))
}

pub async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<HistoryPage>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = snapshot(&state).await?;
    load_source(&mut tx, id, false).await?;
    let revisions = sqlx::query_as::<_, RevisionView>(&format!("SELECT {REVISION_COLUMNS} FROM singbox_subscription_source_revisions WHERE source_id=$1 ORDER BY generation DESC LIMIT 100"))
        .bind(id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(HistoryPage {
        source_id: id,
        revisions,
    }))
}
