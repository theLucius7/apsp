use crate::error::{ApiError, ApiResult};
use apsp_compiler::{
    client::ExternalClientNode,
    external::{ExternalOutbound, NormalizedOutbound},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgConnection};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMode {
    FollowNode,
    Pinned,
}
impl UpdateMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FollowNode => "follow_node",
            Self::Pinned => "pinned",
        }
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub external_node_id: i64,
    pub source_id: i64,
    pub identity_epoch: i64,
    pub node_version_id: i64,
    pub update_mode: UpdateMode,
    pub metadata_revision: i64,
}

#[derive(FromRow)]
pub struct Binding {
    pub external_node_id: i64,
    pub source_id: i64,
    pub identity_epoch: i64,
    pub node_version_id: i64,
    pub update_mode: String,
    pub created_at: i64,
}
impl Binding {
    pub fn matches(&self, value: &Reference) -> bool {
        self.external_node_id == value.external_node_id
            && self.source_id == value.source_id
            && self.identity_epoch == value.identity_epoch
            && self.node_version_id == value.node_version_id
            && self.update_mode == value.update_mode.as_str()
    }
    pub fn reference(&self, metadata_revision: i64) -> Reference {
        Reference {
            external_node_id: self.external_node_id,
            source_id: self.source_id,
            identity_epoch: self.identity_epoch,
            node_version_id: self.node_version_id,
            metadata_revision,
            update_mode: if self.update_mode == "pinned" {
                UpdateMode::Pinned
            } else {
                UpdateMode::FollowNode
            },
        }
    }
}

#[derive(Serialize)]
pub struct Entry {
    #[serde(flatten)]
    pub reference: Reference,
    pub name: String,
    pub source_name: String,
    pub protocol: String,
    pub server: String,
    pub port: u16,
    pub available: bool,
    pub reason: Option<&'static str>,
    pub current_version_id: Option<i64>,
    pub resolved_version_id: Option<i64>,
    pub source_last_error: Option<String>,
}

#[derive(FromRow)]
pub struct NodeState {
    pub id: i64,
    pub source_id: i64,
    pub identity_epoch: i64,
    pub current_version_id: Option<i64>,
    pub name: String,
    pub source_name: String,
    pub adopted: bool,
    pub present: bool,
    pub identity_unique: bool,
    pub source_epoch: i64,
    pub source_archived: bool,
    pub source_deleted: bool,
    pub source_last_error: Option<String>,
    pub enabled: bool,
    pub deleted: bool,
    pub metadata_revision: i64,
    pub sort_order: i64,
}

/// A version read through `singbox_external_version_configs`: a numbered-source
/// configuration (also kept for imported versions), or an ordered-source
/// typed outbound.
#[derive(FromRow)]
pub struct Version {
    pub id: i64,
    pub external_node_id: i64,
    pub source_id: i64,
    pub identity_epoch: i64,
    pub parser_version: String,
    pub ordered: bool,
    pub config_json: Option<Value>,
    pub config_sha256: Option<String>,
    pub normalized_config: Option<Value>,
    pub content_digest: Option<String>,
}
impl Version {
    pub fn outbound(&self) -> Option<ExternalOutbound> {
        if let (Some(config), Some(sha256)) = (&self.config_json, &self.config_sha256) {
            let value = ExternalOutbound(config.clone());
            value.capabilities().ok()?;
            let serialized = serde_json::to_vec(&value).ok()?;
            return (super::super::sources::parse::digest(&serialized) == *sha256).then_some(value);
        }
        // Typed outbounds are checked against their stored content digest first.
        let typed: NormalizedOutbound =
            serde_json::from_value(self.normalized_config.clone()?).ok()?;
        let serialized = serde_json::to_vec(&typed).ok()?;
        if super::super::sources::parse::digest(&serialized) != *self.content_digest.as_ref()? {
            return None;
        }
        let value = ExternalOutbound::from_normalized(&typed).ok()?;
        value.capabilities().ok()?;
        Some(value)
    }

    /// New grants require the parser that currently produces this kind of
    /// version. A version imported from numbered sources is still served from
    /// its original outbound, so the numbered parser's version applies to it.
    fn current_parser(&self) -> bool {
        self.parser_version
            == if self.ordered && self.config_json.is_none() {
                super::super::subscription_parser::PARSER_VERSION
            } else {
                super::super::sources::parse::PARSER_VERSION
            }
    }
}

