use super::{
    model::{Provider, domain, identifier},
    providers::RecordClient,
};
use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    pub name: String,
    pub provider: Provider,
    pub credential_id: Uuid,
    pub zone_ids: Vec<String>,
    /// Domain names of `zone_ids`, kept so people pick a domain instead of an identifier.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub zone_names: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub server_ids: Vec<i64>,
    pub enabled: bool,
}
#[derive(FromRow, Serialize)]
pub(super) struct Account {
    pub id: Uuid,
    #[sqlx(json)]
    pub config: Config,
    pub revision: i64,
    pub checked_at: Option<i64>,
    pub error_code: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Write {
    config: Config,
    revision: Option<i64>,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/naming/accounts", get(list).post(create))
        .route("/api/naming/accounts/cloudflare", post(connect_cloudflare))
        .route("/api/naming/accounts/access-key", post(connect_access_key))
        .route("/api/naming/accounts/{id}", put(update))
        .route("/api/naming/accounts/{id}/check", post(check))
}

pub(super) async fn load(pool: &PgPool, id: Uuid) -> ApiResult<Account> {
    sqlx::query_as("SELECT * FROM dns_accounts WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound)
}

pub(super) fn permits(
    actor: &control_center::Principal,
    config: &Config,
    capability: &str,
) -> bool {
    actor.allows(capability)
        && if config.server_ids.is_empty() {
            actor.global_servers()
        } else {
            config.server_ids.iter().all(|id| actor.allows_server(*id))
        }
}

pub(super) async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    config: &Config,
    capability: &str,
) -> ApiResult<()> {
    let actor = control_center::authenticate(state, headers).await?;
    if !permits(&actor, config, capability) {
        return Err(ApiError::Forbidden(
            "当前管理员没有此 DNS 账号完整授权范围的操作权限".into(),
        ));
    }
    Ok(())
}

async fn normalize(state: &AppState, headers: &HeaderMap, config: &mut Config) -> ApiResult<()> {
    normalize_scope(state, headers, config).await?;
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM credential_entries WHERE id=$1 AND kind='dns' AND enabled)",
    )
    .bind(config.credential_id)
    .fetch_one(&state.pool)
    .await?;
    if !valid {
        return Err(ApiError::Conflict(
            "DNS 凭据不存在、已停用或用途不匹配".into(),
        ));
    }
    Ok(())
}

