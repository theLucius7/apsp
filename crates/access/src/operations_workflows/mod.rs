mod client;
mod diagnosis;
mod mutations;
mod snapshots;
pub(super) use client::apply_definition;

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Operation {
    PolicyBatch {
        user_ids: Vec<i64>,
        group_ids: Vec<i64>,
    },
    ReplacePackage {
        user_id: i64,
        package_group_id: i64,
    },
    SetValidity {
        user_id: i64,
        until: Validity,
    },
    ResetQuota {
        user_id: i64,
    },
    RotateNodeCredentials {
        user_id: i64,
        node_ids: Vec<i64>,
    },
    MigrateNode {
        source_node_id: i64,
        candidate_node_id: i64,
    },
    Failover {
        source_chain_id: i64,
        alternate_chain_id: i64,
        group_ids: Vec<i64>,
        reason: String,
    },
}

/// The new expiry of a user's current package. The administrator states the exact moment
/// (`{"at": seconds}`) or no time limit (`"never"`); there is no default, so a request that
/// leaves the field out is rejected instead of silently removing the limit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Validity {
    At(i64),
    Never,
}

/// Stored as a package's expiry when it has none; `singbox_entitlements` reports it as NULL
/// and the compatibility bridge passes NULL on to the legacy panel.
pub(super) const NO_EXPIRY: i64 = 253_402_300_799;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/operations/preview", post(preview))
        .route("/operations/{id}/apply", post(apply))
        .route(
            "/operations/privacy",
            get(diagnosis::privacy).put(diagnosis::save_privacy),
        )
        .route("/users/{id}/diagnosis", get(diagnosis::user))
        .route(
            "/users/{id}/client-template",
            get(client::get).put(client::save),
        )
        .route("/users/{id}/compatibility", post(client::compatibility))
}

