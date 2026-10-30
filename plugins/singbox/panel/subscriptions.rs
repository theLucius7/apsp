use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_compiler::Node;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct SubscriptionQuery {
    pub format: Option<String>,
    pub download: Option<bool>,
}

fn validate_format(format: &str) -> ApiResult<()> {
    if !matches!(format, "links" | "singbox" | "clash") {
        return Err(ApiError::BadRequest(
            "订阅格式仅支持 links、singbox 或 clash".into(),
        ));
    }
    Ok(())
}

/// Clash family clients (Clash, Clash Meta/mihomo, Clash Verge, FlClash, Stash)
/// fetch the plain subscription URL and expect a Clash profile. Other clients
/// keep the original default.
fn client_format(headers: &HeaderMap) -> Option<&'static str> {
    let agent = headers
        .get(header::USER_AGENT)?
        .to_str()
        .ok()?
        .to_ascii_lowercase();
    ["clash", "mihomo", "stash"]
        .iter()
        .any(|family| agent.contains(family))
        .then_some("clash")
}

struct Snapshot {
    template: Option<Value>,
    nodes: Vec<Node>,
    external: super::external_access::model::SubscriptionNodes,
    granted_nodes: i64,
    eligible_nodes: usize,
    entitlement: super::entitlements::Entitlement,
}

impl Snapshot {
    fn message(&self) -> (&'static str, String) {
        if !self.entitlement.allowed {
            let reason = match self.entitlement.status.as_str() {
                "expired" => "套餐已到期，请先续期或重新分配套餐",
                "exhausted" => "当前套餐周期流量已用尽，请等待重置或调整套餐",
                "not_started" => "套餐尚未生效，请等待开始时间",
                _ => "当前套餐状态不允许使用代理",
            };
            return ("blocked", reason.into());
        }
        if self.nodes.is_empty() && self.external.nodes.is_empty() {
            return (
                "empty",
                if self.granted_nodes == 0 {
                    "尚未分配节点，请先授权节点或分配策略组"
                } else {
                    "暂无可订阅节点，请检查节点启用状态、链路两端和配置部署结果"
                }
                .into(),
            );
        }
        (
            "ready",
            if self.external.granted > 0 {
                format!(
                    "{} 个受管节点、{} 个外部节点可生成订阅；外部运行与流量由提供方控制",
                    self.nodes.len(),
                    self.external.nodes.len()
                )
            } else {
                format!(
                    "{} 个节点可生成订阅；仅包含当前有效且已成功应用的授权",
                    self.nodes.len()
                )
            },
        )
    }

    fn content(&self, user_id: i64, format: &str) -> ApiResult<String> {
        // A restricted entitlement is refused with the same reason in every format: an empty
        // 200 response would make clients silently drop all of the user's nodes.
        if !self.entitlement.allowed {
            return Err(ApiError::Conflict(self.message().1));
        }
        if format == "clash" {
            if self.nodes.is_empty() && self.external.nodes.is_empty() {
                return Err(ApiError::Conflict(self.message().1));
            }
            // A sing-box client template describes sing-box routing; the Clash profile
            // carries the same nodes with its own selection groups.
            let profile =
                apsp_compiler::clash::compile_clash(&self.nodes, user_id, &self.external.nodes)
                    .map_err(anyhow::Error::from)?;
            if profile.included == 0 {
                return Err(ApiError::Conflict(
                    "订阅中的节点均无法以 Clash 格式等价表达，请使用 format=singbox 下载 sing-box JSON".into(),
                ));
            }
            return Ok(profile.content);
        }
        if format == "links" {
            if self.template.is_some() {
                return Err(ApiError::Conflict("当前用户启用完整客户端模板，请使用 singbox 格式；链接格式不能表达选择组、DNS 和路由".into()));
            }
            if self.external.granted > 0 {
                return Err(ApiError::Conflict(
                    "此订阅包含外部节点，请使用 format=singbox 下载完整 sing-box JSON".into(),
                ));
            }
            apsp_compiler::subscription_links(&self.nodes, user_id).map_err(|error| match error {
                apsp_compiler::CompileError::RequiresJson { .. } => ApiError::Conflict(
                    "此订阅包含需要完整配置的协议，请使用 format=singbox 下载 sing-box JSON".into(),
                ),
                other => ApiError::Internal(anyhow::Error::from(other)),
            })
        } else {
            if self.nodes.is_empty() && self.external.nodes.is_empty() {
                return Err(ApiError::Conflict(self.message().1));
            }
            super::operations_workflows::apply_definition(
                apsp_compiler::client::compile_with_external(
                    &self.nodes,
                    user_id,
                    &self.external.nodes,
                )
                .map_err(anyhow::Error::from)?,
                self.template.as_ref(),
            )
        }
    }
}

