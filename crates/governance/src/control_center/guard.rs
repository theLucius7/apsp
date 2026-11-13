use super::access::{Principal, authenticate, require_recent_proof, with_request_principal};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    ports::{BoxFuture, Port},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderValue, Method, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use serde_json::{Value, json};

pub fn redact(value: &Value) -> Value {
    fn walk(value: &Value, depth: usize) -> Value {
        if depth > 8 {
            return json!("[已截断]");
        }
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .take(64)
                    .map(|(key, value)| {
                        let lower = key.to_ascii_lowercase();
                        let sensitive = [
                            "password",
                            "token",
                            "secret",
                            "private",
                            "credential",
                            "authorization",
                            "command",
                            "content",
                            "input",
                            "certificate_pem",
                            "uuid",
                            "short_id",
                            "totp",
                            "otp",
                            "api_key",
                            "access_key",
                        ]
                        .iter()
                        .any(|item| lower.contains(item))
                            || lower.contains("subscription")
                            || [
                                "url",
                                "uri",
                                "raw",
                                "yaml",
                                "payload",
                                "bundle",
                                "text",
                                "key",
                                "psk",
                                "certificate",
                                "cookie",
                                "email",
                                "notes",
                                "body",
                                "records",
                                "members",
                                "gateway",
                                "merchant",
                                "sign",
                                "activation",
                            ]
                            .contains(&lower.as_str())
                            || lower.ends_with("_url")
                            || lower.ends_with("_uri")
                            || lower.ends_with("_key");
                        (
                            key.clone(),
                            if sensitive {
                                json!("[已脱敏]")
                            } else {
                                walk(value, depth + 1)
                            },
                        )
                    })
                    .collect(),
            ),
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .take(32)
                    .map(|value| walk(value, depth + 1))
                    .collect(),
            ),
            Value::String(value) => json!(value.chars().take(512).collect::<String>()),
            value => value.clone(),
        }
    }
    walk(value, 0)
}

fn audit_request(path: &str, value: &Value) -> Value {
    let mut value = value.clone();
    if path.starts_with("/api/fleet/terminals/") && path.ends_with("/input") {
        // PTY data includes commands and interactive passwords. Keep window
        // metadata, but never persist the terminal byte stream in audit rows.
        if let Some(object) = value.as_object_mut() {
            object.insert("data".into(), json!("[已脱敏]"));
        }
    }
    redact(&value)
}

/// The proxy network's console: traffic services, service access and reachability checks.
fn proxy_console(path: &str) -> bool {
    path.starts_with("/api/traffic/")
        || path.starts_with("/api/access/")
        || path == "/api/observability/reachability"
        || path.starts_with("/api/observability/reachability/")
}

/// Names and DNS records.
fn names(path: &str) -> bool {
    path.starts_with("/api/naming/")
}

/// Alibaba Cloud accounts and resources with the changes made to them.
fn alicloud(path: &str) -> bool {
    path == "/api/resources/alicloud" || path.starts_with("/api/resources/alicloud/")
}

/// Renewals and paid services.
fn expenses(path: &str) -> bool {
    path.starts_with("/api/costs/")
}

/// Routes whose handlers identify their own callers: devices, the sign-in itself, the
/// public dashboard's access check, shared reports, installing machines and the member
/// portal with its accounts.
fn independent_identity(path: &str) -> bool {
    path.starts_with("/api/agent/")
        || path == "/api/login"
        || path.starts_with("/api/login/passkey/")
        || path == "/api/logout"
        || path == "/api/dashboard/access"
        || path.starts_with("/api/access/portal/")
        || path.starts_with("/api/access/portal-public/")
        || path.starts_with("/api/network-workbench/shared/")
        || path.starts_with("/api/bootstrap/")
}

