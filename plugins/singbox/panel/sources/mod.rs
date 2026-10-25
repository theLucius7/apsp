mod adoption;
mod fetch;
mod mihomo;
mod model;
pub mod parse;
mod previews;
mod revisions;
mod service;
mod structured;
mod uri;

pub use model::{FrozenExternalVersion, latest_follow_version_on, load_version_on};
pub(crate) use service::SCHEDULER_LOCK;
pub use service::refresh_due;

use crate::{
    AppState,
    auth::require_admin,
    error::{ApiError, ApiResult},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use model::{CreateSource, Job, NodePreview, PatchSource, Source};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/subscription-sources", get(list).post(create))
        .route(
            "/subscription-sources/{id}",
            get(get_source).patch(update).delete(remove),
        )
        .route("/subscription-sources/{id}/refresh", post(refresh))
        .route("/subscription-sources/{id}/nodes", get(nodes))
        .route(
            "/subscription-sources/{id}/nodes/{node_id}",
            axum::routing::patch(adoption::update),
        )
        .route("/subscription-source-previews", post(previews::create))
        .route(
            "/subscription-source-previews/{id}",
            axum::routing::delete(previews::remove),
        )
        .route(
            "/subscription-source-previews/{id}/commit",
            post(previews::commit),
        )
        .route("/subscription-source-jobs/{id}", get(job))
        .route("/subscription-source-jobs/{id}/cancel", post(cancel))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Vec<Source>>> {
    require_admin(&state, &headers).await?;
    Ok(Json(model::list_on(&state.pool).await?))
}

async fn get_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Source>> {
    require_admin(&state, &headers).await?;
    Ok(Json(model::get_on(&state.pool, id).await?))
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<CreateSource>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<(StatusCode, Json<Source>)> {
    require_admin(&state, &headers).await?;
    super::source_migration::ensure_numbered_writable(&state.pool).await?;
    let Json(request) = request.map_err(|_| bad("来源请求字段或大小不符合要求"))?;
    let name = super::business::name(&request.name)?;
    let interval = refresh_interval(request.refresh_interval_seconds.unwrap_or(86400))?;
    let user_agent = request
        .user_agent
        .as_deref()
        .unwrap_or(fetch::DEFAULT_USER_AGENT);
    fetch::validate_user_agent(user_agent).map_err(import_error)?;
    let host = match request.kind.as_str() {
        "url" if request.content.is_none() => {
            let url = request
                .url
                .as_deref()
                .ok_or_else(|| bad("请填写订阅地址"))?;
            let url = fetch::validate_url(url).map_err(import_error)?;
            if let Some(value) = &request.authorization {
                fetch::validate_authorization(value).map_err(import_error)?;
            }
            Some(url.host_str().expect("validated host").to_string())
        }
        "inline" if request.url.is_none() && request.authorization.is_none() => {
            validate_content(
                request
                    .content
                    .as_deref()
                    .ok_or_else(|| bad("请粘贴或上传配置内容"))?,
            )?;
            None
        }
        _ => return Err(bad("来源类型与内容不匹配")),
    };
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    let id: i64 = sqlx::query_scalar("INSERT INTO singbox_subscription_sources(name,kind,secret_url,secret_authorization,secret_content,source_host,refresh_interval_seconds,created_at,user_agent,auto_refresh) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id")
        .bind(name).bind(request.kind).bind(request.url).bind(request.authorization).bind(request.content).bind(host).bind(interval).bind(apsp_protocol::now_timestamp()).bind(user_agent).bind(request.auto_refresh.unwrap_or(true)).fetch_one(&mut *tx).await?;
    service::queue_on(&mut tx, id).await?;
    tx.commit().await?;
    service::kick(&state);
    Ok((
        StatusCode::CREATED,
        Json(model::get_on(&state.pool, id).await?),
    ))
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    request: Result<Json<PatchSource>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<Json<Source>> {
    require_admin(&state, &headers).await?;
    super::source_migration::ensure_numbered_writable(&state.pool).await?;
    let Json(request) = request.map_err(|_| bad("来源请求字段或大小不符合要求"))?;
    if !(1..=super::business::MAX_SAFE_INTEGER).contains(&request.settings_revision) {
        return Err(bad("来源修订号必须是可精确表示的正整数"));
    }
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    #[derive(sqlx::FromRow)]
    struct Settings {
        name: String,
        kind: String,
        secret_url: Option<String>,
        secret_authorization: Option<String>,
        secret_content: Option<String>,
        settings_revision: i64,
        identity_epoch: i64,
        refresh_interval_seconds: i64,
        auto_refresh: bool,
        user_agent: String,
        archived: bool,
        current_revision_id: Option<i64>,
    }
    let mut settings: Settings = sqlx::query_as("SELECT name,kind,secret_url,secret_authorization,secret_content,settings_revision,identity_epoch,refresh_interval_seconds,auto_refresh,user_agent,archived,current_revision_id FROM singbox_subscription_sources WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
        .bind(id).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    if request.settings_revision != settings.settings_revision {
        return Err(ApiError::Conflict("来源设置已修改，请刷新后重试".into()));
    }
    if request.clear_authorization && request.authorization.is_some() {
        return Err(bad("替换认证与清除认证不能同时提交"));
    }
    let mut identity_changed = request.replace_source;
    let mut content_changed = false;
    if let Some(value) = request.auto_refresh {
        settings.auto_refresh = value;
    }
    if let Some(value) = request.user_agent {
        fetch::validate_user_agent(&value).map_err(import_error)?;
        content_changed |= value != settings.user_agent;
        settings.user_agent = value;
    }
    if let Some(value) = request.name {
        settings.name = super::business::name(&value)?;
    }
    if let Some(value) = request.refresh_interval_seconds {
        settings.refresh_interval_seconds = refresh_interval(value)?;
    }
    if let Some(value) = request.archived {
        settings.archived = value;
    }
    if let Some(value) = request.url {
        if settings.kind != "url" {
            return Err(bad("内容来源不能设置订阅地址"));
        }
        fetch::validate_url(&value).map_err(import_error)?;
        if settings.secret_authorization.is_some()
            && request.authorization.is_none()
            && !request.clear_authorization
            && !settings.secret_url.as_deref().is_some_and(|previous| {
                crate::subscription_fetch::same_stored_origin(previous, &value)
            })
        {
            return Err(bad(
                "更换到其他主机时须重新填写或清除获取认证，已保存的认证不会发往新地址",
            ));
        }
        identity_changed |= settings.secret_url.as_ref() != Some(&value);
        content_changed = true;
        settings.secret_url = Some(value);
    }
    if let Some(value) = request.authorization {
        if settings.kind != "url" {
            return Err(bad("内容来源不能设置获取认证"));
        }
        fetch::validate_authorization(&value).map_err(import_error)?;
        identity_changed |= settings.secret_authorization.as_ref() != Some(&value);
        content_changed = true;
        settings.secret_authorization = Some(value);
    }
    if request.clear_authorization {
        identity_changed |= settings.secret_authorization.is_some();
        settings.secret_authorization = None;
        content_changed = true;
    }
    if let Some(value) = request.content {
        if settings.kind != "inline" {
            return Err(bad("网址来源不能替换粘贴内容"));
        }
        validate_content(&value)?;
        settings.secret_content = Some(value);
        content_changed = true;
    }
    if request.replace_source && !content_changed {
        return Err(bad("更换来源时请填写新地址、认证或内容"));
    }
    if identity_changed {
        settings.identity_epoch = settings
            .identity_epoch
            .checked_add(1)
            .filter(|next| *next <= super::business::MAX_SAFE_INTEGER)
            .ok_or_else(|| ApiError::Conflict("来源身份代次已达上限".into()))?;
    }
    settings.settings_revision = settings
        .settings_revision
        .checked_add(1)
        .filter(|next| *next <= super::business::MAX_SAFE_INTEGER)
        .ok_or_else(|| ApiError::Conflict("来源修订号已达上限".into()))?;
    let source_host = settings
        .secret_url
        .as_deref()
        .map(fetch::validate_url)
        .transpose()
        .map_err(import_error)?
        .and_then(|url| url.host_str().map(str::to_owned));
    let now = apsp_protocol::now_timestamp();
    sqlx::query("UPDATE singbox_subscription_sources SET name=$2,secret_url=$3,secret_authorization=$4,secret_content=$5,source_host=$6,settings_revision=$7,identity_epoch=$8,refresh_interval_seconds=$9,archived=$10,etag=NULL,last_modified=NULL,cache_settings_revision=NULL,cache_identity_epoch=NULL,next_refresh_at=CASE WHEN kind='url' AND NOT $10 AND $12 THEN $11 ELSE NULL END,auto_refresh=$12,user_agent=$13 WHERE id=$1")
        .bind(id).bind(settings.name).bind(settings.secret_url).bind(settings.secret_authorization).bind(settings.secret_content).bind(source_host).bind(settings.settings_revision).bind(settings.identity_epoch).bind(settings.refresh_interval_seconds).bind(settings.archived).bind(now).bind(settings.auto_refresh).bind(settings.user_agent).execute(&mut *tx).await?;
    let superseded = sqlx::query("UPDATE singbox_source_jobs SET state='superseded',phase='finished',finished_at=$2 WHERE source_id=$1 AND state IN ('queued','running')")
        .bind(id).bind(now).execute(&mut *tx).await?.rows_affected() > 0;
    if !settings.archived
        && (content_changed || superseded || settings.current_revision_id.is_none())
    {
        service::queue_on(&mut tx, id).await?;
    }
    tx.commit().await?;
    service::kick(&state);
    Ok(Json(model::get_on(&state.pool, id).await?))
}

async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    require_admin(&state, &headers).await?;
    super::source_migration::ensure_numbered_writable(&state.pool).await?;
    let source = model::get_on(&state.pool, id).await?;
    if source.kind != "url" {
        return Err(bad("内容来源请通过“更新内容”提交新版本"));
    }
    let job = service::queue(&state.pool, id).await?;
    service::kick(&state);
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn nodes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<NodePreview>>> {
    require_admin(&state, &headers).await?;
    Ok(Json(model::nodes_on(&state.pool, id).await?))
}

async fn job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Job>> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        sqlx::query_as(&format!(
            "SELECT {} FROM singbox_source_jobs WHERE id=$1",
            model::JOB_COLUMNS
        ))
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::NotFound)?,
    ))
}

