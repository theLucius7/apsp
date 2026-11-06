//! Optional per-user hostnames (ADR 0103): each user fetches their subscription from
//! `https://{prefix}{slug}.{zone}/{path}` and signs in at `{path}/login` there, both
//! parts random and resettable, so a scanner has neither a shared hostname nor a shared
//! path to look at. Each host has its own proxied DNS record and answers nothing else:
//! not the administrator console, not the bare hostname, not another user's account.
use crate::{
    AppState,
    auth::random_token,
    control_center,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    body::Body,
    extract::{Query, Request, State},
    http::{HeaderMap, Method, StatusCode, Uri, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rand::RngCore;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use std::{collections::HashMap, sync::LazyLock, time::Duration};
use tokio::sync::Notify;
use uuid::Uuid;

const CLOUDFLARE_API: &str = "https://api.cloudflare.com/client/v4";
const SLUG_LENGTH: usize = 16;
const BATCH: i64 = 25;
const RECORD_COMMENT: &str = "apsp subscription host; managed by the panel";

static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);

#[derive(Clone, FromRow)]
struct Domain {
    enabled: bool,
    credential_id: Uuid,
    zone_id: String,
    zone_name: String,
    host_prefix: String,
    target: String,
    legacy_paths: bool,
    revision: i64,
}

async fn domain(pool: &PgPool) -> ApiResult<Option<Domain>> {
    Ok(sqlx::query_as(
        "SELECT enabled,credential_id,zone_id,zone_name,host_prefix,target,legacy_paths,revision FROM singbox_subscription_domain",
    )
    .fetch_optional(pool)
    .await?)
}

fn random_slug() -> String {
    // Lowercase base32: 80 random bits that stay valid in a hostname.
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut bytes = [0_u8; SLUG_LENGTH];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|byte| ALPHABET[usize::from(byte & 31)] as char)
        .collect()
}

fn host_name(domain: &Domain, slug: &str) -> String {
    format!("{}{slug}.{}", domain.host_prefix, domain.zone_name)
}

fn address_of(domain: &Domain, slug: &str, path: &str) -> String {
    format!("https://{}/{path}", host_name(domain, slug))
}

const BRIDGED: &str = "EXISTS(SELECT 1 FROM singbox_nexus_identities i JOIN singbox_nexus_bridge b ON b.import_id=i.import_id WHERE b.enabled AND i.user_id=u.id)";

/// Whether the retained legacy renderer still serves this user's subscription.
pub async fn is_bridged(pool: &PgPool, user: i64) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar(&format!("SELECT {BRIDGED} FROM users u WHERE u.id=$1"))
            .bind(user)
            .fetch_optional(pool)
            .await?
            .unwrap_or(false),
    )
}

/// Hostnames that can be handed out now. A host whose DNS record is not ready falls
/// back while another address still works; a bridged user always has one that does.
pub async fn own_addresses(pool: &PgPool) -> ApiResult<HashMap<i64, String>> {
    let Some(domain) = domain(pool).await?.filter(|domain| domain.enabled) else {
        return Ok(HashMap::new());
    };
    let rows: Vec<(i64, String, String, String, bool)> = sqlx::query_as(&format!(
        "SELECT h.user_id,h.slug,h.path_token,h.state,{BRIDGED} FROM singbox_subscription_hosts h JOIN users u ON u.id=h.user_id"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(_, _, _, state, bridged)| state == "ready" || (!domain.legacy_paths && !bridged))
        .map(|(user, slug, path, _, _)| (user, address_of(&domain, &slug, &path)))
        .collect())
}

pub async fn own_address(pool: &PgPool, user: i64) -> ApiResult<Option<String>> {
    Ok(own_addresses(pool).await?.remove(&user))
}

