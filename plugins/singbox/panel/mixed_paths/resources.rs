use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Value>>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    Ok(Json(resources_on(&mut tx).await?))
}

pub(crate) async fn resources_on(tx: &mut Transaction<'_, Postgres>) -> ApiResult<Vec<Value>> {
    let nodes=sqlx::query("SELECT n.id,n.name,n.server_id,s.name AS server_name,n.protocol,n.public_host,n.port,n.enabled,(SELECT COUNT(DISTINCT h.chain_id) FROM singbox_chain_hops h JOIN singbox_live_chains c ON c.id=h.chain_id WHERE h.managed_node_id=n.id) AS reference_count, (n.enabled AND n.protocol='vless-reality' AND NOT EXISTS(SELECT 1 FROM accesses a WHERE a.node_id=n.id) AND NOT EXISTS(SELECT 1 FROM singbox_policy_nodes p WHERE p.node_id=n.id) AND NOT EXISTS(SELECT 1 FROM singbox_live_chains c WHERE c.entry_node_id=n.id OR c.exit_node_id=n.id) AND NOT EXISTS(SELECT 1 FROM singbox_chain_hops h JOIN singbox_live_chains c ON c.id=h.chain_id WHERE h.managed_node_id=n.id)) AND (NOT EXISTS(SELECT 1 FROM singbox_chains oc WHERE oc.path_kind='ordered' AND (oc.deleted_at IS NULL OR oc.phase<>'retired') AND oc.entry_node_id=n.id) AND NOT EXISTS(SELECT 1 FROM singbox_ordered_chain_hops oh JOIN singbox_chains oc ON oc.id=oh.chain_id WHERE oc.path_kind='ordered' AND (oc.deleted_at IS NULL OR oc.phase<>'retired') AND oh.managed_node_id=n.id AND (oh.generation=ANY(ARRAY[oc.desired_generation,oc.applied_generation,oc.candidate_generation,oc.recovery_generation]) OR EXISTS(SELECT 1 FROM unnest(ARRAY[oc.desired_generation,oc.applied_generation,oc.candidate_generation,oc.recovery_generation]) AS selected(generation) WHERE selected.generation IS NOT NULL AND NOT EXISTS(SELECT 1 FROM singbox_ordered_chain_versions v WHERE v.chain_id=oc.id AND v.generation=selected.generation))))) AS entry_eligible FROM nodes n JOIN servers s ON s.id=n.server_id WHERE n.deleted_at IS NULL AND s.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM singbox_chains c WHERE c.entry_node_id=n.id AND (c.deleted_at IS NULL OR (c.path_kind='ordered' AND c.phase<>'retired'))) ORDER BY n.id").fetch_all(&mut **tx).await?;
    let mut result = Vec::new();
    for n in nodes {
        let count: i64 = n.get("reference_count");
        let protocol: String = n.get("protocol");
        result.push(json!({"kind":"direct","id":n.get::<i64,_>("id"),"name":n.get::<String,_>("name"),"server_id":n.get::<i64,_>("server_id"),"managed_server_ids":[n.get::<i64,_>("server_id")],"managed_middle_server_ids":[],"managed_exit_server_ids":[],"server_name":n.get::<String,_>("server_name"),"protocol":protocol,"public_host":n.get::<String,_>("public_host"),"port":n.get::<i32,_>("port"),"enabled":n.get::<bool,_>("enabled"),"available":n.get::<bool,_>("enabled"),"entry_eligible":n.get::<bool,_>("entry_eligible"),"role":if count>0 {"managed_hop"} else {"direct"},"entry_node_id":null,"tcp":true,"udp":!matches!(protocol.as_str(),"naive"),"legacy":false,"active_generation":null,"pending_generation":null,"minimum_generation":0,"stage":"direct","last_error":null,"reference_count":count}));
    }
    let chains=sqlx::query("SELECT c.*,n.server_id,s.name AS server_name,n.protocol,n.public_host,n.port,n.enabled,singbox_path_resources_available(c.id) AS available,v.networks,v.stage,v.last_error,(SELECT COUNT(*) FROM singbox_policy_chains p WHERE p.chain_id=c.id) AS reference_count FROM singbox_live_chains c JOIN nodes n ON n.id=c.entry_node_id JOIN servers s ON s.id=n.server_id LEFT JOIN singbox_chain_versions v ON v.chain_id=c.id AND v.generation=COALESCE(c.pending_generation,c.active_generation) ORDER BY c.id").fetch_all(&mut **tx).await?;
    for c in chains {
        let chain_id: i64 = c.get("id");
        let entry_server: i64 = c.get("server_id");
        let members: Vec<(i64, bool)> = sqlx::query_as("SELECT DISTINCT h.managed_server_id,(h.position=(SELECT MAX(last.position) FROM singbox_chain_hops last WHERE last.chain_id=h.chain_id AND last.generation=h.generation)) AS final_hop FROM singbox_chain_hops h WHERE h.chain_id=$1 AND h.generation=COALESCE($2,$3) AND h.managed_server_id IS NOT NULL ORDER BY 1,2")
            .bind(chain_id).bind(c.get::<Option<i64>, _>("pending_generation")).bind(c.get::<Option<i64>, _>("active_generation")).fetch_all(&mut **tx).await?;
        let mut managed_servers = std::collections::BTreeSet::from([entry_server]);
        let mut middle_servers = std::collections::BTreeSet::new();
        let mut exit_servers = std::collections::BTreeSet::new();
        for (server, final_hop) in members {
            managed_servers.insert(server);
            if final_hop {
                exit_servers.insert(server);
            } else {
                middle_servers.insert(server);
            }
        }
        let networks: Value = c
            .try_get("networks")
            .unwrap_or_else(|_| json!({"tcp":true,"udp":true}));
        result.push(json!({"kind":"chain","id":c.get::<i64,_>("id"),"name":c.get::<String,_>("name"),"server_id":c.get::<i64,_>("server_id"),"managed_server_ids":managed_servers,"managed_middle_server_ids":middle_servers,"managed_exit_server_ids":exit_servers,"server_name":c.get::<String,_>("server_name"),"protocol":c.get::<String,_>("protocol"),"public_host":c.get::<String,_>("public_host"),"port":c.get::<i32,_>("port"),"enabled":c.get::<bool,_>("enabled"),"available":c.get::<bool,_>("available"),"role":"chain_entry","entry_node_id":c.get::<i64,_>("entry_node_id"),"tcp":networks["tcp"],"udp":networks["udp"],"legacy":c.get::<String,_>("path_kind")=="legacy","active_generation":c.get::<Option<i64>,_>("active_generation"),"pending_generation":c.get::<Option<i64>,_>("pending_generation"),"minimum_generation":c.get::<i64,_>("minimum_generation"),"stage":c.get::<Option<String>,_>("stage").unwrap_or_else(||"active".into()),"last_error":c.get::<Option<String>,_>("last_error"),"reference_count":c.get::<i64,_>("reference_count")}));
    }
    Ok(result)
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, i64)>,
) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let resource = resources_on(&mut tx)
        .await?
        .into_iter()
        .find(|v| v["kind"] == kind && v["id"] == id)
        .ok_or(ApiError::NotFound)?;
    let node_id = resource["entry_node_id"].as_i64().unwrap_or(id);
    let query = format!(
        "SELECT {} FROM nodes n WHERE n.id=$1",
        super::super::business::NODE_COLUMNS
    );
    let node: super::super::business::NodeRow = sqlx::query_as(&query)
        .bind(node_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let node = node.view()?;
    let mut hops = Vec::new();
    let mut versions = Vec::new();
    let mut conversion = None;
    if kind == "chain" {
        conversion = crate::mixed_conversion::conversion(&mut tx, id).await?;
        let rows=sqlx::query("SELECT h.*,n.name AS managed_name,n.protocol AS managed_protocol,n.public_host AS managed_host,n.port AS managed_port,n.enabled AS managed_enabled,n.deleted_at AS managed_deleted,ms.deleted_at AS managed_server_deleted,s.archived AS source_archived,s.deleted_at AS source_deleted,s.identity_epoch AS source_epoch,e.identity_epoch AS node_epoch,e.name AS external_name,e.present,e.identity_unique,e.current_version_id,v.config_json,v.name AS version_name FROM singbox_chain_hops h JOIN singbox_live_chains c ON c.id=h.chain_id LEFT JOIN nodes n ON n.id=h.managed_node_id LEFT JOIN servers ms ON ms.id=h.managed_server_id LEFT JOIN singbox_subscription_sources s ON s.id=h.source_id LEFT JOIN singbox_external_nodes e ON e.id=h.external_node_id LEFT JOIN singbox_external_node_versions v ON v.id=h.external_version_id WHERE h.chain_id=$1 AND h.generation=COALESCE(c.pending_generation,c.active_generation) ORDER BY h.position").bind(id).fetch_all(&mut *tx).await?;
        for h in rows {
            let managed = h.get::<String, _>("kind") == "managed";
            let external: Value = h.try_get("config_json").unwrap_or(Value::Null);
            hops.push(json!({"position":h.get::<i32,_>("position"),"kind":h.get::<String,_>("kind"),"node_id":if managed {h.get::<i64,_>("managed_node_id")} else {h.get::<i64,_>("external_node_id")},"server_id":h.get::<Option<i64>,_>("managed_server_id"),"source_id":h.get::<Option<i64>,_>("source_id"),"version_id":h.get::<Option<i64>,_>("external_version_id"),"update_mode":h.get::<Option<String>,_>("update_mode"),"name":if managed {h.get::<String,_>("managed_name")} else {h.get::<String,_>("version_name")},"protocol":if managed {json!(h.get::<String,_>("managed_protocol"))} else {external["type"].clone()},"server":if managed {json!(h.get::<String,_>("managed_host"))} else {external["server"].clone()},"port":if managed {json!(h.get::<i32,_>("managed_port"))} else {external["server_port"].clone()},"present":if managed {h.get::<bool,_>("managed_enabled")&&h.get::<Option<i64>,_>("managed_deleted").is_none()&&h.get::<Option<i64>,_>("managed_server_deleted").is_none()} else {h.get::<bool,_>("present")&&h.get::<bool,_>("identity_unique")&&!h.get::<bool,_>("source_archived")&&h.get::<Option<i64>,_>("source_deleted").is_none()&&h.get::<i64,_>("source_epoch")==h.get::<i64,_>("node_epoch")},"latest_version_id":h.get::<Option<i64>,_>("current_version_id")}));
        }
        let rows=sqlx::query("SELECT generation,stage,last_error,created_at FROM singbox_chain_versions WHERE chain_id=$1 ORDER BY generation DESC LIMIT 64").bind(id).fetch_all(&mut *tx).await?;
        versions=rows.into_iter().map(|v|json!({"generation":v.get::<i64,_>("generation"),"stage":v.get::<String,_>("stage"),"last_error":v.get::<Option<String>,_>("last_error"),"created_at":v.get::<i64,_>("created_at")})).collect();
    }
    Ok(Json(
        json!({"resource":resource,"node":node,"hops":hops,"versions":versions,"conversion":conversion}),
    ))
}

pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, i64)>,
    Json(value): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    if kind == "direct" {
        let request = serde_json::from_value(value)
            .map_err(|_| ApiError::BadRequest("节点参数无效".into()))?;
        let response = super::super::nodes::update(
            State(state.clone()),
            headers.clone(),
            Path(id),
            Json(request),
        )
        .await?;
        return Ok(Json(
            serde_json::to_value(response.0).map_err(anyhow::Error::from)?,
        ));
    }
    if kind != "chain" {
        return Err(ApiError::NotFound);
    }
    if value.as_object().is_none_or(|o| {
        !o.contains_key("name")
            || o.keys()
                .any(|key| !matches!(key.as_str(), "name" | "subscription_name"))
    }) {
        return Err(ApiError::BadRequest(
            "链路仅允许修改名称；更换路径请创建新链路".into(),
        ));
    }
    let name = super::super::business::name(value["name"].as_str().unwrap_or_default())?;
    let subscription_name = value
        .get("subscription_name")
        .map(|v| super::super::business::name(v.as_str().unwrap_or_default()))
        .transpose()?;
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    let node:i64=sqlx::query_scalar("UPDATE singbox_chains SET name=$2 WHERE id=$1 AND deleted_at IS NULL RETURNING entry_node_id").bind(id).bind(&name).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    if let Some(subscription_name) = subscription_name {
        let server: i64 =
            sqlx::query_scalar("UPDATE nodes SET name=$2 WHERE id=$1 RETURNING server_id")
                .bind(node)
                .bind(subscription_name)
                .fetch_one(&mut *tx)
                .await?;
        super::super::business::mark_dirty(&mut tx, &[server]).await?;
    }
    tx.commit().await?;
    detail(State(state), headers, Path((kind, id))).await
}

