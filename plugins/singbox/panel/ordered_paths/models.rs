use apsp_compiler::{ManagedEndpointSnapshot, Node, PathHop};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub tcp: bool,
    pub udp: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenVersion {
    pub entry: ManagedEndpointSnapshot,
    pub hops: Vec<FrozenHop>,
    pub legacy_relay_uuid: Option<Uuid>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum FrozenHop {
    Managed {
        endpoint: Box<ManagedEndpointSnapshot>,
        relay_uuid: Uuid,
    },
    Subscription {
        source_id: i64,
        identity_epoch: i64,
        external_node_id: Uuid,
        node_version_id: Uuid,
        source_revision_id: Uuid,
        update_mode: String,
        outbound: Box<apsp_compiler::external::NormalizedOutbound>,
        content_digest: String,
    },
}
impl FrozenHop {
    pub(crate) fn compiler(&self) -> PathHop {
        match self {
            Self::Managed {
                endpoint,
                relay_uuid,
            } => PathHop::Managed {
                endpoint: endpoint.clone(),
                relay_uuid: *relay_uuid,
            },
            Self::Subscription {
                source_id,
                identity_epoch,
                external_node_id,
                node_version_id,
                source_revision_id,
                outbound,
                ..
            } => PathHop::External {
                source_id: *source_id,
                identity_epoch: *identity_epoch,
                node_id: *external_node_id,
                version_id: *node_version_id,
                source_revision_id: *source_revision_id,
                outbound: outbound.clone(),
            },
        }
    }
}

#[derive(FromRow)]
pub(crate) struct ChainRow {
    pub id: i64,
    pub name: String,
    pub entry_node_id: i64,
    pub exit_node_id: Option<i64>,
    pub path_kind: String,
    pub settings_revision: i64,
    pub desired_generation: i64,
    pub applied_generation: Option<i64>,
    pub candidate_generation: Option<i64>,
    pub recovery_generation: Option<i64>,
    pub minimum_generation: i64,
    pub phase: String,
    pub last_error: Option<String>,
    pub deleted_at: Option<i64>,
    pub route_enabled: bool,
    pub last_granted: bool,
    pub restore_step: Option<String>,
}
pub(crate) const CHAIN_COLUMNS: &str = "id,name,entry_node_id,exit_node_id,path_kind,settings_revision,desired_generation,applied_generation,candidate_generation,recovery_generation,minimum_generation,phase,last_error,deleted_at,route_enabled,last_granted,restore_step";

#[derive(FromRow)]
pub(crate) struct VersionRow {
    pub generation: i64,
    pub legacy: bool,
    #[sqlx(json)]
    pub capabilities: Capabilities,
    #[sqlx(json)]
    pub snapshot: FrozenVersion,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PublicHop {
    Managed {
        position: usize,
        node_id: i64,
        endpoint_version_id: Uuid,
        endpoint: super::super::proxy_resources::ResourceEndpoint,
    },
    Subscription {
        position: usize,
        source_id: i64,
        source_name: String,
        identity_epoch: i64,
        external_node_id: Uuid,
        node_version_id: Uuid,
        source_revision_id: Uuid,
        update_mode: String,
        name: String,
        protocol: String,
        server: String,
        server_port: u16,
        sni: Option<String>,
        transport: Option<String>,
        capabilities: Capabilities,
        source_archived: bool,
        node_present: bool,
        update_error: Option<String>,
    },
}
#[derive(Serialize)]
pub struct DependencyView {
    pub server_id: i64,
    pub role: String,
    pub hop_position: Option<i32>,
    pub generation: i64,
    pub stage: String,
    pub required_revision: Option<i64>,
    pub applied_revision: Option<i64>,
    pub bundle_sha256: Option<String>,
    pub state: String,
    pub observed_at: Option<i64>,
}
#[derive(Serialize)]
pub struct ProbeView {
    pub stage: String,
    pub request_id: Uuid,
    pub state: String,
    pub observed_at: Option<i64>,
    pub error: Option<String>,
}
#[derive(Serialize)]
pub struct PathState {
    pub desired_generation: i64,
    pub candidate_generation: Option<i64>,
    pub applied_generation: Option<i64>,
    pub recovery_generation: Option<i64>,
    pub minimum_generation: i64,
    pub phase: String,
    pub capabilities: Capabilities,
    pub last_error: Option<String>,
    pub dependencies: Vec<DependencyView>,
    pub probe: Option<ProbeView>,
    pub generations: Vec<GenerationView>,
}
#[derive(Serialize)]
pub struct GenerationView {
    pub generation: i64,
    pub state: String,
    pub hops: Vec<PublicHop>,
}
#[derive(Serialize, FromRow)]
pub struct SourceDependency {
    pub chain_id: i64,
    pub chain_name: String,
    pub generation: i64,
    pub state: String,
    pub hop_position: i32,
    pub external_node_id: Uuid,
    pub node_version_id: Uuid,
    pub identity_epoch: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceUpdate {
    pub request_id: Uuid,
    pub settings_revision: i64,
    pub name: Option<String>,
    pub entry_name: Option<String>,
    pub public_host: Option<String>,
    pub port: Option<i64>,
    pub sni: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyVersions {
    pub request_id: Uuid,
    pub settings_revision: i64,
    pub generation: i64,
    pub versions: Vec<VersionSelection>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionSelection {
    pub hop_position: usize,
    pub node_version_id: Uuid,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct MutationReceipt {
    pub request_id: Uuid,
    pub kind: String,
    pub id: i64,
    pub settings_revision: i64,
    pub generation: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct ServerSnapshot {
    pub schema_version: u8,
    pub public_nodes: Vec<Node>,
    pub accounting_users: Vec<AccountingUser>,
    pub path_dependencies: Vec<SnapshotDependency>,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct AccountingUser {
    pub user_id: i64,
    pub node_id: i64,
    pub stat_name: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SnapshotDependency {
    pub chain_id: i64,
    pub generation: i64,
    pub role: String,
    pub hop_position: i32,
    pub route_active: bool,
}
pub(crate) fn public_nodes(value: Value) -> Result<Vec<Node>, serde_json::Error> {
    if value.is_array() {
        serde_json::from_value(value)
    } else {
        serde_json::from_value::<ServerSnapshot>(value).map(|snapshot| snapshot.public_nodes)
    }
}
