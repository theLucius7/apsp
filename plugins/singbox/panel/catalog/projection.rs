use crate::{AppState, auth::require_admin, error::ApiResult};
use axum::{Json, extract::State, http::HeaderMap};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeMap;

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Value>>> {
    require_admin(&state, &headers).await?;
    let mut tx = crate::read_snapshot(&state.pool).await?;
    Ok(Json(catalog_on(&mut tx).await?))
}

pub(crate) async fn catalog_on(tx: &mut Transaction<'_, Postgres>) -> ApiResult<Vec<Value>> {
    let mut resources = super::super::mixed_paths::resources::resources_on(tx).await?;
    // Private values contribute to concurrency tokens, never to the public projection.
    let nodes: Vec<(i64, Value)> =
        sqlx::query_as("SELECT n.id,to_jsonb(n) FROM nodes n WHERE n.deleted_at IS NULL")
            .fetch_all(&mut **tx)
            .await?;
    let nodes: BTreeMap<_, _> = nodes.into_iter().collect();
    let chains: Vec<(i64, Value)> = sqlx::query_as("SELECT c.id,jsonb_build_object('name',c.name,'entry_node_id',c.entry_node_id,'active_generation',c.active_generation,'pending_generation',c.pending_generation,'minimum_generation',c.minimum_generation) FROM singbox_live_chains c")
        .fetch_all(&mut **tx).await?;
    let chains: BTreeMap<_, _> = chains.into_iter().collect();
    let mut inputs = BTreeMap::new();
    for resource in &resources {
        let id = resource["id"].as_i64().expect("resource id");
        let kind = resource["kind"].as_str().expect("resource kind");
        let node = resource["entry_node_id"].as_i64().unwrap_or(id);
        inputs.insert(
            (kind.to_owned(), id),
            json!([
                nodes.get(&node),
                chains.get(&id).filter(|_| kind == "chain")
            ]),
        );
    }
    // External nodes come from numbered sources until the source migration and
    // from ordered sources after it; ids are the public numeric ids either way.
    let external = sqlx::query("SELECT e.id,e.source_id,e.identity_epoch,e.name,e.present,e.identity_unique,e.current_version_id,e.source_name,e.source_epoch,e.source_archived AS archived,e.source_deleted_at AS source_deleted,e.source_settings_revision AS settings_revision,(SELECT COUNT(DISTINCT h.chain_id) FROM singbox_chain_hops h JOIN singbox_live_chains c ON c.id=h.chain_id WHERE h.external_node_id=e.id)+(SELECT COUNT(DISTINCT h.chain_id) FROM singbox_ordered_chain_hops h JOIN singbox_ordered_external_nodes o ON o.id=h.external_node_id JOIN singbox_chains c ON c.id=h.chain_id WHERE o.public_id=e.id AND c.path_kind='ordered' AND c.phase<>'retired' AND h.generation IN (c.applied_generation,c.candidate_generation,c.recovery_generation,c.desired_generation)) AS reference_count FROM singbox_external_node_states e WHERE e.adopted ORDER BY e.id")
        .fetch_all(&mut **tx).await?;
    let versions = super::super::external_access::model::versions(
        tx,
        external
            .iter()
            .filter_map(|row| row.get::<Option<i64>, _>("current_version_id"))
            .collect(),
    )
    .await?;
    for row in external {
        let id: i64 = row.get("id");
        let version = row
            .get::<Option<i64>, _>("current_version_id")
            .and_then(|id| versions.get(&id));
        let outbound = version.and_then(|version| version.outbound());
        let capabilities = outbound.as_ref().and_then(|v| v.capabilities().ok());
        let available = row.get::<bool, _>("present")
            && row.get::<bool, _>("identity_unique")
            && !row.get::<bool, _>("archived")
            && row.get::<Option<i64>, _>("source_deleted").is_none()
            && row.get::<i64, _>("identity_epoch") == row.get::<i64, _>("source_epoch")
            && capabilities.is_some();
        let reason = if row.get::<Option<i64>, _>("source_deleted").is_some() {
            Some("source_deleted")
        } else if row.get::<bool, _>("archived") {
            Some("source_archived")
        } else if row.get::<i64, _>("identity_epoch") != row.get::<i64, _>("source_epoch") {
            Some("source_replaced")
        } else if !row.get::<bool, _>("present") {
            Some("node_missing")
        } else if !row.get::<bool, _>("identity_unique") {
            Some("ambiguous_node_identity")
        } else if capabilities.is_none() {
            Some("unsupported_configuration")
        } else {
            None
        };
        let resource = json!({"kind":"external","id":id,"name":row.get::<String,_>("name"),
            "server_id":null,"server_name":null,"protocol":outbound.as_ref().map(|v|v.protocol()),
            "public_host":outbound.as_ref().map(|v|v.server()),"port":outbound.as_ref().map(|v|v.port()),
            "enabled":true,"available":available,"entry_eligible":false,"role":"external","entry_node_id":null,
            "tcp":capabilities.as_ref().is_some_and(|v|v.tcp),"udp":capabilities.as_ref().is_some_and(|v|v.udp),
            "legacy":false,"active_generation":null,"pending_generation":null,"minimum_generation":0,
            "stage":"external","last_error":reason,"reference_count":row.get::<i64,_>("reference_count"),
            "source_id":row.get::<i64,_>("source_id"),"source_name":row.get::<String,_>("source_name"),
            "version_id":row.get::<Option<i64>,_>("current_version_id"),"identity_epoch":row.get::<i64,_>("identity_epoch"),
            "present":row.get::<bool,_>("present"),"identity_unique":row.get::<bool,_>("identity_unique")});
        inputs.insert(
            ("external".into(), id),
            json!([
                &resource,
                row.get::<i64, _>("settings_revision"),
                version.and_then(|version| version
                    .config_sha256
                    .clone()
                    .or_else(|| version.content_digest.clone()))
            ]),
        );
        resources.push(resource);
    }
    let metadata: Vec<(String, i64, Value)> =
        sqlx::query_as("SELECT kind,id,to_jsonb(m) FROM singbox_node_metadata m")
            .fetch_all(&mut **tx)
            .await?;
    let metadata: BTreeMap<_, _> = metadata
        .into_iter()
        .map(|(kind, id, value)| ((kind, id), value))
        .collect();
    resources.retain_mut(|resource| {
        let key = (
            resource["kind"].as_str().unwrap().to_owned(),
            resource["id"].as_i64().unwrap(),
        );
        let meta = metadata
            .get(&key)
            .cloned()
            .unwrap_or_else(|| json!({"revision":0}));
        if !meta["deleted_at"].is_null() {
            return false;
        }
        let original_name = resource["name"].clone();
        resource["original_name"] = original_name.clone();
        if key.0 == "external" {
            resource["name"] = meta
                .get("name_override")
                .filter(|v| v.is_string())
                .cloned()
                .unwrap_or(original_name);
            resource["enabled"] = json!(meta["enabled"].as_bool().unwrap_or(true));
            resource["available"] = json!(
                resource["available"].as_bool().unwrap_or(false) && resource["enabled"] == true
            );
        }
        for (field, default) in [
            ("tags", json!([])),
            ("note", json!("")),
            ("sort_order", json!(0)),
        ] {
            resource[field] = meta.get(field).cloned().unwrap_or(default);
        }
        resource["metadata_revision"] = meta["revision"].clone();
        let bytes = serde_json::to_vec(&json!([inputs.get(&key), meta])).expect("JSON values");
        resource["revision"] = json!(format!("{:x}", Sha256::digest(bytes)));
        true
    });
    resources.sort_by_key(|v| {
        (
            v["sort_order"].as_i64().unwrap_or(0),
            v["kind"].as_str().unwrap_or("").to_owned(),
            v["id"].as_i64().unwrap_or(0),
        )
    });
    Ok(resources)
}