pub async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, i64)>,
) -> ApiResult<StatusCode> {
    auth::require_admin(&state, &headers).await?;
    if kind == "direct" {
        return super::super::nodes::remove(State(state), headers, Path(id)).await;
    }
    if kind != "chain" {
        return Err(ApiError::NotFound);
    }
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    remove_on(&mut tx, id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_on(tx: &mut Transaction<'_, Postgres>, id: i64) -> ApiResult<()> {
    let entry: i64 =
        sqlx::query_scalar("SELECT entry_node_id FROM singbox_live_chains WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    let hosts:Vec<i64>=sqlx::query_scalar("SELECT server_id FROM nodes WHERE id=$2 UNION SELECT managed_server_id FROM singbox_chain_hops WHERE chain_id=$1 AND managed_server_id IS NOT NULL UNION SELECT e.server_id FROM singbox_chains c JOIN nodes e ON e.id=c.exit_node_id WHERE c.id=$1 ORDER BY 1").bind(id).bind(entry).fetch_all(&mut **tx).await?;
    super::super::proxy_resources::lock_cleanup_servers(tx, &hosts).await?;
    super::super::proxy_resources::ensure_chain_entry_unreferenced_on(tx, id, entry).await?;
    sqlx::query("UPDATE nodes SET enabled=FALSE,deleted_at=$2 WHERE id=$1")
        .bind(entry)
        .bind(apsp_protocol::now_timestamp())
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM accesses WHERE node_id=$1")
        .bind(entry)
        .execute(&mut **tx)
        .await?;
    // Legacy resources also own the rich immutable projection. Complete its
    // local retirement without changing any saved snapshot or request receipt.
    sqlx::query("UPDATE singbox_chains SET deleted_at=$2,route_enabled=FALSE,phase=CASE WHEN path_kind='legacy' THEN 'retired' ELSE phase END,applied_generation=CASE WHEN path_kind='legacy' THEN NULL ELSE applied_generation END WHERE id=$1")
        .bind(id)
        .bind(apsp_protocol::now_timestamp())
        .execute(&mut **tx)
        .await?;
    super::super::business::mark_dirty(tx, &hosts).await?;
    Ok(())
}