fn feature(path: &str) -> Option<&'static str> {
    if path == "/api/control-center/observers" {
        return Some("monitoring");
    }
    if path.starts_with("/api/control-center/") {
        return Some("security");
    }
    if proxy_console(path) {
        return Some("proxy");
    }
    if names(path) {
        return Some("dns");
    }
    if alicloud(path) || expenses(path) {
        return Some("cloud");
    }
    if path.starts_with("/api/network-workbench/") {
        return Some("diagnostics");
    }
    if path.starts_with("/api/network-configuration/") {
        return Some("network");
    }
    if path.starts_with("/api/operations/backups") || path.starts_with("/api/operations/backup-") {
        return Some("recovery");
    }
    if path.starts_with("/api/operations/cloud") || path == "/api/operations/suppliers" {
        return Some("cloud");
    }
    if path.starts_with("/api/operations/") {
        return Some("operations");
    }
    if path.contains("/terminal") {
        return Some("terminal");
    }
    if path.contains("/files")
        || path.contains("/configurations")
        || path.contains("/config-history")
    {
        return Some("files");
    }
    if path.contains("/services") {
        return Some("services");
    }
    if path.contains("/commands") {
        return Some("terminal");
    }
    if path.contains("/diagnostics")
        || path.contains("/node-quality")
        || path.contains("/tcp-quality")
        || path.contains("/ip-quality")
    {
        return Some("diagnostics");
    }
    if path.starts_with("/api/fleet/") || path.starts_with("/api/servers") {
        return Some("servers");
    }
    if path.starts_with("/api/dashboard/")
        || path.starts_with("/api/statistics")
        || path.starts_with("/api/latency-tasks")
        || path.starts_with("/api/probes/")
        || path.starts_with("/api/notifications")
        || path.starts_with("/api/alert-rules")
        || path.starts_with("/api/telemetry/")
    {
        return Some("monitoring");
    }
    if path.starts_with("/api/security/")
        || path.starts_with("/api/settings")
        || path.starts_with("/api/artifacts")
        || path.starts_with("/api/exchange-rates")
    {
        return Some("security");
    }
    None
}

fn server_path(path: &str) -> Option<i64> {
    let parts: Vec<_> = path.split('/').collect();
    parts.windows(2).find_map(|pair| {
        (pair[0] == "servers")
            .then(|| pair[1].parse::<i64>().ok().filter(|id| *id > 0))
            .flatten()
    })
}

fn own_account_path(path: &str) -> bool {
    path.starts_with("/api/security/totp")
        || path == "/api/security/password"
        || path == "/api/security/passkeys"
        || path.starts_with("/api/security/passkeys/")
        || path == "/api/me"
        || path == "/api/control-center/me"
        || path == "/api/control-center/reauth"
        || path == "/api/control-center/reauth/passkey/start"
        || path == "/api/control-center/reauth/passkey/finish"
        || path == "/api/control-center/events"
        || path == "/api/control-center/export/servers"
        || path == "/api/control-center/search"
        || path.starts_with("/api/control-center/preferences/")
        || path == "/api/control-center/sessions"
        || path.starts_with("/api/control-center/sessions/")
}

/// Changes to the administrator's own sign-in factors use the same recent proof
/// as every other high-risk action, so a Passkey satisfies them as well.
fn own_account_needs_proof(path: &str) -> bool {
    path == "/api/security/password"
        || path == "/api/security/totp/setup"
        || path == "/api/security/totp/disable"
        || path == "/api/security/passkeys/register/start"
        || (path.starts_with("/api/security/passkeys/") && path.ends_with("/remove"))
}

fn high_risk(path: &str, method: &Method) -> bool {
    if *method == Method::GET || *method == Method::HEAD || *method == Method::OPTIONS {
        return false;
    }
    if own_account_path(path) {
        return own_account_needs_proof(path);
    }
    if *method == Method::POST
        && path.starts_with("/api/operations/cloud/hetzner/accounts/")
        && path.ends_with("/refresh")
    {
        return false;
    }
    // Stopping work that was already authorised must never wait for a fresh
    // proof: cancelling a remote command and closing a terminal session.
    if (*method == Method::POST && path.contains("/commands/") && path.ends_with("/cancel"))
        || (*method == Method::DELETE && path.starts_with("/api/fleet/terminals/"))
    {
        return false;
    }
    *method == Method::DELETE
        || path.contains("/enrollment")
        || path.ends_with("/batch-enrollment")
        || path.contains("/commands")
        || path.contains("/terminal")
        || path.contains("/credentials")
        || path.contains("/restore")
        || path.contains("/administrators")
        || path.contains("/cloud/")
        || path.contains("/power")
        || path.contains("/bandwidth")
        || path.contains("/security-group")
        || guarded_change(path)
}

