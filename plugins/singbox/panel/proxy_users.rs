use crate::{
    AppState,
    auth::{random_token, require_admin},
    business,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(FromRow)]
struct ProxyUserRow {
    id: i64,
    name: String,
    subscription_token: String,
}

#[derive(Serialize)]
pub struct ProxyUserView {
    pub id: i64,
    pub name: String,
    pub subscription_token: String,
    pub subscription_url: String,
    pub credentials_redacted: bool,
    pub credential_access_reason: &'static str,
}
impl ProxyUserRow {
    fn view(self, public_url: &str, reveal: bool, host: Option<String>) -> ProxyUserView {
        ProxyUserView {
            id: self.id,
            name: self.name,
            subscription_url: if reveal {
                host.unwrap_or_else(|| format!("{public_url}/sub/{}", self.subscription_token))
            } else {
                String::new()
            },
            subscription_token: if reveal {
                self.subscription_token
            } else {
                String::new()
            },
            credentials_redacted: !reveal,
            credential_access_reason: super::secret_access::REASON,
        }
    }
}

/// One line of the user list: who may use what and whether they can sign in.
/// It carries no credentials, so reading it needs no recent administrator proof.
#[derive(Serialize, FromRow)]
pub struct ProxyUserOverview {
    pub id: i64,
    pub status: String,
    pub allowed: bool,
    pub package_name: Option<String>,
    pub monthly_bytes: Option<String>,
    pub used_bytes: String,
    pub expires_at: Option<i64>,
    pub group_ids: Vec<i64>,
    pub external_nodes: i64,
    /// The retained legacy renderer still decides this user's nodes.
    pub bridged: bool,
    pub portal_created: bool,
    pub portal_keys: i64,
    pub activation_expires_at: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyUserRequest {
    pub name: String,
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<ProxyUserView>>> {
    require_admin(&state, &headers).await?;
    let reveal = super::secret_access::may_reveal(&state, &headers).await?;
    let users = sqlx::query_as::<_, ProxyUserRow>(
        "SELECT id,name,subscription_token FROM users WHERE deleted_at IS NULL ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await?;
    if reveal {
        super::secret_access::audit_read(&state,&headers,None,"security_subscription_addresses_read",serde_json::json!({"source":"user_list","user_ids":users.iter().map(|user|user.id).collect::<Vec<_>>(),"credential_values_recorded":false})).await?;
    }
    let mut hosts = if reveal {
        super::subscription_hosts::addresses(&state.pool).await?
    } else {
        Default::default()
    };
    Ok(Json(
        users
            .into_iter()
            .map(|user| {
                let host = hosts.remove(&user.id);
                user.view(&state.config.public_url, reveal, host)
            })
            .collect(),
    ))
}

pub async fn overview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<ProxyUserOverview>>> {
    require_admin(&state, &headers).await?;
    let rows = sqlx::query_as::<_, ProxyUserOverview>(
        "SELECT e.user_id AS id,e.status,e.allowed,e.package_name,e.monthly_bytes,e.used_bytes,e.expires_at,\
         COALESCE((SELECT array_agg(p.group_id ORDER BY p.group_id) FROM singbox_user_policies p WHERE p.user_id=e.user_id),'{}'::BIGINT[]) AS group_ids,\
         (SELECT count(DISTINCT x.external_node_id) FROM singbox_user_policies p JOIN singbox_policy_external_nodes x ON x.group_id=p.group_id WHERE p.user_id=e.user_id) AS external_nodes,\
         EXISTS(SELECT 1 FROM singbox_nexus_identities i JOIN singbox_nexus_bridge b ON b.import_id=i.import_id WHERE b.enabled AND i.user_id=e.user_id) AS bridged,\
         portal.account_id IS NOT NULL AS portal_created,\
         (SELECT count(*) FROM passkey_credentials c WHERE c.account_id=portal.account_id) AS portal_keys,\
         CASE WHEN portal.activation_expires_at>$1 THEN portal.activation_expires_at END AS activation_expires_at \
         FROM singbox_entitlements($1) e LEFT JOIN singbox_portal_accounts portal ON portal.user_id=e.user_id ORDER BY e.user_id",
    )
    .bind(now_timestamp())
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<ProxyUserView>> {
    require_admin(&state, &headers).await?;
    let reveal = super::secret_access::may_reveal(&state, &headers).await?;
    let user = sqlx::query_as::<_, ProxyUserRow>(
        "SELECT id,name,subscription_token FROM users WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;
    if reveal {
        super::secret_access::audit_read(
            &state,
            &headers,
            Some(id),
            "security_subscription_addresses_read",
            serde_json::json!({"source":"user_detail","credential_values_recorded":false}),
        )
        .await?;
    }
    let host = if reveal {
        super::subscription_hosts::address(&state.pool, id).await?
    } else {
        None
    };
    Ok(Json(user.view(&state.config.public_url, reveal, host)))
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ProxyUserRequest>,
) -> ApiResult<(StatusCode, Json<ProxyUserView>)> {
    require_admin(&state, &headers).await?;
    let reveal = super::secret_access::may_reveal(&state, &headers).await?;
    let name = business::name(&request.name)?;
    let user = sqlx::query_as::<_, ProxyUserRow>("INSERT INTO users(name,subscription_token) VALUES($1,$2) RETURNING id,name,subscription_token").bind(name).bind(random_token()).fetch_one(&state.pool).await?;
    if reveal {
        super::secret_access::audit_read(
            &state,
            &headers,
            Some(user.id),
            "security_subscription_addresses_read",
            serde_json::json!({"source":"user_create_response","credential_values_recorded":false}),
        )
        .await?;
    }
    super::subscription_hosts::settle(&state.pool).await;
    let host = if reveal {
        super::subscription_hosts::address(&state.pool, user.id).await?
    } else {
        None
    };
    Ok((
        StatusCode::CREATED,
        Json(user.view(&state.config.public_url, reveal, host)),
    ))
}

pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(request): Json<ProxyUserRequest>,
) -> ApiResult<Json<ProxyUserView>> {
    require_admin(&state, &headers).await?;
    let reveal = super::secret_access::may_reveal(&state, &headers).await?;
    let name = business::name(&request.name)?;
    let mut transaction = state.pool.begin().await?;
    super::entitlements::lock(&mut transaction).await?;
    business::lock_user(&mut transaction, id).await?;
    let servers = business::lock_user_servers(&mut transaction, id).await?;
    let user = sqlx::query_as::<_, ProxyUserRow>(
        "UPDATE users SET name=$2 WHERE id=$1 RETURNING id,name,subscription_token",
    )
    .bind(id)
    .bind(name)
    .fetch_one(&mut *transaction)
    .await?;
    business::mark_dirty(&mut transaction, &servers).await?;
    transaction.commit().await?;
    if reveal {
        super::secret_access::audit_read(
            &state,
            &headers,
            Some(id),
            "security_subscription_addresses_read",
            serde_json::json!({"source":"user_update_response","credential_values_recorded":false}),
        )
        .await?;
    }
    let host = if reveal {
        super::subscription_hosts::address(&state.pool, id).await?
    } else {
        None
    };
    Ok(Json(user.view(&state.config.public_url, reveal, host)))
}

pub async fn reset_subscription(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<ProxyUserView>> {
    let administrator =
        crate::control_center::require_capability(&state, &headers, "proxy:write").await?;
    crate::control_center::require_recent_proof(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    super::business::lock_user(&mut tx, id).await?;
    let legacy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_bridge b JOIN singbox_nexus_identities i ON i.import_id=b.import_id WHERE b.enabled AND i.user_id=$1)").bind(id).fetch_one(&mut *tx).await?;
    if legacy {
        // The token stays with the retained renderer; the user's own address still rotates.
        let enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM singbox_subscription_domain WHERE enabled)",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !enabled {
            return Err(ApiError::Conflict(
                "原订阅兼容期间保留现有链接，请在节点接管后再重置".into(),
            ));
        }
        let user = sqlx::query_as::<_, ProxyUserRow>(
            "SELECT id,name,subscription_token FROM users WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
        super::subscription_hosts::rotate(&mut tx, id).await?;
        super::operations_workflows::event(
            &mut tx,
            Some(administrator),
            Some(id),
            "security_subscription_reset",
            serde_json::json!({"own_address_only":true}),
        )
        .await?;
        tx.commit().await?;
        super::subscription_hosts::settle(&state.pool).await;
        let host = super::subscription_hosts::address(&state.pool, id).await?;
        return Ok(Json(user.view(&state.config.public_url, true, host)));
    }
    let user = sqlx::query_as::<_, ProxyUserRow>(
        "UPDATE users SET subscription_token=$2 WHERE id=$1 AND deleted_at IS NULL RETURNING id,name,subscription_token",
    )
    .bind(id)
    .bind(random_token())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    super::operations_workflows::event(&mut tx, Some(administrator), Some(id), "security_subscription_reset", serde_json::json!({"node_credentials_rotated":false,"downloaded_credentials_revoked":false})).await?;
    super::subscription_hosts::rotate(&mut tx, id).await?;
    tx.commit().await?;
    super::subscription_hosts::settle(&state.pool).await;
    let host = super::subscription_hosts::address(&state.pool, id).await?;
    Ok(Json(user.view(&state.config.public_url, true, host)))
}

pub async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_admin(&state, &headers).await?;
    let mut transaction = state.pool.begin().await?;
    super::entitlements::lock(&mut transaction).await?;
    business::lock_user(&mut transaction, id).await?;
    let servers = business::lock_user_servers(&mut transaction, id).await?;
    sqlx::query("UPDATE users SET deleted_at=$2 WHERE id=$1")
        .bind(id)
        .bind(now_timestamp())
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM passkey_accounts WHERE id IN (SELECT account_id FROM singbox_portal_accounts WHERE user_id=$1)")
        .bind(id).execute(&mut *transaction).await?;
    sqlx::query("DELETE FROM accesses WHERE user_id=$1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM singbox_user_policies WHERE user_id=$1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    super::subscription_hosts::retire(&mut transaction, id).await?;
    business::mark_dirty(&mut transaction, &servers).await?;
    transaction.commit().await?;
    super::subscription_hosts::settle(&state.pool).await;
    Ok(StatusCode::NO_CONTENT)
}