/// Where each user's subscription can really be fetched, when that is not the panel's
/// own `/sub/{token}`: their hostname, or else the retained renderer that serves them.
pub async fn addresses(pool: &PgPool) -> ApiResult<HashMap<i64, String>> {
    let bridged: Vec<(i64, String)> = sqlx::query_as(
        "SELECT i.user_id,b.subscription_base||u.subscription_token FROM singbox_nexus_identities i JOIN singbox_nexus_bridge b ON b.import_id=i.import_id JOIN users u ON u.id=i.user_id WHERE b.enabled AND u.deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await?;
    let mut addresses: HashMap<i64, String> = bridged.into_iter().collect();
    addresses.extend(own_addresses(pool).await?);
    Ok(addresses)
}

pub async fn address(pool: &PgPool, user: i64) -> ApiResult<Option<String>> {
    Ok(addresses(pool).await?.remove(&user))
}

/// Where this user signs in when they have their own hostname.
pub async fn login_base(pool: &PgPool, user: i64) -> ApiResult<Option<String>> {
    Ok(own_address(pool, user)
        .await?
        .map(|address| format!("{address}/login")))
}

/// Whether `/sub/{token}` on the panel's own host has been turned off.
pub async fn legacy_paths_closed(pool: &PgPool) -> ApiResult<bool> {
    Ok(domain(pool)
        .await?
        .is_some_and(|domain| domain.enabled && !domain.legacy_paths))
}

fn plain_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Body::empty(),
    )
        .into_response()
}

fn request_host(request: &Request) -> Option<&str> {
    let value = request.headers().get(header::HOST)?.to_str().ok()?;
    Some(
        value
            .rsplit_once(':')
            .filter(|(_, port)| port.bytes().all(|byte| byte.is_ascii_digit()))
            .map_or(value, |(host, _)| host),
    )
}

fn panel_host(public_url: &str) -> &str {
    let rest = public_url
        .split_once("://")
        .map_or(public_url, |(_, rest)| rest);
    let host = rest.split('/').next().unwrap_or(rest);
    host.rsplit_once(':').map_or(host, |(host, _)| host)
}

fn slug_of<'a>(domain: &Domain, host: &'a str) -> Option<&'a str> {
    let label = host
        .strip_suffix(domain.zone_name.as_str())?
        .strip_suffix('.')?;
    let slug = label.strip_prefix(domain.host_prefix.as_str())?;
    (slug.len() == SLUG_LENGTH
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte)))
    .then_some(slug)
}

#[derive(Clone)]
struct OwnHost {
    user: i64,
    token: String,
    path: String,
    /// Set while the retained legacy renderer still serves this user.
    renderer: Option<String>,
}