/// Changes that need a recent proof although no general rule above names them: expense
/// imports and settings, changes to Alibaba Cloud accounts and resources (previews,
/// billing checks and refreshes excepted), and in the proxy network the member mail and
/// invitation settings, portal guides, nodes, subscription resets, portal invitations,
/// the reachability opt-in and the subscription domain.
fn guarded_change(path: &str) -> bool {
    path == "/api/costs/import"
        || path == "/api/costs/settings"
        || (path.starts_with("/api/resources/alicloud/")
            && !path.ends_with("/preview")
            && !path.ends_with("/cdt-check")
            && !path.ends_with("/refresh"))
        || (proxy_console(path)
            && !path.ends_with("/preview")
            && (path.ends_with("/account-mail")
                || path.contains("/portal-guides")
                || path.ends_with("/invitation-settings")
                || path.ends_with("/invitations/quota")
                || path.contains("/nodes")
                || path.contains("/nexus-nodes")
                || path.contains("/subscription/reset")
                || path.ends_with("/portal/invitation")
                || path.ends_with("/reachability/settings")
                || path.ends_with("/subscription-domain")))
}

/// Requests that only read although they are not GET: a provider refresh of the
/// inventory, a billing check, and name previews, checks and record comparisons.
fn reads_only(path: &str, method: &Method) -> bool {
    (*method == Method::POST
        && path.starts_with("/api/operations/cloud/hetzner/accounts/")
        && path.ends_with("/refresh"))
        || (path.starts_with("/api/network-configuration/servers/") && path.ends_with("/inventory"))
        || ((alicloud(path) || expenses(path))
            && *method == Method::POST
            && path.ends_with("/cdt-check"))
        || (names(path)
            && ((path.starts_with("/api/naming/rules/") && path.ends_with("/preview"))
                || path.ends_with("/check")
                || (path.starts_with("/api/naming/accounts/")
                    && path.contains("/records/")
                    && path.ends_with("/reconcile"))))
}

/// Entries an administrator limited to some servers may open without a server in the
/// path: the handlers narrow the result to that administrator's servers themselves.
fn any_scope(path: &str, method: &Method, read: bool) -> bool {
    (path == "/api/servers" && read)
        || names(path)
        || (path == "/api/costs/servers" && (*method == Method::GET || *method == Method::HEAD))
        || path.starts_with("/api/network-workbench/")
        || path.starts_with("/api/network-configuration/")
        || path.starts_with("/api/operations/")
        || (read && (path == "/api/fleet/templates" || path == "/api/network-workbench/catalog"))
}

