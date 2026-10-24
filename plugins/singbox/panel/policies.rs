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
use sqlx::{FromRow, Postgres, Row, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

const SELECT: &str = "SELECT g.id,g.name,ARRAY(SELECT node_id FROM singbox_policy_nodes WHERE group_id=g.id ORDER BY node_id) AS node_ids,ARRAY(SELECT chain_id FROM singbox_policy_chains WHERE group_id=g.id ORDER BY chain_id) AS chain_ids,(SELECT COUNT(*) FROM singbox_user_policies p JOIN users u ON u.id=p.user_id AND u.deleted_at IS NULL WHERE p.group_id=g.id) AS member_count FROM singbox_policy_groups g";

#[derive(Serialize, FromRow)]
pub struct Policy {
    pub id: i64,
    pub name: String,
    pub node_ids: Vec<i64>,
    pub chain_ids: Vec<i64>,
    pub member_count: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRequest {
    pub name: String,
    pub node_ids: Vec<i64>,
    pub chain_ids: Vec<i64>,
}

pub(crate) fn ids(values: &[i64]) -> ApiResult<Vec<i64>> {
    if values.len() > 1024 || values.iter().any(|id| *id <= 0) {
        return Err(ApiError::BadRequest(
            "最多选择 1024 项，编号必须是正整数".into(),
        ));
    }
    Ok(values
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Policy>>> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        sqlx::query_as(&format!("{SELECT} ORDER BY g.id"))
            .fetch_all(&state.pool)
            .await?,
    ))
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PolicyRequest>,
) -> ApiResult<(StatusCode, Json<Policy>)> {
    require_admin(&state, &headers).await?;
    Ok((
        StatusCode::CREATED,
        Json(save(&state, None, request).await?),
    ))
}

pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(request): Json<PolicyRequest>,
) -> ApiResult<Json<Policy>> {
    require_admin(&state, &headers).await?;
    Ok(Json(save(&state, Some(id), request).await?))
}