async fn load(tx: &mut Transaction<'_, Postgres>, user_id: i64) -> ApiResult<Snapshot> {
    let at = apsp_protocol::now_timestamp();
    let entitlement: super::entitlements::Entitlement =
        sqlx::query_as("SELECT * FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(at)
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    let granted_nodes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM singbox_desired_accesses WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await?;
    let accesses = sqlx::query("SELECT a.node_id,a.uuid,a.credential FROM singbox_eligible_accesses($2) a WHERE a.user_id=$1 AND NOT EXISTS (SELECT 1 FROM singbox_live_chains c JOIN nodes n ON n.id=c.entry_node_id LEFT JOIN nodes e ON e.id=c.exit_node_id WHERE c.entry_node_id=a.node_id AND ((c.path_kind='legacy' AND (SELECT COUNT(*) FROM server_module_status m JOIN servers s ON s.id=m.server_id WHERE m.server_id=ANY(ARRAY[n.server_id,e.server_id]) AND m.module='singbox' AND m.healthy AND m.applied_rev=m.target_rev AND s.dirty_at IS NULL AND s.deleted_at IS NULL) <> 2) OR (c.path_kind='mixed' AND NOT singbox_path_ready(c.id))))")
        .bind(user_id).bind(at).fetch_all(&mut **tx).await?;
    let mut current = BTreeMap::<i64, (Uuid, String)>::new();
    for row in accesses {
        let node_id: i64 = row.get("node_id");
        let chain: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM singbox_chains WHERE entry_node_id=$1 AND path_kind='ordered' AND (deleted_at IS NULL OR phase<>'retired')",
        )
        .bind(node_id)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(chain) = chain
            && !super::ordered_paths::lifecycle::qualified(tx, chain).await?
        {
            continue;
        }
        current.insert(node_id, (row.get("uuid"), row.get("credential")));
    }
    let eligible_nodes = current.len();
    let snapshots: Vec<(i64,i64,serde_json::Value)> = sqlx::query_as("SELECT d.server_id,d.rev,COALESCE(p.source_json,d.source_json) FROM deployments d LEFT JOIN singbox_deployment_projections p ON p.server_id=d.server_id AND p.rev=d.rev JOIN server_module_status m ON m.server_id=d.server_id AND m.module=d.module AND m.applied_rev=d.rev JOIN servers s ON s.id=d.server_id WHERE m.module='singbox' AND m.healthy AND s.deleted_at IS NULL AND EXISTS (SELECT 1 FROM nodes n WHERE n.server_id=s.id AND n.id=ANY($1)) ORDER BY d.server_id")
        .bind(current.keys().copied().collect::<Vec<_>>()).fetch_all(&mut **tx).await?;
    let mut nodes = Vec::new();
    for (server, revision, snapshot) in snapshots {
        let snapshot =
            super::ordered_paths::models::public_nodes(snapshot).map_err(anyhow::Error::from)?;
        let projection:BTreeMap<i64,serde_json::Value>=sqlx::query_as::<_,(i64,serde_json::Value)>("SELECT node_id,public_fields FROM singbox_deployment_public_projection WHERE server_id=$1 AND module='singbox' AND revision=$2").bind(server).bind(revision).fetch_all(&mut **tx).await?.into_iter().collect();
        for mut node in snapshot {
            if let Some(display) = projection.get(&node.id) {
                if let Some(name) = display.get("name").and_then(serde_json::Value::as_str) {
                    node.name = name.into();
                }
                if let Some(host) = display
                    .get("public_host")
                    .and_then(serde_json::Value::as_str)
                {
                    node.public_host = host.into();
                }
                if let Some(sni) = display.get("sni").and_then(serde_json::Value::as_str) {
                    node.sni = sni.into();
                }
                if let Some(port) = display
                    .get("public_port")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .filter(|port| *port > 0)
                {
                    node.settings.public_port = Some(port);
                }
            }
            node.users.retain(|access| {
                access.user_id == user_id
                    && current.get(&node.id).is_some_and(|(uuid, credential)| {
                        *uuid == access.uuid && credential == &access.credential
                    })
            });
            if !node.users.is_empty() {
                nodes.push(node);
            }
        }
    }
    nodes.retain(|node| node.enabled);
    nodes.sort_by_key(|node| node.id);
    let mut external = super::external_access::subscription_nodes(tx, user_id).await?;
    if !entitlement.allowed {
        external.nodes.clear();
    }
    let eligible_nodes = eligible_nodes + external.nodes.len();
    let granted_nodes = granted_nodes + external.granted as i64;
    Ok(Snapshot {
        template: sqlx::query_scalar(
            "SELECT definition FROM singbox_client_templates WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?,
        nodes,
        external,
        granted_nodes,
        eligible_nodes,
        entitlement,
    })
}

pub(super) async fn diagnostic_on(
    tx: &mut Transaction<'_, Postgres>,
    user: i64,
) -> ApiResult<Value> {
    let snapshot = load(tx, user).await?;
    let (mut status, mut message) = snapshot.message();
    if status == "ready" {
        match snapshot.content(user, "singbox") {
            Ok(_) => {}
            Err(ApiError::Conflict(reason)) => {
                status = "format_unavailable";
                message = reason;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(
        json!({"status":status,"message":message,"entitlement":snapshot.entitlement,"granted_nodes":snapshot.granted_nodes,"eligible_nodes":snapshot.eligible_nodes,"ready_managed_nodes":snapshot.nodes.len(),"ready_external_nodes":snapshot.external.nodes.len(),"client_template":snapshot.template.is_some()}),
    )
}

pub(super) async fn content_on(
    tx: &mut Transaction<'_, Postgres>,
    user: i64,
    with_template: bool,
) -> ApiResult<String> {
    let mut snapshot = load(tx, user).await?;
    if !with_template {
        snapshot.template = None;
    }
    snapshot.content(user, "singbox")
}

/// Digest of a user's rendered subscription, for comparing before and after
/// a migration; an unavailable format records its reason instead.
pub(crate) async fn content_digest(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    format: &str,
) -> ApiResult<String> {
    let snapshot = load(tx, user_id).await?;
    Ok(match snapshot.content(user_id, format) {
        Ok(content) => crate::auth::hash_token(&content),
        Err(error) => format!("unavailable: {error}"),
    })
}

fn content_type(format: &str) -> &'static str {
    match format {
        "singbox" => "application/json; charset=utf-8",
        "clash" => "text/yaml; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    }
}
fn filename(user_id: i64, format: &str) -> String {
    format!(
        "apsp-{user_id}.{}",
        match format {
            "singbox" => "json",
            "clash" => "yaml",
            _ => "txt",
        }
    )
}

/// Uplink and downlink of one user inside `(after, until]`. This is the split twin of the SQL
/// function `singbox_usage_window`: whole UTC days come from the daily rollup and at most one
/// partial day at each end from the ledger, so both sums add up to the cycle usage that quota
/// enforcement reads. Keep the two in step.
const CYCLE_USAGE: &str = "WITH bounds AS (SELECT (ceil(($2::bigint+1)::numeric/86400)*86400)::bigint AS lo,(floor(($3::bigint+1)::numeric/86400)*86400)::bigint AS hi) \
    SELECT COALESCE(SUM(p.uplink),0)::text AS uplink,COALESCE(SUM(p.downlink),0)::text AS downlink FROM (\
    SELECT d.uplink,d.downlink FROM singbox_usage_daily d,bounds b WHERE d.user_id=$1 AND d.day>=b.lo AND d.day<b.hi \
    UNION ALL SELECT r.uplink,r.downlink FROM usage_records r,bounds b WHERE r.user_id=$1 AND r.period_end>$2::bigint AND r.period_end<LEAST(b.lo,$3::bigint+1) \
    UNION ALL SELECT r.uplink,r.downlink FROM usage_records r,bounds b WHERE r.user_id=$1 AND r.period_end>=GREATEST(b.lo,b.hi) AND r.period_end<=$3::bigint) p";

/// Cumulative usage of a user without a package, the same figures the portal shows.
const TOTAL_USAGE: &str = "SELECT COALESCE(SUM(uplink),0)::text AS uplink,COALESCE(SUM(downlink),0)::text AS downlink FROM singbox_usage_daily WHERE user_id=$1";

fn amount(row: &sqlx::postgres::PgRow, column: &str) -> ApiResult<u128> {
    row.try_get::<String, _>(column)?
        .parse::<u128>()
        .map_err(|error| ApiError::Internal(anyhow::Error::from(error)))
}

/// Splits the usage that quota enforcement sees between upload and download. A manual quota
/// reset credits part of the raw ledger, so the raw split is scaled down to keep
/// upload + download equal to the enforced usage.
fn enforced_split(used: u128, upload: u128, download: u128) -> (u128, u128) {
    let raw = upload.saturating_add(download);
    if used >= raw {
        return (upload, download);
    }
    match used.checked_mul(upload) {
        Some(product) => {
            let scaled = product / raw;
            (scaled, used - scaled)
        }
        None => (upload, download),
    }
}

fn userinfo_value(upload: u128, download: u128, total: &str, expire: Option<i64>) -> String {
    match expire {
        Some(expire) => {
            format!("upload={upload}; download={download}; total={total}; expire={expire}")
        }
        None => format!("upload={upload}; download={download}; total={total}"),
    }
}

/// Value of the `Subscription-Userinfo` header that clients use to show remaining traffic and
/// expiry. A user with a package reports the current cycle and its quota; a user without one
/// has no cycle or quota, so total is 0 and usage is cumulative. Expiry is left out when the
/// package has none. All values are digits, so the header stays ASCII.
async fn subscription_userinfo(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    entitlement: &super::entitlements::Entitlement,
) -> ApiResult<String> {
    let (upload, download) = match (entitlement.cycle_start, entitlement.next_reset) {
        (Some(start), Some(end)) => {
            let row = sqlx::query(CYCLE_USAGE)
                .bind(user_id)
                .bind(start)
                .bind(end)
                .fetch_one(&mut **tx)
                .await?;
            let (upload, download) = (amount(&row, "uplink")?, amount(&row, "downlink")?);
            match entitlement.used_bytes.parse::<u128>() {
                Ok(used) => enforced_split(used, upload, download),
                Err(_) => (upload, download),
            }
        }
        _ => {
            let row = sqlx::query(TOTAL_USAGE)
                .bind(user_id)
                .fetch_one(&mut **tx)
                .await?;
            (amount(&row, "uplink")?, amount(&row, "downlink")?)
        }
    };
    Ok(userinfo_value(
        upload,
        download,
        entitlement.monthly_bytes.as_deref().unwrap_or("0"),
        entitlement.expires_at,
    ))
}

pub async fn get(
    State(state): State<AppState>,
    request_headers: HeaderMap,
    Path(token): Path<String>,
    Query(query): Query<SubscriptionQuery>,
) -> ApiResult<Response> {
    // Token-only URLs keep their original Reality links format, except for Clash
    // family clients, which cannot read links.
    let format = query
        .format
        .as_deref()
        .or_else(|| client_format(&request_headers))
        .unwrap_or("links");
    validate_format(format)?;
    if token.is_empty() || token.len() > 512 {
        return Err(ApiError::NotFound);
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='5s'")
        .execute(&mut *tx)
        .await?;
    let (user_id, user_name) = sqlx::query_as::<_, (i64, String)>(
        "SELECT id,name FROM users WHERE subscription_token=$1 AND deleted_at IS NULL",
    )
    .bind(token)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let snapshot = load(&mut tx, user_id).await?;
    let body = snapshot.content(user_id, format);
    // Usage headers only accompany a subscription that is actually served.
    let userinfo = if body.is_ok() {
        Some(subscription_userinfo(&mut tx, user_id, &snapshot.entitlement).await?)
    } else {
        None
    };
    tx.commit().await?;
    let mut audit = state.pool.begin().await?;
    let record: bool = sqlx::query_scalar("SELECT subscription_days>0 AND COALESCE((SELECT days>0 FROM record_retention_policy WHERE kind='proxy-access'),TRUE) FROM singbox_operation_privacy WHERE singleton").fetch_one(&mut *audit).await?;
    if record {
        super::operations_workflows::record_subscription(
            &mut audit,
            user_id,
            format,
            snapshot.nodes.len(),
            snapshot.external.nodes.len(),
            if body.is_ok() {
                "generated"
            } else if snapshot.entitlement.allowed {
                "unavailable"
            } else {
                "restricted"
            },
        )
        .await?;
    }
    audit.commit().await?;
    let body = body?;
    let mut response = (
        [
            (header::CONTENT_TYPE, content_type(format)),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        body,
    )
        .into_response();
    let headers = response.headers_mut();
    if let Some(userinfo) = userinfo {
        headers.insert(
            HeaderName::from_static("subscription-userinfo"),
            userinfo.parse().map_err(anyhow::Error::from)?,
        );
    }
    // Clients refresh the subscription every 24 hours. The title is base64 text, so the header
    // stays ASCII whatever characters the user's name contains.
    headers.insert(
        HeaderName::from_static("profile-update-interval"),
        HeaderValue::from_static("24"),
    );
    let title = user_name.trim();
    let title = if title.is_empty() { "apsp" } else { title };
    headers.insert(
        HeaderName::from_static("profile-title"),
        format!("base64:{}", STANDARD.encode(title))
            .parse()
            .map_err(anyhow::Error::from)?,
    );
    if query.download == Some(true) {
        headers.insert(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename(user_id, format))
                .parse()
                .map_err(anyhow::Error::from)?,
        );
    }
    Ok(response)
}

pub async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(query): Query<SubscriptionQuery>,
) -> ApiResult<Response> {
    let administrator = super::secret_access::require_reveal(&state, &headers).await?;
    let format = query.format.as_deref().unwrap_or("singbox");
    validate_format(format)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='5s'")
        .execute(&mut *tx)
        .await?;
    let token: String = sqlx::query_scalar(
        "SELECT subscription_token FROM users WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let snapshot = load(&mut tx, id).await?;
    tx.commit().await?;
    let mut audit = state.pool.begin().await?;
    super::operations_workflows::event(&mut audit,Some(administrator),Some(id),"security_subscription_credentials_read",json!({"format":format,"managed_nodes":snapshot.nodes.len(),"external_nodes":snapshot.external.nodes.len(),"credential_values_recorded":false})).await?;
    audit.commit().await?;
    let mut formats = vec!["singbox"];
    if snapshot.entitlement.allowed && snapshot.content(id, "clash").is_ok() {
        formats.push("clash");
    }
    if snapshot.template.is_none()
        && snapshot.external.granted == 0
        && snapshot
            .nodes
            .iter()
            .all(|node| node.protocol_config.is_reality())
    {
        formats.push("links");
    }
    let (mut status, mut message) = snapshot.message();
    let content = if (!snapshot.nodes.is_empty() || !snapshot.external.nodes.is_empty())
        && snapshot.entitlement.allowed
    {
        match snapshot.content(id, format) {
            Ok(content) => Some(content),
            Err(ApiError::Conflict(reason)) => {
                status = "format_unavailable";
                message = reason;
                None
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    let mut ready_nodes: Vec<Value> = snapshot
        .nodes
        .iter()
        .map(|node| json!({"kind":"managed","id":node.id,"name":node.name,"protocol":node.protocol_config.kind()}))
        .collect();
    if snapshot.entitlement.allowed {
        ready_nodes.extend(snapshot.external.entries.iter().filter(|node|node.available).map(|node|json!({
            "kind":"external","id":node.reference.external_node_id,"name":node.name,"protocol":node.protocol,
            "source_id":node.reference.source_id,"source_name":node.source_name,"source_last_error":node.source_last_error
        })));
    }
    Ok(([
        (header::CACHE_CONTROL, "no-store"), (header::REFERRER_POLICY, "no-referrer"),
    ], Json(json!({
        "format":format,"status":status,"message":message,"available_formats":formats,
        "granted_nodes":snapshot.granted_nodes,"eligible_nodes":snapshot.eligible_nodes,"ready_nodes":ready_nodes,
        "managed_nodes":snapshot.nodes.len(),"external_nodes":snapshot.external.nodes.len(),
        "external_granted_nodes":snapshot.external.granted,"external_metering":"provider",
        "content":content,"filename":filename(id,format),"content_type":content_type(format),
        "entitlement":snapshot.entitlement,"subscription_url":format!("{}/sub/{token}",state.config.public_url)
    }))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforced_usage_keeps_the_raw_split_until_a_credit_reduces_it() {
        assert_eq!(enforced_split(100, 40, 60), (40, 60));
        // Half of the raw usage was credited: the split shrinks proportionally.
        assert_eq!(enforced_split(50, 40, 60), (20, 30));
        assert_eq!(enforced_split(0, 40, 60), (0, 0));
        assert_eq!(enforced_split(0, 0, 0), (0, 0));
        // Values too large to scale fall back to the raw split instead of overflowing.
        let big = 1u128 << 100;
        assert_eq!(enforced_split(big, big, big), (big, big));
    }

    #[test]
    fn clash_family_clients_get_the_clash_profile_by_default() {
        let agent = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::USER_AGENT, value.parse().unwrap());
            client_format(&headers)
        };
        for clash in [
            "clash-verge/v2.2.3",
            "ClashforWindows/0.20.39",
            "mihomo/1.19.29",
            "Clash.Meta",
            "FlClash/0.8.80",
            "ClashMetaForAndroid/2.11.5",
            "Stash/2.7.6 Clash/1.11.0",
        ] {
            assert_eq!(agent(clash), Some("clash"), "{clash}");
        }
        for other in ["sing-box 1.14.2", "SFA/1.11", "curl/8.5.0", "v2rayN/7.0"] {
            assert_eq!(agent(other), None, "{other}");
        }
        assert_eq!(client_format(&HeaderMap::new()), None);
        assert!(validate_format("clash").is_ok());
        assert!(validate_format("yaml").is_err());
        assert_eq!(filename(7, "clash"), "apsp-7.yaml");
        assert_eq!(content_type("clash"), "text/yaml; charset=utf-8");
    }

    #[test]
    fn userinfo_header_lists_usage_quota_and_optional_expiry() {
        assert_eq!(
            userinfo_value(1, 2, "3", Some(4)),
            "upload=1; download=2; total=3; expire=4"
        );
        assert_eq!(
            userinfo_value(1, 2, "0", None),
            "upload=1; download=2; total=0"
        );
    }
}