async fn own_host(pool: &PgPool, slug: &str) -> Option<OwnHost> {
    let row: Option<(i64, String, String, Option<String>)> = sqlx::query_as(
        "SELECT h.user_id,u.subscription_token,h.path_token,(SELECT b.subscription_base FROM singbox_nexus_identities i JOIN singbox_nexus_bridge b ON b.import_id=i.import_id WHERE b.enabled AND i.user_id=u.id) FROM singbox_subscription_hosts h JOIN users u ON u.id=h.user_id WHERE h.slug=$1 AND u.deleted_at IS NULL",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    row.map(|(user, token, path, renderer)| OwnHost {
        user,
        token,
        path,
        renderer,
    })
}

const PORTAL_API: &str = "/api/plugins/sing-box/portal/";

async fn portal_account(pool: &PgPool, user: i64) -> Option<Uuid> {
    sqlx::query_scalar("SELECT account_id FROM singbox_portal_accounts WHERE user_id=$1")
        .bind(user)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// Whether the panel's own host may still serve this account's portal.
async fn portal_moved(pool: &PgPool, account: &str) -> bool {
    let Ok(account) = account.parse::<Uuid>() else {
        return false;
    };
    let closed = legacy_paths_closed(pool).await.unwrap_or(false);
    closed && sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM singbox_portal_accounts p JOIN singbox_subscription_hosts h ON h.user_id=p.user_id WHERE p.account_id=$1)")
        .bind(account)
        .fetch_one(pool)
        .await
        .unwrap_or(false)
}

/// Runs before every route. A user's hostname answers only that user's subscription,
/// sign-in page and portal API, and looks like nothing else; every other hostname
/// continues to the panel untouched.
pub async fn gate(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let Some(host) = request_host(&request).map(str::to_ascii_lowercase) else {
        return next.run(request).await;
    };
    let path = request.uri().path().to_owned();
    if host == panel_host(&state.config.public_url) {
        if let Some(rest) = path.strip_prefix(PORTAL_API)
            && portal_moved(&state.pool, rest.split('/').next().unwrap_or_default()).await
        {
            return plain_not_found();
        }
        return next.run(request).await;
    }
    let Ok(Some(domain)) = domain(&state.pool).await else {
        return next.run(request).await;
    };
    let Some(slug) = domain.enabled.then(|| slug_of(&domain, &host)).flatten() else {
        return next.run(request).await;
    };
    let Some(owner) = own_host(&state.pool, slug).await else {
        return plain_not_found();
    };
    let reading = request.method() == Method::GET || request.method() == Method::HEAD;
    if let Some(rest) = path.strip_prefix(PORTAL_API) {
        let account = portal_account(&state.pool, owner.user).await;
        if account
            .is_some_and(|account| rest.split('/').next() == Some(account.to_string().as_str()))
        {
            return next.run(request).await;
        }
        return plain_not_found();
    }
    if reading && path.starts_with("/assets/") {
        return next.run(request).await;
    }
    let requested = path.trim_start_matches('/');
    if reading && requested == format!("{}/login", owner.path) {
        // The sign-in page is the panel's page; its address is what keeps it private.
        *request.uri_mut() = Uri::from_static("/");
        return next.run(request).await;
    }
    if !reading || requested != owner.path {
        return plain_not_found();
    }
    if let Some(renderer) = owner.renderer {
        let response = relay(
            &renderer,
            &owner.token,
            request.uri().query(),
            request.headers(),
        )
        .await;
        // The renderer chose the format and the nodes; the fetch itself is still the
        // user's, so it is kept like any other. A failed record never costs the client
        // its subscription.
        let result = if response.status().is_success() {
            "relayed"
        } else {
            "unavailable"
        };
        if let Err(error) =
            super::subscriptions::record_fetch(&state, owner.user, "renderer", None, result).await
        {
            tracing::warn!(?error, "relayed subscription fetch not recorded");
        }
        return response;
    }
    let query = query_of(request.uri());
    let headers = request.headers().clone();
    match super::subscriptions::serve(&state, &headers, owner.token, query).await {
        Ok(response) => response,
        Err(ApiError::NotFound) => plain_not_found(),
        Err(error) => error.into_response(),
    }
}

/// The origin and RP ID a portal request verifies Passkeys for, or `None` for the
/// panel's own default. A user's own hostname is not a subdomain of the panel, so
/// it uses the zone; a reset, which changes the hostname, then leaves Passkeys
/// working. When the panel itself lives under that zone, its own host uses the
/// zone as well, so a key bound through either address signs in on both.
fn portal_scope(
    public_url: &str,
    request_host: Option<&str>,
    domain: &Domain,
) -> Option<(String, String)> {
    if !domain.enabled {
        return None;
    }
    let panel = panel_host(public_url).to_ascii_lowercase();
    let zone = domain.zone_name.to_ascii_lowercase();
    match request_host {
        Some(host) if host != panel => {
            slug_of(domain, host).map(|_| (format!("https://{host}"), domain.zone_name.clone()))
        }
        _ => (panel == zone || panel.ends_with(&format!(".{zone}")))
            .then(|| (public_url.to_owned(), domain.zone_name.clone())),
    }
}

/// Portal requests verify Passkeys under the scope `portal_scope` chooses.
pub async fn scoped_state(state: AppState, headers: &HeaderMap) -> AppState {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .rsplit_once(':')
                .filter(|(_, port)| port.bytes().all(|byte| byte.is_ascii_digit()))
                .map_or(value, |(host, _)| host)
                .to_ascii_lowercase()
        });
    let Ok(Some(domain)) = domain(&state.pool).await else {
        return state;
    };
    let Some((origin, rp_id)) = portal_scope(&state.config.public_url, host.as_deref(), &domain)
    else {
        return state;
    };
    let mut scoped = state;
    scoped.passkeys = std::sync::Arc::new(crate::passkeys::Service::scoped(&origin, &rp_id));
    scoped
}