/// Keeps operation previews, events and deployment checks only as long as the privacy
/// settings allow.
pub async fn maintenance(state: &AppState) -> ApiResult<()> {
    let mut tx = state.pool.begin().await?;
    prune(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn event(
    tx: &mut Transaction<'_, Postgres>,
    administrator: Option<i64>,
    user: Option<i64>,
    action: &str,
    detail: Value,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO singbox_operation_events(administrator_id,user_id,action,detail,created_at) VALUES($1,$2,$3,$4,$5)")
        .bind(administrator).bind(user).bind(action).bind(detail).bind(apsp_protocol::now_timestamp()).execute(&mut **tx).await?;
    Ok(())
}

pub(super) async fn record_subscription(
    tx: &mut Transaction<'_, Postgres>,
    user: i64,
    format: &str,
    nodes: Option<(usize, usize)>,
    result: &str,
) -> ApiResult<()> {
    let (managed, external) = nodes.unzip();
    event(
        tx,
        None,
        Some(user),
        "subscription_fetch",
        json!({"format":format,"managed_nodes":managed,"external_nodes":external,"result":result}),
    )
    .await
}

pub(super) async fn prune(tx: &mut Transaction<'_, Postgres>) -> ApiResult<()> {
    sqlx::query("DELETE FROM singbox_operation_previews WHERE expires_at<$1 AND receipt IS NULL")
        .bind(apsp_protocol::now_timestamp())
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM singbox_operation_previews q USING singbox_operation_privacy p WHERE q.receipt IS NOT NULL AND q.created_at<$1-86400::bigint*LEAST(p.administrator_days,COALESCE((SELECT days FROM record_retention_policy WHERE kind='audit'),p.administrator_days))").bind(apsp_protocol::now_timestamp()).execute(&mut **tx).await?;
    // Deployment checks are traffic's records; how long administrators' records stay is
    // the privacy setting here.
    let administrator_days: Option<i32> = sqlx::query_scalar("SELECT LEAST(p.administrator_days,COALESCE((SELECT days FROM record_retention_policy WHERE kind='audit'),p.administrator_days)) FROM singbox_operation_privacy p")
        .fetch_optional(&mut **tx)
        .await?;
    if let Some(days) = administrator_days {
        apsp_traffic::runtime_workflows::prune_preflights(
            tx,
            apsp_protocol::now_timestamp() - 86400 * i64::from(days),
        )
        .await?;
    }
    sqlx::query("DELETE FROM singbox_operation_events e USING singbox_operation_privacy p WHERE e.created_at<$1-86400::bigint*CASE WHEN e.action='subscription_fetch' THEN LEAST(p.subscription_days,COALESCE((SELECT days FROM record_retention_policy WHERE kind='proxy-access'),p.subscription_days)) WHEN e.action LIKE 'security_%' THEN LEAST(p.security_days,COALESCE((SELECT days FROM record_retention_policy WHERE kind='audit'),p.security_days)) ELSE LEAST(p.administrator_days,COALESCE((SELECT days FROM record_retention_policy WHERE kind='audit'),p.administrator_days)) END").bind(apsp_protocol::now_timestamp()).execute(&mut **tx).await?;
    Ok(())
}

async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Operation>,
) -> ApiResult<Json<Value>> {
    let administrator =
        crate::control_center::require_capability(&state, &headers, "proxy:write").await?;
    authorize(&state, &headers, &request).await?;
    let mut tx = state.pool.begin().await?;
    apsp_traffic::business::lock_publication(&mut tx).await?;
    let (fingerprint, summary) = snapshots::snapshot(&mut tx, &request).await?;
    let id = Uuid::new_v4();
    let now = apsp_protocol::now_timestamp();
    prune(&mut tx).await?;
    sqlx::query("INSERT INTO singbox_operation_previews(id,administrator_id,request,fingerprint,summary,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(id).bind(administrator).bind(serde_json::to_value(&request).map_err(anyhow::Error::from)?).bind(fingerprint).bind(&summary).bind(now).bind(now+600).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"expires_at":now+600,"summary":summary}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    confirm: bool,
}

async fn apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Confirmation>,
) -> ApiResult<Json<Value>> {
    let administrator =
        crate::control_center::require_capability(&state, &headers, "proxy:write").await?;
    crate::control_center::require_recent_proof(&state, &headers).await?;
    if !request.confirm {
        return Err(ApiError::BadRequest("请先确认预览中的影响".into()));
    }
    let mut tx = state.pool.begin().await?;
    apsp_traffic::business::lock_publication(&mut tx).await?;
    let row = sqlx::query("SELECT request,fingerprint,summary,expires_at,receipt FROM singbox_operation_previews WHERE id=$1 AND administrator_id=$2 FOR UPDATE")
        .bind(id).bind(administrator).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    if let Some(receipt) = row.get::<Option<Value>, _>("receipt") {
        tx.commit().await?;
        return Ok(Json(receipt));
    }
    if row.get::<i64, _>("expires_at") <= apsp_protocol::now_timestamp() {
        return Err(ApiError::Conflict("预览已过期，请重新预览".into()));
    }
    let operation: Operation =
        serde_json::from_value(row.get("request")).map_err(anyhow::Error::from)?;
    authorize(&state, &headers, &operation).await?;
    let (fingerprint, _) = snapshots::snapshot(&mut tx, &operation).await?;
    if fingerprint != row.get::<String, _>("fingerprint") {
        return Err(ApiError::Conflict(
            "对象、授权、账期或部署已变化，请重新预览；草稿保留".into(),
        ));
    }
    let receipt = mutations::execute(&mut tx, administrator, id, &operation).await?;
    event(
        &mut tx,
        Some(administrator),
        mutations::user_id(&operation),
        "operation_apply",
        json!({"preview_id":id,"summary":row.get::<Value,_>("summary"),"receipt":receipt}),
    )
    .await?;
    sqlx::query("UPDATE singbox_operation_previews SET receipt=$2 WHERE id=$1")
        .bind(id)
        .bind(&receipt)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}

async fn authorize(state: &AppState, headers: &HeaderMap, operation: &Operation) -> ApiResult<()> {
    let (users, nodes, groups, chains) = match operation {
        Operation::PolicyBatch {
            user_ids,
            group_ids,
        } => (user_ids.clone(), Vec::new(), group_ids.clone(), Vec::new()),
        Operation::ReplacePackage { user_id, .. }
        | Operation::SetValidity { user_id, .. }
        | Operation::ResetQuota { user_id } => (vec![*user_id], Vec::new(), Vec::new(), Vec::new()),
        Operation::RotateNodeCredentials { user_id, node_ids } => {
            (vec![*user_id], node_ids.clone(), Vec::new(), Vec::new())
        }
        Operation::MigrateNode {
            source_node_id,
            candidate_node_id,
        } => (
            Vec::new(),
            vec![*source_node_id, *candidate_node_id],
            Vec::new(),
            Vec::new(),
        ),
        Operation::Failover {
            source_chain_id,
            alternate_chain_id,
            group_ids,
            ..
        } => (
            Vec::new(),
            Vec::new(),
            group_ids.clone(),
            vec![*source_chain_id, *alternate_chain_id],
        ),
    };
    let affected:Vec<i64>=sqlx::query_scalar("WITH selected_nodes AS (SELECT node_id FROM accesses WHERE user_id=ANY($1) UNION SELECT unnest($2::bigint[]) UNION SELECT node_id FROM singbox_policy_nodes WHERE group_id=ANY($3) UNION SELECT c.entry_node_id FROM singbox_chains c WHERE c.id=ANY($4) OR c.id IN(SELECT chain_id FROM singbox_policy_chains WHERE group_id=ANY($3)) UNION SELECT h.managed_node_id FROM singbox_ordered_chain_hops h WHERE h.chain_id=ANY($4) OR h.chain_id IN(SELECT chain_id FROM singbox_policy_chains WHERE group_id=ANY($3))) SELECT DISTINCT n.server_id FROM nodes n JOIN selected_nodes a ON a.node_id=n.id ORDER BY n.server_id")
        .bind(users).bind(nodes).bind(groups).bind(chains).fetch_all(&state.pool).await?;
    for server in affected {
        crate::control_center::require_server(state, headers, server, "proxy:write").await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
