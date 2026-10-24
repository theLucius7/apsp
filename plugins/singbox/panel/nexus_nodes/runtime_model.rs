use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_compiler::preserved::{PreservedNode, compile_preserved_server};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeSnapshot {
    pub source_id: String,
    pub server_id: i64,
    pub nodes: Vec<RuntimeNode>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeNode {
    pub legacy_node_id: i64,
    pub preserved: PreservedNode,
}

pub(super) fn digest(snapshot: &RuntimeSnapshot) -> ApiResult<String> {
    Ok(crate::auth::hash_token(
        &serde_json::to_string(snapshot).map_err(anyhow::Error::from)?,
    ))
}

pub(super) fn validate(snapshot: &RuntimeSnapshot) -> ApiResult<()> {
    let mut ids = BTreeSet::new();
    if snapshot.nodes.is_empty() || snapshot.nodes.len() > 200 {
        return Err(ApiError::BadRequest(
            "每台主机需提供完整的 1 至 200 个节点配置".into(),
        ));
    }
    for entry in &snapshot.nodes {
        let node = &entry.preserved.node;
        if entry.legacy_node_id <= 0
            || !ids.insert(entry.legacy_node_id)
            || node.id != entry.legacy_node_id
            || !node.users.is_empty()
            || !matches!(
                node.protocol_config,
                apsp_compiler::ProtocolConfig::VlessReality
                    | apsp_compiler::ProtocolConfig::Shadowsocks2022 { .. }
                    | apsp_compiler::ProtocolConfig::Hysteria2 { .. }
            )
        {
            return Err(ApiError::BadRequest(
                "原节点标识、用户列表或协议不适用于运行迁移".into(),
            ));
        }
    }
    compile_preserved_server(
        &snapshot
            .nodes
            .iter()
            .map(|entry| entry.preserved.clone())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| ApiError::BadRequest(format!("保留配置无法合并：{error}")))?;
    Ok(())
}

fn decoded(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| value.clone())
}

/// Compare the proposed wire identity with the separately encrypted inventory.
pub(super) async fn inspect(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &RuntimeSnapshot,
) -> ApiResult<Value> {
    validate(snapshot)?;
    let rows = sqlx::query("SELECT n.id,n.legacy_node_id,n.display,n.credential_id FROM singbox_nexus_nodes n WHERE n.source_id=$1 AND n.server_id=$2 ORDER BY n.legacy_node_id FOR SHARE")
        .bind(&snapshot.source_id).bind(snapshot.server_id).fetch_all(&mut **tx).await?;
    let supplied: BTreeSet<_> = snapshot
        .nodes
        .iter()
        .map(|entry| entry.legacy_node_id)
        .collect();
    let expected: BTreeSet<i64> = rows.iter().map(|row| row.get("legacy_node_id")).collect();
    let mut conflicts = Vec::new();
    if supplied != expected {
        conflicts.push("需一次接管同一主机的全部原节点，不能遗漏实例".to_owned());
    }
    for entry in &snapshot.nodes {
        let Some(row) = rows
            .iter()
            .find(|row| row.get::<i64, _>("legacy_node_id") == entry.legacy_node_id)
        else {
            continue;
        };
        let archive = control_center::credentials::resolve_reference_pool(
            &state.pool,
            row.get::<Uuid, _>("credential_id"),
            "backup",
            "nexus-runtime-preview",
        )
        .await?;
        let original = &archive["node"];
        let protocol = decoded(&original["protocol_settings"]);
        let display: Value = row.get("display");
        let node = &entry.preserved.node;
        let rate = display["rate"].as_str().unwrap_or_default();
        if !rate
            .parse::<f64>()
            .is_ok_and(|v| v.is_finite() && (0.0..=999999.0).contains(&v))
            || rate
                .split_once('.')
                .is_some_and(|(_, fraction)| fraction.len() > 6)
        {
            conflicts.push(format!(
                "原节点 {} 的流量倍率不受支持",
                entry.legacy_node_id
            ));
        }
        let mut same = display["listen_port"].as_str() == Some(node.port.to_string().as_str())
            && original["host"].as_str() == Some(node.public_host.as_str())
            && original["name"].as_str() == Some(node.name.as_str());
        match &node.protocol_config {
            apsp_compiler::ProtocolConfig::VlessReality => {
                same &= original["type"] == "vless" && protocol["tls"] == 2;
                let reality = &protocol["reality_settings"];
                same &= reality["private_key"] == node.private_key
                    && reality["public_key"] == node.public_key
                    && reality["short_id"] == node.short_id
                    && reality["server_name"] == node.sni;
            }
            apsp_compiler::ProtocolConfig::Shadowsocks2022 { method, .. } => {
                same &= original["type"] == "shadowsocks" && protocol["cipher"] == json!(method);
            }
            apsp_compiler::ProtocolConfig::Hysteria2 { .. } => {
                same &= original["type"] == "hysteria" && protocol["version"] == 2
            }
            _ => same = false,
        }
        if original["rate_time_enable"] == 1 {
            conflicts.push(format!(
                "原节点 {} 的分时倍率需先迁移计费规则",
                entry.legacy_node_id
            ));
        }
        if !same {
            conflicts.push(format!(
                "原节点 {} 的监听或客户端身份与已存档资料不一致",
                entry.legacy_node_id
            ));
        }
    }
    let saved: Option<(String, String)> =
        sqlx::query_as("SELECT digest,state FROM singbox_nexus_runtime_hosts WHERE server_id=$1")
            .bind(snapshot.server_id)
            .fetch_optional(&mut **tx)
            .await?;
    let hash = digest(snapshot)?;
    if saved.as_ref().is_some_and(|(value, _)| value != &hash) {
        conflicts.push("该主机已准备另一份运行快照，不能覆盖".into());
    }
    let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes n WHERE n.server_id=$1 AND n.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM singbox_nexus_runtime_nodes r WHERE r.node_id=n.id)) OR EXISTS(SELECT 1 FROM deployments d WHERE d.server_id=$1 AND NOT EXISTS(SELECT 1 FROM singbox_nexus_runtime_hosts h WHERE h.server_id=d.server_id))")
        .bind(snapshot.server_id).fetch_one(&mut **tx).await?;
    if occupied {
        conflicts.push("目标主机已有其他受管节点或部署，不能合并发布所有权".into());
    }
    Ok(
        json!({"server_id":snapshot.server_id,"digest":hash,"node_count":snapshot.nodes.len(),"already_prepared":saved.is_some(),"state":saved.map(|(_,s)|s).unwrap_or_else(||"pending_agent".into()),"conflicts":conflicts}),
    )
}