const RELAYED_HEADERS: [&str; 6] = [
    "content-type",
    "content-disposition",
    "subscription-userinfo",
    "profile-update-interval",
    "profile-web-page-url",
    "profile-title",
];
const RELAY_LIMIT: usize = 16 * 1024 * 1024;
static RELAY: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap_or_default()
});

/// Answers from the retained renderer, which still holds this user's nodes. The client
/// sees only its own address; the client's User-Agent still picks the format there.
async fn relay(renderer: &str, token: &str, query: Option<&str>, headers: &HeaderMap) -> Response {
    let mut url = format!("{renderer}{token}");
    if let Some(query) = query {
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str(query);
    }
    let mut upstream = RELAY.get(url);
    if let Some(agent) = headers.get(header::USER_AGENT) {
        upstream = upstream.header(header::USER_AGENT, agent);
    }
    let unavailable = || {
        (
            StatusCode::BAD_GATEWAY,
            [(header::CACHE_CONTROL, "no-store")],
            Body::empty(),
        )
            .into_response()
    };
    let Ok(mut answer) = upstream.send().await else {
        return unavailable();
    };
    let mut response = Response::builder()
        .status(answer.status())
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    for name in RELAYED_HEADERS {
        if let Some(value) = answer.headers().get(name) {
            response = response.header(name, value);
        }
    }
    let mut body = Vec::new();
    loop {
        match answer.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() <= RELAY_LIMIT => {
                body.extend_from_slice(&chunk)
            }
            Ok(None) => break,
            _ => return unavailable(),
        }
    }
    response
        .body(Body::from(body))
        .unwrap_or_else(|_| unavailable())
}

fn query_of(uri: &Uri) -> super::subscriptions::SubscriptionQuery {
    Query::try_from_uri(uri)
        .map(|Query(query)| query)
        .unwrap_or_default()
}

/// Gives a user their first host. Safe to call repeatedly.
async fn assign(tx: &mut Transaction<'_, Postgres>, user: i64) -> ApiResult<()> {
    sqlx::query("INSERT INTO singbox_subscription_hosts(user_id,slug,path_token,updated_at) VALUES($1,$2,$3,$4) ON CONFLICT (user_id) DO NOTHING")
        .bind(user)
        .bind(random_slug())
        .bind(random_token())
        .bind(now_timestamp())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// New hostname and path for one user; the old pair stops answering when this commits.
pub async fn rotate(tx: &mut Transaction<'_, Postgres>, user: i64) -> ApiResult<()> {
    let enabled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_subscription_domain WHERE enabled)",
    )
    .fetch_one(&mut **tx)
    .await?;
    retire(tx, user).await?;
    if enabled {
        assign(tx, user).await?;
    }
    Ok(())
}