async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Job>> {
    require_admin(&state, &headers).await?;
    super::source_migration::ensure_numbered_writable(&state.pool).await?;
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    let changed = sqlx::query("UPDATE singbox_source_jobs SET state='cancelled',phase='finished',finished_at=$2 WHERE id=$1 AND state IN ('queued','running')")
        .bind(id).bind(apsp_protocol::now_timestamp()).execute(&mut *tx).await?.rows_affected();
    if changed == 1 {
        sqlx::query("UPDATE singbox_subscription_sources s SET last_error='cancelled' FROM singbox_source_jobs j WHERE j.id=$1 AND s.id=j.source_id AND s.settings_revision=j.settings_revision AND s.identity_epoch=j.identity_epoch")
            .bind(id).execute(&mut *tx).await?;
    }
    let result = sqlx::query_as(&format!(
        "SELECT {} FROM singbox_source_jobs WHERE id=$1",
        model::JOB_COLUMNS
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    tx.commit().await?;
    Ok(Json(result))
}

async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_admin(&state, &headers).await?;
    super::source_migration::ensure_numbered_writable(&state.pool).await?;
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM singbox_subscription_sources WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    let dependencies: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT h.chain_id FROM singbox_chain_hops h JOIN singbox_chains c ON c.id=h.chain_id WHERE h.source_id=$1 AND c.deleted_at IS NULL ORDER BY h.chain_id")
        .bind(id).fetch_all(&mut *tx).await?;
    if !dependencies.is_empty() {
        return Err(ApiError::Conflict(format!(
            "来源仍被链路引用（编号：{}）；可先归档来源",
            dependencies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("、")
        )));
    }
    let assigned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_external_accesses a JOIN users u ON u.id=a.user_id WHERE a.source_id=$1 AND u.deleted_at IS NULL)")
        .bind(id).fetch_one(&mut *tx).await?;
    if assigned {
        return Err(ApiError::Conflict(
            "来源节点仍分配给代理用户，请先撤销分配；可先归档来源".into(),
        ));
    }
    let now = apsp_protocol::now_timestamp();
    sqlx::query("UPDATE singbox_subscription_sources SET archived=TRUE,deleted_at=$2,settings_revision=settings_revision+1,etag=NULL,last_modified=NULL,next_refresh_at=NULL WHERE id=$1").bind(id).bind(now).execute(&mut *tx).await?;
    sqlx::query("UPDATE singbox_source_jobs SET state='cancelled',phase='finished',finished_at=$2 WHERE source_id=$1 AND state IN ('queued','running')").bind(id).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn refresh_interval(value: i64) -> ApiResult<i64> {
    if (300..=2_592_000).contains(&value) {
        Ok(value)
    } else {
        Err(bad("刷新周期须为 5 分钟至 30 天"))
    }
}
fn validate_content(value: &str) -> ApiResult<()> {
    if value.trim().is_empty() || value.len() > parse::MAX_BODY {
        Err(bad("配置内容不能为空且不能超过 2 MiB"))
    } else {
        Ok(())
    }
}
fn bad(message: &str) -> ApiError {
    ApiError::BadRequest(message.into())
}
fn import_error(error: parse::ImportError) -> ApiError {
    bad(match error.0 {
        "non_public_source_address" => "订阅地址必须指向公网，不能使用本机、私网或保留地址",
        "invalid_source_authorization" => "获取认证不能为空或包含非法字符",
        "invalid_source_user_agent" => "请求标识须为 1 至 256 字节的可打印 ASCII 文本",
        _ => "请填写无用户名、密码和片段的有效 HTTPS 订阅地址",
    })
}

#[cfg(test)]
mod tests;