async fn allowed(
    _state: &AppState,
    actor: &Principal,
    path: &str,
    method: &Method,
) -> ApiResult<()> {
    if own_account_path(path) {
        if actor.token_id.is_some()
            && ![
                "/api/me",
                "/api/control-center/me",
                "/api/control-center/search",
                "/api/control-center/events",
                "/api/control-center/export/servers",
            ]
            .contains(&path)
        {
            return Err(ApiError::Forbidden(
                "工作区、账号安全和会话操作需要交互式管理员会话".into(),
            ));
        }
        return Ok(());
    }
    if actor.role == "owner" && actor.token_id.is_none() {
        return Ok(());
    }
    let typed_fleet = path.starts_with("/api/fleet/operations/")
        || path.starts_with("/api/fleet/terminals/")
        || path.ends_with("/fleet/operations");
    if typed_fleet {
        if let Some(id) = server_path(path)
            && !actor.allows_server(id)
        {
            return Err(ApiError::Forbidden(
                "当前管理员或 API 令牌未获授权访问此服务器".into(),
            ));
        }
        // These handlers derive capability and server scope from the typed stored operation.
        return Ok(());
    }
    if path.starts_with("/api/network-configuration/documents/")
        && path.ends_with("/execute")
        && *method == Method::POST
    {
        // The stored document and typed action determine read versus write access.
        if !actor.allows("network:read") && !actor.allows("network:write") {
            return Err(ApiError::Forbidden(
                "当前管理员或 API 令牌没有网络操作权限".into(),
            ));
        }
        return Ok(());
    }
    let feature =
        feature(path).ok_or_else(|| ApiError::Forbidden("此接口尚未开放给受限管理员".into()))?;
    let read = *method == Method::GET || *method == Method::HEAD || reads_only(path, method);

    let capability = format!("{feature}:{}", if read { "read" } else { "write" });
    if !actor.allows(&capability) {
        return Err(ApiError::Forbidden(
            "当前管理员或 API 令牌没有此功能权限".into(),
        ));
    }
    if feature == "security" {
        return Err(ApiError::Forbidden("全局安全设置需要所有者会话".into()));
    }
    if let Some(id) = server_path(path) {
        if !actor.allows_server(id) {
            return Err(ApiError::Forbidden(
                "当前管理员或 API 令牌未获授权访问此服务器".into(),
            ));
        }
    } else if !actor.global_servers() && !any_scope(path, method, read) {
        return Err(ApiError::Forbidden(
            "跨服务器入口需要全服务器授权；可从已授权服务器详情进入".into(),
        ));
    }
    Ok(())
}

/// How the guard treats a request to `path`, as far as the path and method decide it.
/// The route policy table test (`crates/panel/tests/route_policy.rs`) records one line per
/// registered route, so a change of any rule shows up as a reviewed difference.
pub fn describe(path: &str, method: &Method) -> String {
    if !path.starts_with("/api/") {
        return "outside the guard".into();
    }
    if independent_identity(path) {
        return "own identity".into();
    }
    let proof = if high_risk(path, method) {
        ", recent proof"
    } else {
        ""
    };
    let anonymous = if path.starts_with("/api/dashboard/") {
        ", open without a session"
    } else {
        ""
    };
    if own_account_path(path) {
        let tokens = [
            "/api/me",
            "/api/control-center/me",
            "/api/control-center/search",
            "/api/control-center/events",
            "/api/control-center/export/servers",
        ]
        .contains(&path);
        let who = if tokens {
            "any administrator or token"
        } else {
            "interactive administrator"
        };
        return format!("own account: {who}{proof}");
    }
    let typed_fleet = path.starts_with("/api/fleet/operations/")
        || path.starts_with("/api/fleet/terminals/")
        || path.ends_with("/fleet/operations");
    if typed_fleet {
        let scope = match server_path(path) {
            Some(_) => "that server",
            None => "any scope",
        };
        return format!("typed by the stored operation, {scope}{proof}{anonymous}");
    }
    if path.starts_with("/api/network-configuration/documents/")
        && path.ends_with("/execute")
        && *method == Method::POST
    {
        return format!("network:read or network:write, any scope{proof}{anonymous}");
    }
    let Some(feature) = feature(path) else {
        return format!("owner session only{proof}{anonymous}");
    };
    let read = *method == Method::GET || *method == Method::HEAD || reads_only(path, method);
    if feature == "security" {
        return format!("owner session only (security){proof}{anonymous}");
    }
    let scope = if server_path(path).is_some() {
        "that server"
    } else if any_scope(path, method, read) {
        "any scope"
    } else {
        "all servers"
    };
    format!(
        "{feature}:{}, {scope}{proof}{anonymous}",
        if read { "read" } else { "write" }
    )
}

/// The stored object behind a request path, read before and after a change for the audit
/// trail. Declared here and answered by the owners of those objects (governance comes
/// before them in the dependency order); the application connects it.
pub trait AuditSnapshots: Send + Sync {
    /// `None` when no owner keeps an object at `path`; `Null` when the object is absent.
    fn snapshot<'a>(
        &'a self,
        state: &'a AppState,
        path: &'a str,
    ) -> BoxFuture<'a, ApiResult<Option<Value>>>;
}

