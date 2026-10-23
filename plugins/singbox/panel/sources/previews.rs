use super::{bad, fetch, model, parse, refresh_interval, revisions, validate_content};
use crate::{
    AppState,
    auth::require_admin,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreatePreview {
    kind: String,
    url: Option<String>,
    authorization: Option<String>,
    content: Option<String>,
    user_agent: Option<String>,
    name: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Preview {
    id: Uuid,
    expires_at: i64,
    format: String,
    supported_count: usize,
    unsupported_count: usize,
    nodes: Vec<PreviewNode>,
}

#[derive(Serialize)]
struct PreviewNode {
    key: String,
    index: usize,
    name: String,
    protocol: Option<String>,
    server: Option<String>,
    port: Option<u16>,
    transport: Option<String>,
    tcp: bool,
    udp: bool,
    supported: bool,
    reason: Option<String>,
}

fn projection(id: Uuid, expires_at: i64, batch: &parse::ParsedBatch) -> Preview {
    let mut nodes = Vec::new();
    for (index, node) in batch.nodes.iter().enumerate() {
        let capabilities = node
            .outbound
            .capabilities()
            .expect("validated parser result");
        nodes.push(PreviewNode {
            key: format!("node-{index}"),
            index,
            name: node.name.clone(),
            protocol: Some(node.outbound.protocol().into()),
            server: Some(node.outbound.server().into()),
            port: Some(node.outbound.port()),
            transport: Some(
                node.outbound
                    .0
                    .pointer("/transport/type")
                    .and_then(Value::as_str)
                    .unwrap_or(
                        if capabilities.required_transport
                            == apsp_compiler::external::ExternalTransport::Udp
                        {
                            "quic"
                        } else {
                            "tcp"
                        },
                    )
                    .into(),
            ),
            tcp: capabilities.tcp,
            udp: capabilities.udp,
            supported: true,
            reason: None,
        });
    }
    for (index, node) in batch.rejected.iter().enumerate() {
        nodes.push(PreviewNode {
            key: format!("rejected-{index}"),
            index: node.index,
            name: node.name.clone(),
            protocol: None,
            server: None,
            port: None,
            transport: None,
            tcp: false,
            udp: false,
            supported: false,
            reason: Some(node.reason.clone()),
        });
    }
    Preview {
        id,
        expires_at,
        format: batch.format.into(),
        supported_count: batch.nodes.len(),
        unsupported_count: batch.rejected.len(),
        nodes,
    }
}

pub(super) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<CreatePreview>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<(StatusCode, Json<Preview>)> {
    let admin_id = require_admin(&state, &headers).await?;
    super::super::source_migration::ensure_numbered_writable(&state.pool).await?;
    let Json(request) = request.map_err(|_| bad("预览请求字段或大小不符合要求"))?;
    let permit = parse::try_admit()
        .map_err(|_| ApiError::Conflict("正在处理其他导入预览，请稍后重试".into()))?;
    if let Some(name) = &request.name {
        super::super::business::name(name)?;
    }
    let user_agent = request
        .user_agent
        .as_deref()
        .unwrap_or(fetch::DEFAULT_USER_AGENT);
    fetch::validate_user_agent(user_agent).map_err(super::import_error)?;
    let (body, traffic) = match request.kind.as_str() {
        "inline" if request.url.is_none() && request.authorization.is_none() => {
            let value = request
                .content
                .as_deref()
                .ok_or_else(|| bad("请粘贴或上传配置内容"))?;
            validate_content(value)?;
            (value.as_bytes().to_vec(), None)
        }
        "url" if request.content.is_none() => {
            let url = request
                .url
                .as_deref()
                .ok_or_else(|| bad("请填写订阅地址"))?;
            fetch::validate_url(url).map_err(super::import_error)?;
            if let Some(value) = &request.authorization {
                fetch::validate_authorization(value).map_err(super::import_error)?;
            }
            let fetched = fetch::download(
                url,
                request.authorization.as_deref(),
                None,
                None,
                user_agent,
            )
            .await
            .map_err(|error| ApiError::BadRequest(format!("获取来源失败：{}", error.0)))?;
            (
                fetched.body.ok_or_else(|| bad("来源没有返回配置内容"))?,
                fetched.traffic,
            )
        }
        _ => return Err(bad("来源类型与内容不匹配")),
    };
    // Keep the permit inside the blocking task if the HTTP request is cancelled.
    // Otherwise repeated cancellations could evade the parser concurrency limit.
    let (body, _body_sha256, batch, _permit) = parse::admitted_parse(body, permit)
        .await
        .map_err(|error| ApiError::BadRequest(format!("解析来源失败：{}", error.0)))?;
    let now = apsp_protocol::now_timestamp();
    let expires_at = now + 600;
    let id = Uuid::new_v4();
    let response = projection(id, expires_at, &batch);
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    sqlx::query("DELETE FROM singbox_source_previews WHERE expires_at <= $1")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM singbox_source_previews WHERE admin_id=$1")
            .bind(admin_id)
            .fetch_one(&mut *tx)
            .await?;
    if count >= 8 {
        return Err(ApiError::Conflict(
            "最多保留 8 份待确认预览，请先保存或等待预览过期".into(),
        ));
    }
    sqlx::query("INSERT INTO singbox_source_previews(id,admin_id,kind,secret_url,secret_authorization,secret_body,user_agent,parser_version,traffic,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
        .bind(id).bind(admin_id).bind(request.kind).bind(request.url).bind(request.authorization).bind(body).bind(user_agent)
        .bind(parse::PARSER_VERSION).bind(traffic.unwrap_or_else(|| serde_json::json!({}))).bind(now).bind(expires_at).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(response)))
}

pub(super) async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let admin_id = require_admin(&state, &headers).await?;
    sqlx::query("DELETE FROM singbox_source_previews WHERE id=$1 AND admin_id=$2")
        .bind(id)
        .bind(admin_id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommitPreview {
    name: String,
    selected: Vec<String>,
    refresh_interval_seconds: Option<i64>,
    auto_refresh: Option<bool>,
}

#[derive(Clone, PartialEq, FromRow)]
struct StoredPreview {
    kind: String,
    secret_url: Option<String>,
    secret_authorization: Option<String>,
    secret_body: Vec<u8>,
    user_agent: String,
    parser_version: String,
    created_at: i64,
    traffic: Value,
    expires_at: i64,
}

impl StoredPreview {
    fn matches_capture(&self, captured: &Self, now: i64) -> bool {
        self == captured && self.expires_at > now && self.parser_version == parse::PARSER_VERSION
    }
}

const PREVIEW_SELECT: &str = "SELECT kind,secret_url,secret_authorization,secret_body,user_agent,parser_version,created_at,traffic,expires_at FROM singbox_source_previews WHERE id=$1 AND admin_id=$2";

pub(super) async fn commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    request: Result<Json<CommitPreview>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<(StatusCode, Json<model::Source>)> {
    let admin_id = require_admin(&state, &headers).await?;
    super::super::source_migration::ensure_numbered_writable(&state.pool).await?;
    let Json(request) = request.map_err(|_| bad("保存预览请求字段不符合要求"))?;
    let name = super::super::business::name(&request.name)?;
    let interval = refresh_interval(request.refresh_interval_seconds.unwrap_or(86400))?;
    if request.selected.is_empty() || request.selected.len() > parse::MAX_NODES {
        return Err(bad("请选择 1 至 5000 个支持的节点"));
    }
    let permit = parse::try_admit()
        .map_err(|_| ApiError::Conflict("正在处理其他来源解析，请稍后重试".into()))?;
    let captured: StoredPreview = sqlx::query_as(PREVIEW_SELECT)
        .bind(id)
        .bind(admin_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::Conflict("预览已保存或不存在，请重新解析".into()))?;
    let now = apsp_protocol::now_timestamp();
    if !captured.matches_capture(&captured, now) {
        return Err(ApiError::Conflict(
            "预览已过期或解析器已更新，请重新解析".into(),
        ));
    }
    let (body, body_sha256, batch, _permit) =
        parse::admitted_parse(captured.secret_body.clone(), permit)
            .await
            .map_err(|_| bad("预览内容无法解析，请重新创建"))?;
    let selected: BTreeSet<_> = request.selected.iter().cloned().collect();
    if selected.len() != request.selected.len() {
        return Err(bad("不能重复选择同一个节点"));
    }
    let mut keys = Vec::new();
    for (index, node) in batch.nodes.iter().enumerate() {
        if selected.contains(&format!("node-{index}")) {
            keys.push(node.identity_key.clone());
        }
    }
    if keys.len() != selected.len() {
        return Err(bad("所选节点不属于此预览或不受支持"));
    }
    // Parsing never holds the shared topology lock. Recheck every captured
    // private field and the live deadline after obtaining the commit locks.
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    let stored: StoredPreview = sqlx::query_as(&format!("{PREVIEW_SELECT} FOR UPDATE"))
        .bind(id)
        .bind(admin_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::Conflict("预览已保存或不存在，请重新解析".into()))?;
    let now = apsp_protocol::now_timestamp();
    if !stored.matches_capture(&captured, now) {
        return Err(ApiError::Conflict(
            "预览已变化、过期或解析器已更新，请重新解析".into(),
        ));
    }
    let host = stored
        .secret_url
        .as_deref()
        .map(fetch::validate_url)
        .transpose()
        .map_err(super::import_error)?
        .and_then(|url| url.host_str().map(str::to_owned));
    let content = (stored.kind == "inline")
        .then(|| String::from_utf8(body.clone()))
        .transpose()
        .map_err(anyhow::Error::from)?;
    let source: i64 = sqlx::query_scalar("INSERT INTO singbox_subscription_sources(name,kind,secret_url,secret_authorization,secret_content,source_host,refresh_interval_seconds,auto_refresh,user_agent,created_at,last_attempt_at,next_refresh_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10,CASE WHEN $2='url' AND $8 THEN $10+$7 ELSE NULL END) RETURNING id")
        .bind(name).bind(stored.kind).bind(stored.secret_url).bind(stored.secret_authorization).bind(content).bind(host).bind(interval).bind(request.auto_refresh.unwrap_or(true)).bind(stored.user_agent).bind(now).fetch_one(&mut *tx).await?;
    let revision = revisions::save_on(&mut tx, source, 1, 1, &body_sha256, batch, now).await?;
    sqlx::query("UPDATE singbox_external_nodes SET adopted=TRUE WHERE source_id=$1 AND identity_key=ANY($2)").bind(source).bind(keys).execute(&mut *tx).await?;
    revisions::save_traffic_on(
        &mut tx,
        source,
        stored
            .traffic
            .as_object()
            .filter(|fields| !fields.is_empty())
            .map(|_| stored.traffic.clone()),
        stored.created_at,
    )
    .await?;
    sqlx::query("UPDATE singbox_subscription_sources SET current_revision_id=$2,last_success_at=$3 WHERE id=$1").bind(source).bind(revision).bind(now).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM singbox_source_previews WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(model::get_on(&state.pool, source).await?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_requires_the_exact_private_capture_and_current_deadline() {
        let captured = StoredPreview {
            kind: "url".into(),
            secret_url: Some("https://source.example.com/TEST_ONLY-original".into()),
            secret_authorization: Some("Bearer TEST_ONLY-original".into()),
            secret_body: b"http://proxy.example.com:443#first".to_vec(),
            user_agent: fetch::DEFAULT_USER_AGENT.into(),
            parser_version: parse::PARSER_VERSION.into(),
            created_at: 10,
            traffic: serde_json::json!({"download":1}),
            expires_at: 610,
        };
        assert!(captured.matches_capture(&captured, 609));
        assert!(!captured.matches_capture(&captured, 610));
        assert!(!captured.matches_capture(&captured, 611));
        let mut changed = Vec::new();
        let mut row = captured.clone();
        row.kind = "inline".into();
        changed.push(row);
        let mut row = captured.clone();
        row.secret_url = Some("https://source.example.com/TEST_ONLY-replaced".into());
        changed.push(row);
        let mut row = captured.clone();
        row.secret_authorization = Some("Bearer TEST_ONLY-replaced".into());
        changed.push(row);
        let mut row = captured.clone();
        row.secret_body = b"http://proxy.example.com:443#other".to_vec();
        assert_eq!(row.secret_body.len(), captured.secret_body.len());
        changed.push(row);
        let mut row = captured.clone();
        row.user_agent = "TEST_ONLY replacement".into();
        changed.push(row);
        let mut row = captured.clone();
        row.parser_version = "TEST_ONLY old parser".into();
        changed.push(row);
        let mut row = captured.clone();
        row.created_at += 1;
        changed.push(row);
        let mut row = captured.clone();
        row.traffic = serde_json::json!({"download":2});
        changed.push(row);
        let mut row = captured.clone();
        row.expires_at += 1;
        changed.push(row);
        for row in changed {
            assert!(!row.matches_capture(&captured, 20));
        }
        let mut old_parser = captured.clone();
        old_parser.parser_version = "TEST_ONLY old parser".into();
        assert!(!old_parser.matches_capture(&old_parser, 20));
    }
}