async fn save(state: &AppState, id: Option<i64>, request: PolicyRequest) -> ApiResult<Policy> {
    let name = super::business::name(&request.name)?;
    let nodes = ids(&request.node_ids)?;
    let chains = ids(&request.chain_ids)?;
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    let valid_nodes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes n JOIN servers s ON s.id=n.server_id WHERE n.id=ANY($1) AND n.deleted_at IS NULL AND s.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM singbox_chains c WHERE c.entry_node_id=n.id AND (c.deleted_at IS NULL OR (c.path_kind='ordered' AND c.phase<>'retired')))")
        .bind(&nodes).fetch_one(&mut *tx).await?;
    let mut valid_chains: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM singbox_live_chains c WHERE c.id=ANY($1) AND (singbox_path_resources_available(c.id) OR EXISTS(SELECT 1 FROM singbox_policy_chains p WHERE p.chain_id=c.id AND p.group_id=$2))")
        .bind(&chains).bind(id).fetch_one(&mut *tx).await?;
    let ordered: Vec<(i64, bool)> = sqlx::query_as("SELECT c.id,EXISTS(SELECT 1 FROM singbox_policy_chains p WHERE p.chain_id=c.id AND p.group_id=$2) FROM singbox_chains c WHERE c.id=ANY($1) AND c.path_kind='ordered' AND c.deleted_at IS NULL ORDER BY c.id")
        .bind(&chains).bind(id).fetch_all(&mut *tx).await?;
    for (chain, retained) in ordered {
        if retained || super::ordered_paths::chain_is_structurally_available(&mut tx, chain).await?
        {
            valid_chains += 1;
        }
    }
    if valid_nodes != nodes.len() as i64 || valid_chains != chains.len() as i64 {
        return Err(ApiError::BadRequest(
            "所选节点或链路不可用；链路入口必须通过链路授权".into(),
        ));
    }
    let id: i64 = if let Some(id) = id {
        sqlx::query_scalar("UPDATE singbox_policy_groups SET name=$2 WHERE id=$1 RETURNING id")
            .bind(id)
            .bind(name)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?
    } else {
        sqlx::query_scalar("INSERT INTO singbox_policy_groups(name) VALUES($1) RETURNING id")
            .bind(name)
            .fetch_one(&mut *tx)
            .await?
    };
    let members: Vec<i64> = sqlx::query_scalar("SELECT p.user_id FROM singbox_user_policies p JOIN users u ON u.id=p.user_id AND u.deleted_at IS NULL WHERE p.group_id=$1 ORDER BY p.user_id")
        .bind(id).fetch_all(&mut *tx).await?;
    sqlx::query("DELETE FROM singbox_policy_nodes WHERE group_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM singbox_policy_chains WHERE group_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO singbox_policy_nodes SELECT $1,unnest($2::bigint[])")
        .bind(id)
        .bind(&nodes)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO singbox_policy_chains SELECT $1,unnest($2::bigint[])")
        .bind(id)
        .bind(&chains)
        .execute(&mut *tx)
        .await?;
    sync_users(&mut tx, &members).await?;
    let result = sqlx::query_as(&format!("{SELECT} WHERE g.id=$1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    let assigned: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_user_policies WHERE group_id=$1)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if assigned {
        return Err(ApiError::Conflict(
            "请先取消用户的策略组分配，再删除此组".into(),
        ));
    }
    if sqlx::query("DELETE FROM singbox_policy_groups WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 0
    {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UserPolicies {
    pub group_ids: Vec<i64>,
}

pub async fn user_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<UserPolicies>> {
    require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    super::business::lock_user(&mut tx, id).await?;
    let group_ids = sqlx::query_scalar(
        "SELECT group_id FROM singbox_user_policies WHERE user_id=$1 ORDER BY group_id",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(UserPolicies { group_ids }))
}

pub async fn user_set(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(request): Json<UserPolicies>,
) -> ApiResult<Json<UserPolicies>> {
    require_admin(&state, &headers).await?;
    let groups = ids(&request.group_ids)?;
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    super::business::lock_user(&mut tx, id).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM singbox_policy_groups WHERE id=ANY($1)")
            .bind(&groups)
            .fetch_one(&mut *tx)
            .await?;
    if count != groups.len() as i64 {
        return Err(ApiError::BadRequest("策略组不存在".into()));
    }
    sqlx::query("DELETE FROM singbox_user_policies WHERE user_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO singbox_user_policies(user_id,group_id) SELECT $1,unnest($2::bigint[])",
    )
    .bind(id)
    .bind(&groups)
    .execute(&mut *tx)
    .await?;
    sync_users(&mut tx, &[id]).await?;
    tx.commit().await?;
    Ok(Json(UserPolicies { group_ids: groups }))
}

/// Materialize credentials for the union of direct grants and all policy groups.
/// Removing one overlapping source preserves credentials; final revocation deletes them.
pub(crate) async fn sync_users(tx: &mut Transaction<'_, Postgres>, users: &[i64]) -> ApiResult<()> {
    if users.is_empty() {
        return Ok(());
    }
    sqlx::query("SELECT id FROM users WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(users)
        .fetch_all(&mut **tx)
        .await?;
    let servers: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT n.server_id FROM nodes n JOIN (SELECT node_id FROM accesses WHERE user_id=ANY($1) UNION SELECT node_id FROM singbox_desired_accesses WHERE user_id=ANY($1)) a ON a.node_id=n.id ORDER BY n.server_id")
        .bind(users).fetch_all(&mut **tx).await?;
    super::business::mark_dirty(tx, &servers).await?;
    sqlx::query("DELETE FROM accesses a WHERE user_id=ANY($1) AND NOT direct_grant AND NOT EXISTS(SELECT 1 FROM singbox_desired_accesses d WHERE d.user_id=a.user_id AND d.node_id=a.node_id)")
        .bind(users).execute(&mut **tx).await?;
    let missing = sqlx::query("SELECT d.user_id,d.node_id,n.protocol_config FROM singbox_desired_accesses d JOIN nodes n ON n.id=d.node_id WHERE d.user_id=ANY($1) AND NOT EXISTS(SELECT 1 FROM accesses a WHERE a.user_id=d.user_id AND a.node_id=d.node_id) ORDER BY d.user_id,d.node_id")
        .bind(users).fetch_all(&mut **tx).await?;
    for row in missing {
        let user: i64 = row.get("user_id");
        let node: i64 = row.get("node_id");
        let config: apsp_compiler::ProtocolConfig =
            serde_json::from_value(row.get("protocol_config")).map_err(anyhow::Error::from)?;
        sqlx::query("INSERT INTO accesses(user_id,node_id,uuid,stat_name,direct_grant,credential) VALUES($1,$2,$3,$4,FALSE,$5)")
            .bind(user).bind(node).bind(Uuid::new_v4()).bind(apsp_compiler::stat_name(user,node))
            .bind(super::node_protocol::credential(config.credential_size())).execute(&mut **tx).await?;
    }
    Ok(())
}