/// Checks everything about an account except its credential reference.
async fn normalize_scope(
    state: &AppState,
    headers: &HeaderMap,
    config: &mut Config,
) -> ApiResult<()> {
    config.name = config.name.trim().into();
    if config.name.is_empty()
        || config.name.len() > 128
        || config.name.chars().any(char::is_control)
        || config.zone_ids.is_empty()
        || config.zone_ids.len() > 32
        || config.server_ids.len() > 32
    {
        return Err(ApiError::BadRequest(
            "账号名称、区域或服务器授权范围无效".into(),
        ));
    }
    for zone in &mut config.zone_ids {
        *zone = zone.trim().to_ascii_lowercase();
        let valid = match config.provider {
            Provider::Cloudflare | Provider::Huawei => identifier(zone),
            Provider::Aliyun | Provider::Tencent => {
                domain(zone).is_some_and(|name| name == *zone && !name.starts_with("*."))
            }
        };
        if !valid {
            return Err(ApiError::BadRequest("DNS 区域标识无效".into()));
        }
    }
    config.zone_ids.sort();
    config.zone_ids.dedup();
    config.server_ids.sort();
    config.server_ids.dedup();
    authorize(state, headers, config, "dns:write").await?;
    for id in &config.server_ids {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM servers WHERE id=$1)")
            .bind(id)
            .fetch_one(&state.pool)
            .await?;
        if !exists {
            return Err(ApiError::BadRequest("授权范围包含不存在的服务器".into()));
        }
    }
    Ok(())
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Vec<Value>>> {
    let actor = control_center::authenticate(&state, &headers).await?;
    if !actor.allows("dns:read") {
        return Err(ApiError::Forbidden("没有 DNS 读取权限".into()));
    }
    let accounts: Vec<Account> =
        sqlx::query_as("SELECT * FROM dns_accounts ORDER BY created_at,id")
            .fetch_all(&state.pool)
            .await?;
    let mut values = Vec::new();
    for account in accounts {
        if permits(&actor, &account.config, "dns:read") {
            let capabilities = super::dns_record_spec::capabilities(account.config.provider);
            let mut value = serde_json::to_value(account).map_err(anyhow::Error::from)?;
            value["record_management_available"] = true.into();
            value["record_capabilities"] = capabilities;
            value["unavailable_reason"] = Value::Null;
            values.push(value);
        }
    }
    Ok(Json(values))
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut input): Json<Write>,
) -> ApiResult<Json<Value>> {
    control_center::require_recent_proof(&state, &headers).await?;
    normalize(&state, &headers, &mut input.config).await?;
    // An account grants its credential over the listed zones; binding one is owner-only.
    control_center::require_credential_choice(&state, &headers, true).await?;
    let id = Uuid::new_v4();
    let now = apsp_protocol::now_timestamp();
    sqlx::query("INSERT INTO dns_accounts(id,config,created_at,updated_at) VALUES($1,$2,$3,$3)")
        .bind(id)
        .bind(json!(input.config))
        .bind(now)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!(load(&state.pool, id).await?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloudflareConnect {
    #[serde(default)]
    name: String,
    api_token: String,
}

#[derive(Deserialize)]
struct CloudflareZones {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    result: Vec<CloudflareZone>,
}
#[derive(Deserialize)]
struct CloudflareZone {
    id: String,
    name: String,
}

const CLOUDFLARE_ZONES: &str =
    "https://api.cloudflare.com/client/v4/zones?per_page=32&status=active";

/// The domains this token may manage; an empty list means it may manage none.
async fn cloudflare_zones(url: &str, token: &str) -> ApiResult<Vec<CloudflareZone>> {
    let refused = || {
        ApiError::Conflict("Cloudflare 没有接受这个令牌，请检查令牌是否完整、是否仍然有效".into())
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(anyhow::Error::from)?;
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| ApiError::Conflict("暂时无法连接 Cloudflare，请稍后重试".into()))?;
    let zones: CloudflareZones = response.json().await.map_err(|_| refused())?;
    if zones.success {
        Ok(zones.result)
    } else {
        Err(refused())
    }
}

/// Saves a Cloudflare token in one step: checks it, finds its domains, encrypts it
/// and registers an account every Cloudflare feature can then choose from a list.
async fn connect_cloudflare(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CloudflareConnect>,
) -> ApiResult<Json<Value>> {
    control_center::require_recent_proof(&state, &headers).await?;
    control_center::require_credential_choice(&state, &headers, true).await?;
    let token = super::model::token(input.api_token.trim())?;
    let zones = cloudflare_zones(CLOUDFLARE_ZONES, &token).await?;
    if zones.is_empty() {
        return Err(ApiError::Conflict(
            "这个令牌有效，但没有可管理的域名。请在 Cloudflare 给令牌加上 DNS 编辑权限和对应域名"
                .into(),
        ));
    }
    let name = if input.name.trim().is_empty() {
        format!("Cloudflare · {}", zones[0].name)
    } else {
        input.name.trim().to_owned()
    };
    let mut tx = state.pool.begin().await?;
    let credential_id = control_center::credentials::store_generated_on(
        &mut tx,
        &name,
        "dns",
        json!({"api_token":token}),
    )
    .await?;
    let config = Config {
        name,
        provider: Provider::Cloudflare,
        credential_id,
        zone_ids: zones.iter().map(|zone| zone.id.clone()).collect(),
        zone_names: zones.into_iter().map(|zone| (zone.id, zone.name)).collect(),
        server_ids: Vec::new(),
        enabled: true,
    };
    let id = Uuid::new_v4();
    let now = apsp_protocol::now_timestamp();
    sqlx::query("INSERT INTO dns_accounts(id,config,created_at,updated_at) VALUES($1,$2,$3,$3)")
        .bind(id)
        .bind(json!(config))
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!(load(&state.pool, id).await?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessKeyConnect {
    provider: Provider,
    #[serde(default)]
    name: String,
    access_key_id: String,
    access_key_secret: String,
    zone_ids: Vec<String>,
}

/// The access-key counterpart of `connect_cloudflare`: encrypts the key pair
/// and registers the account in one step, so nobody has to create a
/// credential elsewhere and paste its identifier.
async fn connect_access_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<AccessKeyConnect>,
) -> ApiResult<Json<Value>> {
    control_center::require_recent_proof(&state, &headers).await?;
    control_center::require_credential_choice(&state, &headers, true).await?;
    let label = match input.provider {
        Provider::Cloudflare => {
            return Err(ApiError::BadRequest(
                "Cloudflare 账号请使用 API 令牌添加".into(),
            ));
        }
        Provider::Tencent => ("tencent", "腾讯云 DNSPod"),
        Provider::Aliyun => ("aliyun", "阿里云 DNS"),
        Provider::Huawei => ("huawei", "华为云 DNS"),
    };
    let key = input.access_key_id.trim();
    let secret = input.access_key_secret.trim();
    if !crate::cloud_api::credential(key) || !crate::cloud_api::credential(secret) {
        return Err(ApiError::BadRequest(
            "请填写有效的访问密钥 ID 与 Secret".into(),
        ));
    }
    let mut config = Config {
        name: input.name,
        provider: input.provider,
        credential_id: Uuid::nil(),
        zone_ids: input.zone_ids,
        zone_names: Default::default(),
        server_ids: Vec::new(),
        enabled: true,
    };
    if config.name.trim().is_empty() {
        config.name = format!(
            "{} · {}",
            label.1,
            config
                .zone_ids
                .first()
                .map_or(String::new(), |zone| zone.trim().to_ascii_lowercase())
        );
    }
    normalize_scope(&state, &headers, &mut config).await?;
    let mut tx = state.pool.begin().await?;
    config.credential_id = control_center::credentials::store_generated_on(
        &mut tx,
        &config.name,
        "dns",
        json!({"provider":label.0,"access_key_id":key,"access_key_secret":secret}),
    )
    .await?;
    let id = Uuid::new_v4();
    let now = apsp_protocol::now_timestamp();
    sqlx::query("INSERT INTO dns_accounts(id,config,created_at,updated_at) VALUES($1,$2,$3,$3)")
        .bind(id)
        .bind(json!(config))
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!(load(&state.pool, id).await?)))
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(mut input): Json<Write>,
) -> ApiResult<Json<Value>> {
    control_center::require_recent_proof(&state, &headers).await?;
    let before = load(&state.pool, id).await?;
    authorize(&state, &headers, &before.config, "dns:write").await?;
    normalize(&state, &headers, &mut input.config).await?;
    let widened = before.config.credential_id != input.config.credential_id
        || before.config.provider != input.config.provider
        || input
            .config
            .zone_ids
            .iter()
            .any(|zone| !before.config.zone_ids.contains(zone));
    control_center::require_credential_choice(&state, &headers, widened).await?;
    if sqlx::query("UPDATE dns_accounts SET config=$2,revision=revision+1,updated_at=$3,checked_at=NULL,error_code=NULL WHERE id=$1 AND revision=$4")
        .bind(id).bind(json!(input.config)).bind(apsp_protocol::now_timestamp()).bind(input.revision)
        .execute(&state.pool).await?.rows_affected()!=1 { return Err(ApiError::Conflict("账号已修改，请刷新后重试".into())); }
    Ok(Json(json!(load(&state.pool, id).await?)))
}

pub(super) async fn client(pool: &PgPool, account: &Account) -> ApiResult<RecordClient> {
    if !account.config.enabled {
        return Err(ApiError::Conflict("DNS 账号已停用".into()));
    }
    let secret = control_center::credentials::resolve_reference_pool(
        pool,
        account.config.credential_id,
        "dns",
        &format!("dns-account:{}", account.id),
    )
    .await?;
    RecordClient::new(account.config.provider, &secret).map_err(failure)
}

pub(super) fn failure(error: super::cloudflare::Failure) -> ApiError {
    ApiError::Conflict(format!("DNS 提供方返回未确认结果：{}", error.code))
}

pub(super) async fn zone(client: &RecordClient, account: &Account, id: &str) -> ApiResult<String> {
    if !account.config.zone_ids.iter().any(|zone| zone == id) {
        return Err(ApiError::Forbidden("区域不在此账号的明确授权范围内".into()));
    }
    client.zone(id).await.map_err(failure)
}

async fn check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let account = load(&state.pool, id).await?;
    authorize(&state, &headers, &account.config, "dns:read").await?;
    let _permit = state
        .quality_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::Busy)?;
    let result = async {
        let client = client(&state.pool, &account).await?;
        let mut zones = Vec::new();
        for id in &account.config.zone_ids {
            match client.zone(id).await {
                Ok(name) => {
                    zones.push(json!({"id":id,"name":name,"available":true,"error_code":null}))
                }
                Err(error) => zones
                    .push(json!({"id":id,"name":null,"available":false,"error_code":error.code})),
            }
        }
        Ok::<_, ApiError>(zones)
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(super::REQUEST_BUDGET),
        result,
    )
    .await;
    let now = apsp_protocol::now_timestamp();
    let (zones, error) = match result {
        Ok(Ok(zones)) => {
            let failed = zones.iter().any(|zone| zone["available"] != true);
            (
                zones,
                if failed {
                    Some("zone_check_failed")
                } else {
                    None
                },
            )
        }
        Ok(Err(_)) => (Vec::new(), Some("connection_unconfirmed")),
        Err(_) => (Vec::new(), Some("request_timeout")),
    };
    sqlx::query("UPDATE dns_accounts SET checked_at=$2,error_code=$3 WHERE id=$1 AND revision=$4")
        .bind(id)
        .bind(now)
        .bind(error)
        .bind(account.revision)
        .execute(&state.pool)
        .await?;
    Ok(Json(
        json!({"zones":zones,"checked_at":now,"error_code":error,"provider":account.config.provider,"accepted":error.is_none(),"source":"provider_official_api"}),
    ))
}