/// Forgets a user's host and queues its DNS record for deletion.
pub async fn retire(tx: &mut Transaction<'_, Postgres>, user: i64) -> ApiResult<()> {
    sqlx::query("INSERT INTO singbox_subscription_host_retired(record_id,zone_id,retired_at) SELECT h.record_id,d.zone_id,$2 FROM singbox_subscription_hosts h CROSS JOIN singbox_subscription_domain d WHERE h.user_id=$1 AND h.record_id IS NOT NULL ON CONFLICT DO NOTHING")
        .bind(user)
        .bind(now_timestamp())
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM singbox_subscription_hosts WHERE user_id=$1")
        .bind(user)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Asks the worker to look now, and waits briefly so an address returned right after a
/// reset already works; anything left is retried in the background.
pub async fn settle(pool: &PgPool) {
    if let Err(error) = tokio::time::timeout(
        Duration::from_secs(8),
        reconcile(pool, &DnsApi::production()),
    )
    .await
    {
        tracing::debug!(%error, "subscription host reconcile continues in the background");
    }
    WAKE.notify_one();
}

/// The few Cloudflare calls this feature needs.
pub struct DnsApi {
    client: Client,
    base: String,
}

#[derive(Deserialize)]
struct Envelope<T> {
    success: bool,
    #[serde(default)]
    errors: Vec<ApiFailure>,
    result: Option<T>,
}
#[derive(Deserialize)]
struct ApiFailure {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}
#[derive(Deserialize)]
struct RecordId {
    id: String,
}
#[derive(Deserialize)]
struct Zone {
    name: String,
}

impl DnsApi {
    pub fn production() -> Self {
        Self::new(CLOUDFLARE_API)
    }
    pub fn new(base: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
        }
    }
    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Option<T>, String> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "无法连接 Cloudflare".to_owned())?;
        let envelope: Envelope<T> = response
            .json()
            .await
            .map_err(|_| "Cloudflare 返回了无法识别的内容".to_owned())?;
        if envelope.success {
            return Ok(envelope.result);
        }
        Err(envelope.errors.first().map_or_else(
            || "Cloudflare 拒绝了请求".to_owned(),
            |failure| format!("Cloudflare {}：{}", failure.code, failure.message),
        ))
    }
    async fn zone_name(&self, token: &str, zone: &str) -> Result<String, String> {
        let zone: Option<Zone> = self
            .call(token, reqwest::Method::GET, &format!("/zones/{zone}"), None)
            .await?;
        zone.map(|zone| zone.name)
            .ok_or_else(|| "Cloudflare 没有返回这个域名".to_owned())
    }
    async fn create(&self, token: &str, domain: &Domain, slug: &str) -> Result<String, String> {
        let name = host_name(domain, slug);
        let created: Option<RecordId> = self
            .call(token, reqwest::Method::POST, &format!("/zones/{}/dns_records", domain.zone_id), Some(json!({"type":"CNAME","name":name,"content":domain.target,"proxied":true,"ttl":1,"comment":RECORD_COMMENT})))
            .await?;
        created
            .map(|record| record.id)
            .ok_or_else(|| "Cloudflare 没有返回记录标识".to_owned())
    }
    async fn delete(&self, token: &str, zone: &str, record: &str) -> Result<(), String> {
        match self
            .call::<RecordId>(
                token,
                reqwest::Method::DELETE,
                &format!("/zones/{zone}/dns_records/{record}"),
                None,
            )
            .await
        {
            Ok(_) => Ok(()),
            // Already gone: 81044 is Cloudflare's "record does not exist".
            Err(error) if error.contains("81044") => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// One pass of at most one batch; the worker calls this until nothing is left.
pub async fn reconcile(pool: &PgPool, dns: &DnsApi) -> ApiResult<()> {
    let Some(domain) = domain(pool).await?.filter(|domain| domain.enabled) else {
        return Ok(());
    };
    let users: Vec<i64> = sqlx::query_scalar("SELECT u.id FROM users u WHERE u.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM singbox_subscription_hosts h WHERE h.user_id=u.id) ORDER BY u.id LIMIT $1")
        .bind(BATCH)
        .fetch_all(pool)
        .await?;
    for user in users {
        let mut tx = pool.begin().await?;
        assign(&mut tx, user).await?;
        tx.commit().await?;
    }
    let pending: Vec<(i64, String)> = sqlx::query_as("SELECT user_id,slug FROM singbox_subscription_hosts WHERE state<>'ready' ORDER BY updated_at LIMIT $1")
        .bind(BATCH)
        .fetch_all(pool)
        .await?;
    let retired: Vec<(String, String)> = sqlx::query_as("SELECT record_id,zone_id FROM singbox_subscription_host_retired ORDER BY retired_at LIMIT $1")
        .bind(BATCH)
        .fetch_all(pool)
        .await?;
    if pending.is_empty() && retired.is_empty() {
        return Ok(());
    }
    // Resolved only when there is work, because each use of a credential is audited.
    let secret = control_center::credentials::resolve_reference_pool(
        pool,
        domain.credential_id,
        "dns",
        "subscription-domain",
    )
    .await?;
    let token = secret["api_token"].as_str().unwrap_or_default().to_owned();
    for (user, slug) in pending {
        let (state, record, error) = match dns.create(&token, &domain, &slug).await {
            Ok(record) => ("ready", Some(record), None),
            Err(error) => ("failed", None, Some(error)),
        };
        // The slug guards against a reset that happened while the call was running.
        sqlx::query("UPDATE singbox_subscription_hosts SET state=$3,record_id=COALESCE($4,record_id),last_error=$5,updated_at=$6 WHERE user_id=$1 AND slug=$2")
            .bind(user)
            .bind(&slug)
            .bind(state)
            .bind(record)
            .bind(error)
            .bind(now_timestamp())
            .execute(pool)
            .await?;
    }
    for (record, zone) in retired {
        if let Err(error) = dns.delete(&token, &zone, &record).await {
            tracing::warn!(%error, "subscription host DNS record not deleted yet");
            continue;
        }
        sqlx::query("DELETE FROM singbox_subscription_host_retired WHERE record_id=$1")
            .bind(record)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn run(state: AppState) {
    let dns = DnsApi::production();
    loop {
        if let Err(error) = reconcile(&state.pool, &dns).await {
            tracing::warn!(%error, "subscription host reconcile failed");
        }
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(30)) => {}
            () = WAKE.notified() => {}
        }
    }
}

#[derive(Serialize)]
struct DomainView {
    configured: bool,
    enabled: bool,
    credential_id: Option<Uuid>,
    zone_id: String,
    zone_name: String,
    host_prefix: String,
    target: String,
    legacy_paths: bool,
    revision: i64,
    hosts: HostCounts,
}
#[derive(Serialize, FromRow)]
struct HostCounts {
    total: i64,
    ready: i64,
    failed: i64,
    last_error: Option<String>,
}

pub async fn get_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    crate::auth::require_admin(&state, &headers).await?;
    let hosts: HostCounts = sqlx::query_as("SELECT count(*) AS total,count(*) FILTER (WHERE state='ready') AS ready,count(*) FILTER (WHERE state='failed') AS failed,(SELECT last_error FROM singbox_subscription_hosts WHERE last_error IS NOT NULL ORDER BY updated_at DESC LIMIT 1) AS last_error FROM singbox_subscription_hosts")
        .fetch_one(&state.pool)
        .await?;
    let saved = domain(&state.pool).await?;
    Ok(Json(json!(DomainView {
        configured: saved.is_some(),
        enabled: saved.as_ref().is_some_and(|domain| domain.enabled),
        credential_id: saved.as_ref().map(|domain| domain.credential_id),
        zone_id: saved
            .as_ref()
            .map(|domain| domain.zone_id.clone())
            .unwrap_or_default(),
        zone_name: saved
            .as_ref()
            .map(|domain| domain.zone_name.clone())
            .unwrap_or_default(),
        host_prefix: saved
            .as_ref()
            .map_or_else(|| "apsp-".to_owned(), |domain| domain.host_prefix.clone()),
        target: saved
            .as_ref()
            .map(|domain| domain.target.clone())
            .unwrap_or_default(),
        legacy_paths: saved.as_ref().is_none_or(|domain| domain.legacy_paths),
        revision: saved.as_ref().map_or(0, |domain| domain.revision),
        hosts,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsWrite {
    enabled: bool,
    credential_id: Uuid,
    zone_id: String,
    host_prefix: String,
    target: String,
    legacy_paths: bool,
    revision: i64,
}

fn hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.contains('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

pub async fn put_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SettingsWrite>,
) -> ApiResult<Json<Value>> {
    control_center::require_recent_proof(&state, &headers).await?;
    control_center::require_credential_choice(&state, &headers, true).await?;
    let prefix = input.host_prefix.trim().to_ascii_lowercase();
    let target = input.target.trim().to_ascii_lowercase();
    let zone = input.zone_id.trim().to_ascii_lowercase();
    if prefix.is_empty()
        || prefix.len() > 20
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || prefix.starts_with('-')
    {
        return Err(ApiError::BadRequest(
            "前缀只能使用小写字母、数字和连字符，最长 20 个字符".into(),
        ));
    }
    if !hostname(&target) {
        return Err(ApiError::BadRequest("解析目标须是完整的主机名".into()));
    }
    if zone.is_empty() || !zone.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::BadRequest("Zone ID 格式无效".into()));
    }
    let secret = control_center::credentials::resolve_reference(
        &state,
        input.credential_id,
        "dns",
        "subscription-domain",
    )
    .await?;
    let token = secret["api_token"].as_str().unwrap_or_default();
    // Asking Cloudflare proves the token can see this zone and gives its real name.
    let zone_name = DnsApi::production()
        .zone_name(token, &zone)
        .await
        .map_err(ApiError::Conflict)?;
    let now = now_timestamp();
    let changed = sqlx::query("INSERT INTO singbox_subscription_domain(enabled,credential_id,zone_id,zone_name,host_prefix,target,legacy_paths,updated_at) SELECT $1,$2,$3,$4,$5,$6,$7,$8 WHERE $9=0 ON CONFLICT (singleton) DO UPDATE SET enabled=$1,credential_id=$2,zone_id=$3,zone_name=$4,host_prefix=$5,target=$6,legacy_paths=$7,updated_at=$8,revision=singbox_subscription_domain.revision+1 WHERE singbox_subscription_domain.revision=$9")
        .bind(input.enabled)
        .bind(input.credential_id)
        .bind(&zone)
        .bind(&zone_name)
        .bind(&prefix)
        .bind(&target)
        .bind(input.legacy_paths)
        .bind(now)
        .bind(input.revision)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "订阅域名设置已被修改，请刷新后重试".into(),
        ));
    }
    WAKE.notify_one();
    get_settings(State(state), headers).await
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    fn domain(enabled: bool) -> Domain {
        Domain {
            enabled,
            credential_id: Uuid::nil(),
            zone_id: "zone".into(),
            zone_name: "example.test".into(),
            host_prefix: "apsp-".into(),
            target: "tunnel.example.test".into(),
            legacy_paths: true,
            revision: 1,
        }
    }

    #[test]
    fn panel_under_the_zone_shares_the_zone_with_personal_hosts() {
        let panel = "https://panel.example.test";
        let own = "apsp-abcdefghijklmnop.example.test";
        let zone = || "example.test".to_owned();
        assert_eq!(
            portal_scope(panel, Some("panel.example.test"), &domain(true)),
            Some((panel.to_owned(), zone()))
        );
        assert_eq!(
            portal_scope(panel, None, &domain(true)),
            Some((panel.to_owned(), zone()))
        );
        assert_eq!(
            portal_scope(panel, Some(own), &domain(true)),
            Some((format!("https://{own}"), zone()))
        );
        // Neither another subdomain nor a disabled feature changes the default.
        assert_eq!(
            portal_scope(panel, Some("other.example.test"), &domain(true)),
            None
        );
        assert_eq!(
            portal_scope(panel, Some("panel.example.test"), &domain(false)),
            None
        );
        assert_eq!(portal_scope(panel, Some(own), &domain(false)), None);
    }

    #[test]
    fn panel_outside_the_zone_keeps_its_own_rp_id() {
        let panel = "https://panel.elsewhere.test";
        assert_eq!(
            portal_scope(panel, Some("panel.elsewhere.test"), &domain(true)),
            None
        );
        // A user's own host still verifies under the zone.
        let own = "apsp-abcdefghijklmnop.example.test";
        assert_eq!(
            portal_scope(panel, Some(own), &domain(true)),
            Some((format!("https://{own}"), "example.test".to_owned()))
        );
        // A look-alike zone suffix is not the zone.
        assert_eq!(
            portal_scope("https://notexample.test", None, &domain(true)),
            None
        );
    }
}