static SNAPSHOTS: Port<dyn AuditSnapshots> = Port::new();

/// Connects the audit snapshots to their owners; later calls keep the first one.
pub fn install(snapshots: &'static dyn AuditSnapshots) {
    SNAPSHOTS.install(snapshots);
}

async fn snapshot(state: &AppState, path: &str) -> ApiResult<Value> {
    if let Some(snapshots) = SNAPSHOTS.get()
        && let Some(value) = snapshots.snapshot(state, path).await?
    {
        return Ok(redact(&value));
    }
    Ok(json!({"state":"unavailable","reason":"此入口的变更前后证据由对应任务或版本历史保存"}))
}

pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    if !path.starts_with("/api/") || independent_identity(&path) {
        return next.run(request).await;
    }
    let headers = request.headers().clone();
    let actor = match authenticate(&state, &headers).await {
        Ok(actor) => actor,
        Err(ApiError::Unauthorized) if path.starts_with("/api/dashboard/") => {
            return next.run(request).await;
        }
        Err(error) => return error.into_response(),
    };
    let method = request.method().clone();
    if let Err(error) = allowed(&state, &actor, &path, &method).await {
        let _ = sqlx::query("INSERT INTO management_audit(admin_id,token_id,action,object_path,request_diff,result,occurred_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(actor.admin_id).bind(actor.token_id).bind(method.as_str()).bind(&path).bind(json!({"body":"[拒绝请求未读取]"})).bind(json!({"phase":"authorization-denied","success":false})).bind(now_timestamp()).execute(&state.pool).await;
        return error.into_response();
    }
    if high_risk(&path, &method)
        && let Err(error) = require_recent_proof(&state, &headers).await
    {
        // Repeated attempts from a stolen session must leave a trace.
        let _ = sqlx::query("INSERT INTO management_audit(admin_id,token_id,action,object_path,request_diff,result,occurred_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(actor.admin_id).bind(actor.token_id).bind(method.as_str()).bind(&path).bind(json!({"body":"[拒绝请求未读取]"})).bind(json!({"phase":"reauth-denied","success":false})).bind(now_timestamp()).execute(&state.pool).await;
        return error.into_response();
    }
    let mutation = method != Method::GET && method != Method::HEAD && method != Method::OPTIONS;
    let (request, audit_id) = if mutation {
        let (parts, body) = request.into_parts();
        // Audit collection must preserve the largest registered JSON endpoint
        // budget. Individual extractors retain their own smaller body limits.
        let maximum = 3 * 1024 * 1024;
        let mut bytes = Vec::new();
        let mut chunks = body.into_data_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(_) => {
                    return ApiError::BadRequest("请求正文读取失败".into()).into_response();
                }
            };
            if chunk.len() > maximum - bytes.len() {
                return (
                    axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                    Json(json!({"error":"请求正文超过管理接口限制"})),
                )
                    .into_response();
            }
            bytes.extend_from_slice(&chunk);
        }
        let requested = serde_json::from_slice::<Value>(&bytes)
            .map(|value| audit_request(&path, &value))
            .unwrap_or(json!({"bytes":bytes.len(),"body":"[非JSON正文未记录]"}));
        let before = match snapshot(&state, &path).await {
            Ok(value) => value,
            Err(error) => return error.into_response(),
        };
        let diff = json!({"before":before,"requested":requested,"after":{"state":"pending"}});
        let audit=sqlx::query_scalar::<_,i64>("INSERT INTO management_audit(admin_id,token_id,action,object_path,request_diff,result,occurred_at) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id")
            .bind(actor.admin_id).bind(actor.token_id).bind(method.as_str()).bind(&path).bind(diff).bind(json!({"phase":"accepted"})).bind(now_timestamp()).fetch_one(&state.pool).await;
        let id = match audit {
            Ok(id) => id,
            Err(error) => return ApiError::Database(error).into_response(),
        };
        (Request::from_parts(parts, Body::from(bytes)), Some(id))
    } else {
        (request, None)
    };
    // Handlers authenticate again; within this request they get this principal.
    let mut response = with_request_principal(&headers, actor.clone(), next.run(request)).await;
    if let Some(id) = audit_id {
        let after = snapshot(&state, &path)
            .await
            .unwrap_or(json!({"state":"unavailable","reason":"结果状态读取失败"}));
        if let Err(error)=sqlx::query("UPDATE management_audit SET result=$2,request_diff=jsonb_set(request_diff,'{after}',$3) WHERE id=$1").bind(id).bind(json!({"phase":"returned","http_status":response.status().as_u16(),"success":response.status().is_success(),"snapshot_semantics":"邻近请求的状态观察；事务性差异以功能历史为准"})).bind(after).execute(&state.pool).await {
            tracing::error!(%error,audit_id=id,"audit result persistence failed");
        }
    }
    if path == "/api/servers"
        && method == Method::GET
        && response.status().is_success()
        && !actor.global_servers()
    {
        let (parts, body) = response.into_parts();
        let bytes = match to_bytes(body, 16 * 1024 * 1024).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return ApiError::Internal(anyhow::anyhow!("server list filtering failed"))
                    .into_response();
            }
        };
        let filtered = match serde_json::from_slice::<Vec<Value>>(&bytes) {
            Ok(values) => values
                .into_iter()
                .filter(|value| {
                    value
                        .get("id")
                        .and_then(Value::as_i64)
                        .is_some_and(|id| actor.allows_server(id))
                })
                .collect::<Vec<_>>(),
            Err(error) => return ApiError::Internal(error.into()).into_response(),
        };
        let bytes = match serde_json::to_vec(&filtered) {
            Ok(bytes) => bytes,
            Err(error) => return ApiError::Internal(error.into()).into_response(),
        };
        response = Response::from_parts(parts, Body::from(bytes));
        response.headers_mut().remove(header::CONTENT_LENGTH);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_audit_keeps_window_metadata_without_input() {
        let secret = "TEST_ONLY terminal password sentinel";
        let value = audit_request(
            "/api/fleet/terminals/00000000-0000-0000-0000-000000000001/input",
            &json!({"data":secret,"columns":80,"rows":24}),
        );
        assert_eq!(value["data"], "[已脱敏]");
        assert_eq!(value["columns"], 80);
        assert_eq!(value["rows"], 24);
        assert!(!value.to_string().contains(secret));
        assert_eq!(
            audit_request(
                "/api/control-center/preferences/view",
                &json!({"data":"sort"})
            )["data"],
            "sort"
        );
    }
    #[test]
    fn audit_redacts_nested_secrets_and_payloads() {
        let value = redact(
            &json!({"name":"example","credentials":{"token":"never"},"nested":{"password":"never","server_id":2},"command":"secret command"}),
        );
        assert_eq!(value["name"], "example");
        assert_eq!(value["credentials"], "[已脱敏]");
        assert_eq!(value["nested"]["password"], "[已脱敏]");
        assert_eq!(value["nested"]["server_id"], 2);
        assert!(!value.to_string().contains("never"));
    }
    #[test]
    fn proxy_names_cloud_and_expense_routes_keep_their_rules() {
        assert!(independent_identity("/api/access/portal/me"));
        assert!(independent_identity("/api/access/portal-public/invitation"));
        assert!(!independent_identity(
            "/api/access/users/1/portal/invitation"
        ));
        assert!(high_risk(
            "/api/access/users/1/subscription/reset",
            &Method::POST
        ));
        // Issuing or resetting a member's sign-in uses the shared proof.
        assert!(high_risk(
            "/api/access/users/1/portal/invitation",
            &Method::POST
        ));
        // Opting in sends node addresses to a third party; a run stays within that choice.
        assert!(high_risk(
            "/api/observability/reachability/settings",
            &Method::PUT
        ));
        assert!(!high_risk(
            "/api/observability/reachability/run",
            &Method::POST
        ));
        assert!(!high_risk("/api/traffic/nodes/preview", &Method::POST));
        assert!(high_risk("/api/traffic/nodes", &Method::POST));
        let reconcile = "/api/naming/accounts/1/records/2/reconcile";
        assert_eq!(feature(reconcile), Some("dns"));
        assert!(reads_only(reconcile, &Method::POST));
        assert!(any_scope(reconcile, &Method::POST, true));
        // A resolver test sends traffic from the panel and stores the result.
        assert!(!reads_only("/api/naming/accounts/1/resolve", &Method::POST));
        assert!(high_risk(
            "/api/resources/alicloud/resources/1/power",
            &Method::POST
        ));
        assert!(!high_risk(
            "/api/resources/alicloud/accounts/1/refresh",
            &Method::POST
        ));
        // Replacing a public IP releases an address: confirming, cancelling and closing it
        // need the shared proof; reading the preview does not.
        for action in ["confirm", "cancel", "reconcile"] {
            assert!(high_risk(
                &format!("/api/resources/alicloud/eip-rotations/1/{action}"),
                &Method::POST
            ));
        }
        assert!(!high_risk(
            "/api/resources/alicloud/resources/1/eip-rotation/preview",
            &Method::POST
        ));
        assert_eq!(feature("/api/resources/alicloud"), Some("cloud"));
        assert!(!high_risk("/api/resources/alicloud", &Method::PUT));
        assert!(high_risk(
            "/api/resources/alicloud/resources/1",
            &Method::PUT
        ));
        assert!(!high_risk(
            "/api/resources/alicloud/resources/1/preview",
            &Method::POST
        ));
        assert!(!high_risk(
            "/api/resources/alicloud/resources/1",
            &Method::GET
        ));
        let check = "/api/resources/alicloud/resources/1/cdt-check";
        assert!(reads_only(check, &Method::POST) && !high_risk(check, &Method::POST));
        assert!(high_risk(
            "/api/resources/alicloud/resources/1/cdt-state",
            &Method::POST
        ));
        assert_eq!(feature("/api/costs/settings"), Some("cloud"));
        assert!(high_risk("/api/costs/settings", &Method::PUT));
        assert!(any_scope("/api/costs/servers", &Method::GET, true));
        assert_eq!(feature("/api/unknown/jobs"), None);
    }

    #[test]
    fn api_id_cannot_be_misread_as_a_server_scope() {
        assert_eq!(server_path("/api/traffic/servers/3/deployments"), Some(3));
        assert_eq!(server_path("/api/naming/accounts/3"), None);
        assert!(high_risk("/api/servers/3/commands", &Method::POST));
    }
    #[test]
    fn enrollment_needs_proof_while_stopping_work_does_not() {
        assert!(high_risk("/api/fleet/batch-enrollment", &Method::POST));
        assert!(high_risk("/api/servers/3/enrollment", &Method::POST));
        assert!(!high_risk(
            "/api/servers/3/commands/0b6f/cancel",
            &Method::POST
        ));
        assert!(!high_risk("/api/fleet/terminals/0b6f", &Method::DELETE));
        assert!(high_risk("/api/servers/3/fleet/terminals", &Method::POST));
        assert!(high_risk("/api/servers/3", &Method::DELETE));
    }
    #[test]
    fn own_sign_in_factor_changes_need_the_shared_proof() {
        for path in [
            "/api/security/password",
            "/api/security/totp/setup",
            "/api/security/totp/disable",
            "/api/security/passkeys/register/start",
            "/api/security/passkeys/0b6f/remove",
        ] {
            assert!(high_risk(path, &Method::POST), "{path}");
        }
        // The new code proves the authenticator; a finish is bound to its challenge.
        for path in [
            "/api/security/totp/confirm",
            "/api/security/passkeys/register/finish",
            "/api/control-center/reauth",
            "/api/control-center/reauth/passkey/start",
            "/api/control-center/reauth/passkey/finish",
            "/api/control-center/preferences/theme",
        ] {
            assert!(!high_risk(path, &Method::POST), "{path}");
        }
        assert!(!high_risk("/api/security/passkeys", &Method::GET));
    }
}