impl NodeState {
    pub fn reason(
        &self,
        version: Option<&Version>,
        epoch: i64,
        new_access: bool,
    ) -> Option<&'static str> {
        if self.source_deleted {
            return Some("source_deleted");
        }
        if self.source_archived {
            return Some("source_archived");
        }
        if epoch != self.identity_epoch || epoch != self.source_epoch {
            return Some("source_replaced");
        }
        if self.deleted {
            return Some("node_deleted");
        }
        if !self.adopted {
            return Some("node_not_adopted");
        }
        if !self.enabled {
            return Some("node_disabled");
        }
        if !self.identity_unique {
            return Some("ambiguous_node_identity");
        }
        if !self.present {
            return Some("node_missing");
        }
        let Some(version) = version else {
            return Some("version_missing");
        };
        if version.external_node_id != self.id
            || version.source_id != self.source_id
            || version.identity_epoch != epoch
        {
            return Some("version_identity_mismatch");
        }
        if version.outbound().is_none() {
            return Some("invalid_version");
        }
        if new_access && !version.current_parser() {
            return Some("parser_update_required");
        }
        None
    }
    pub fn entry(
        &self,
        reference: Reference,
        version: Option<&Version>,
        new_access: bool,
    ) -> Entry {
        let reason = self.reason(version, reference.identity_epoch, new_access);
        let outbound = version.and_then(Version::outbound);
        Entry {
            reference,
            name: self.name.clone(),
            source_name: self.source_name.clone(),
            protocol: outbound
                .as_ref()
                .map(|v| v.protocol().into())
                .unwrap_or_default(),
            server: outbound
                .as_ref()
                .map(|v| v.server().into())
                .unwrap_or_default(),
            port: outbound.as_ref().map_or(0, ExternalOutbound::port),
            available: reason.is_none(),
            reason,
            current_version_id: self.current_version_id,
            resolved_version_id: version.map(|v| v.id),
            source_last_error: self.source_last_error.clone(),
        }
    }
}

pub async fn bindings(connection: &mut PgConnection, user_id: i64) -> ApiResult<Vec<Binding>> {
    Ok(sqlx::query_as("SELECT external_node_id,source_id,identity_epoch,node_version_id,update_mode,created_at FROM singbox_external_accesses WHERE user_id=$1 ORDER BY external_node_id")
        .bind(user_id).fetch_all(connection).await?)
}

pub async fn states(
    connection: &mut PgConnection,
    ids: Option<&[i64]>,
) -> ApiResult<BTreeMap<i64, NodeState>> {
    let rows: Vec<NodeState> = sqlx::query_as("SELECT n.id,n.source_id,n.identity_epoch,n.current_version_id,COALESCE(m.name_override,n.name) AS name,n.source_name,n.adopted,n.present,n.identity_unique,n.source_epoch,n.source_archived,n.source_deleted_at IS NOT NULL AS source_deleted,n.source_last_error,COALESCE(m.enabled,TRUE) AS enabled,m.deleted_at IS NOT NULL AS deleted,COALESCE(m.revision,0) AS metadata_revision,COALESCE(m.sort_order,0) AS sort_order FROM singbox_external_node_states n LEFT JOIN singbox_node_metadata m ON m.kind='external' AND m.id=n.id WHERE ($1::BIGINT[] IS NULL OR n.id=ANY($1)) ORDER BY n.id")
        .bind(ids).fetch_all(connection).await?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

pub async fn versions(
    connection: &mut PgConnection,
    ids: Vec<i64>,
) -> ApiResult<BTreeMap<i64, Version>> {
    let rows:Vec<Version> = sqlx::query_as("SELECT id,external_node_id,source_id,identity_epoch,parser_version,ordered,config_json,config_sha256,normalized_config,content_digest FROM singbox_external_version_configs WHERE id=ANY($1)")
        .bind(ids).fetch_all(connection).await?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

pub async fn revision(connection: &mut PgConnection, user_id: i64) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT revision FROM singbox_external_access_state WHERE user_id=$1")
            .bind(user_id)
            .fetch_optional(connection)
            .await?
            .unwrap_or(0),
    )
}

pub struct SubscriptionNodes {
    pub granted: usize,
    pub nodes: Vec<ExternalClientNode>,
    pub entries: Vec<Entry>,
}

pub async fn subscription_nodes(
    connection: &mut PgConnection,
    user_id: i64,
) -> ApiResult<SubscriptionNodes> {
    let bindings = bindings(connection, user_id).await?;
    let states = states(
        connection,
        Some(
            &bindings
                .iter()
                .map(|v| v.external_node_id)
                .collect::<Vec<_>>(),
        ),
    )
    .await?;
    let ids = bindings
        .iter()
        .filter_map(|binding| {
            if binding.update_mode == "pinned" {
                Some(binding.node_version_id)
            } else {
                states.get(&binding.external_node_id)?.current_version_id
            }
        })
        .collect();
    let versions = versions(connection, ids).await?;
    let mut result = SubscriptionNodes {
        granted: bindings.len(),
        nodes: Vec::new(),
        entries: Vec::new(),
    };
    for binding in bindings {
        let state = states.get(&binding.external_node_id).ok_or_else(|| {
            ApiError::Internal(anyhow::anyhow!("external authorization identity missing"))
        })?;
        let version = if binding.update_mode == "pinned" {
            versions.get(&binding.node_version_id)
        } else {
            state.current_version_id.and_then(|id| versions.get(&id))
        };
        let entry = state.entry(binding.reference(state.metadata_revision), version, false);
        if entry.available
            && let Some(outbound) = version.and_then(Version::outbound)
        {
            result.nodes.push(ExternalClientNode {
                id: state.id,
                name: state.name.clone(),
                sort_order: state.sort_order,
                outbound,
            });
        }
        result.entries.push(entry);
    }
    result.nodes.sort_by_key(|node| (node.sort_order, node.id));
    result.entries.sort_by_key(|entry| {
        (
            states[&entry.reference.external_node_id].sort_order,
            entry.reference.external_node_id,
        )
    });
    Ok(result)
}
