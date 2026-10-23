use crate::error::{ApiError, ApiResult};
use apsp_compiler::paths::{Hop, Path};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub(crate) fn path_error(error: apsp_compiler::CompileError) -> ApiError {
    let reason = match &error {
        apsp_compiler::CompileError::InvalidNode { reason, .. } => reason.as_str(),
        _ => "",
    };
    let message = match reason {
        "a path requires one to eight ordered hops and positive identities" => {
            "每条链路需要 1 至 8 个有效的内部或最终节点"
        }
        "managed hops must be distinct enabled Reality endpoints on other servers" => {
            "受管段必须是其他服务器上的已启用 Reality 节点，不能重复或使用公开入口"
        }
        "a path cannot revisit the same endpoint" | "path returns to its public entry endpoint" => {
            "路径不能重复访问同一端点或返回公开入口"
        }
        "the ordered lower transports cannot carry a TCP connection through every hop" => {
            "相邻段的 TCP/UDP 承载不兼容，请调整顺序或更换节点"
        }
        "managed endpoint differs from its immutable listening snapshot" => {
            "受管端点已偏离保存的监听配置，请创建替代节点和链路"
        }
        "invalid imported endpoint configuration"
        | "unsupported imported path endpoint"
        | "path outbound cannot be compiled" => "导入节点包含不能安全编译的协议或参数",
        "invalid protected path control configuration"
        | "path verification requires a protected local control configuration" => {
            "入口的 HTTPS 验证地址或本机管理端口配置无效"
        }
        "path verification or recovery metadata exceeds its runtime budget" => {
            "链路验证或恢复信息超过运行时预算，请减少本机链路数量"
        }
        _ => "链路配置无效，请检查节点协议、端点和监听端口是否冲突",
    };
    ApiError::BadRequest(message.into())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BatchRequest {
    pub request_id: Uuid,
    pub items: Vec<ChainInput>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChainInput {
    pub name: String,
    pub entry: EntryInput,
    pub hops: Vec<HopInput>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum EntryInput {
    New {
        server_id: i64,
        public_host: String,
        sni: String,
        port: Option<u16>,
    },
    Existing {
        node_id: i64,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HopInput {
    Managed {
        node_id: i64,
    },
    Subscription {
        source_id: i64,
        external_node_id: i64,
        node_version_id: i64,
        update_mode: UpdateMode,
    },
}

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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Receipt {
    pub request_id: Uuid,
    pub chain_ids: Vec<i64>,
    pub entry_node_ids: Vec<i64>,
}

pub(crate) async fn node_on(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
) -> ApiResult<super::super::business::NodeRow> {
    let query = format!(
        "SELECT {} FROM nodes n JOIN servers s ON s.id=n.server_id WHERE n.id=$1 AND n.deleted_at IS NULL AND s.deleted_at IS NULL",
        super::super::business::NODE_COLUMNS
    );
    sqlx::query_as(&query)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}

pub(crate) async fn insert_version_on(
    tx: &mut Transaction<'_, Postgres>,
    path: &Path,
    inputs: &[HopInput],
    previous: Option<i64>,
) -> ApiResult<()> {
    let networks = apsp_compiler::paths::validate(path).map_err(path_error)?;
    let value = serde_json::to_value(path).map_err(anyhow::Error::from)?;
    let hash =
        crate::auth::hash_token(&serde_json::to_string(&value).map_err(anyhow::Error::from)?);
    let generation = i64::try_from(path.generation).map_err(anyhow::Error::from)?;
    let at = apsp_protocol::now_timestamp();
    sqlx::query("INSERT INTO singbox_chain_versions(chain_id,generation,previous_generation,path_json,semantic_hash,networks,stage,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'waiting_dependencies',$7,$7)")
        .bind(path.chain_id).bind(generation).bind(previous).bind(value).bind(hash).bind(serde_json::to_value(networks).map_err(anyhow::Error::from)?).bind(at).execute(&mut **tx).await?;
    for (position, (hop, input)) in path.hops.iter().zip(inputs).enumerate() {
        match (hop, input) {
            (
                Hop::Managed {
                    server_id,
                    endpoint,
                    identity,
                },
                HopInput::Managed { node_id },
            ) if endpoint.id == *node_id => {
                sqlx::query("INSERT INTO singbox_chain_hops(chain_id,generation,position,kind,managed_node_id,managed_server_id,endpoint_json,relay_uuid) VALUES($1,$2,$3,'managed',$4,$5,$6,$7)")
                    .bind(path.chain_id).bind(generation).bind(position as i32).bind(node_id).bind(server_id).bind(serde_json::to_value(endpoint).map_err(anyhow::Error::from)?).bind(identity).execute(&mut **tx).await?;
            }
            (
                Hop::External {
                    node_id,
                    version_id,
                    ..
                },
                HopInput::Subscription {
                    source_id,
                    external_node_id,
                    node_version_id,
                    update_mode,
                },
            ) if node_id == external_node_id && version_id == node_version_id => {
                sqlx::query("INSERT INTO singbox_chain_hops(chain_id,generation,position,kind,source_id,external_node_id,external_version_id,update_mode) VALUES($1,$2,$3,'subscription',$4,$5,$6,$7)")
                    .bind(path.chain_id).bind(generation).bind(position as i32).bind(source_id).bind(node_id).bind(version_id).bind(update_mode.as_str()).execute(&mut **tx).await?;
            }
            _ => return Err(ApiError::BadRequest("链路快照与来源引用不一致".into())),
        }
    }
    sqlx::query("UPDATE singbox_chains SET pending_generation=$2 WHERE id=$1")
        .bind(path.chain_id)
        .bind(generation)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn hop_inputs_on(
    tx: &mut Transaction<'_, Postgres>,
    chain: i64,
    generation: i64,
) -> ApiResult<Vec<HopInput>> {
    let rows = sqlx::query(
        "SELECT * FROM singbox_chain_hops WHERE chain_id=$1 AND generation=$2 ORDER BY position",
    )
    .bind(chain)
    .bind(generation)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| match row.get::<String, _>("kind").as_str() {
            "managed" => Ok(HopInput::Managed {
                node_id: row.get("managed_node_id"),
            }),
            "subscription" => Ok(HopInput::Subscription {
                source_id: row.get("source_id"),
                external_node_id: row.get("external_node_id"),
                node_version_id: row.get("external_version_id"),
                update_mode: if row.get::<String, _>("update_mode") == "follow_node" {
                    UpdateMode::FollowNode
                } else {
                    UpdateMode::Pinned
                },
            }),
            _ => Err(ApiError::Internal(anyhow::anyhow!(
                "invalid persisted hop kind"
            ))),
        })
        .collect()
}

pub(crate) fn servers(path: &Path) -> Vec<i64> {
    let mut hosts = vec![path.entry_server_id];
    hosts.extend(path.hops.iter().filter_map(|hop| {
        if let Hop::Managed { server_id, .. } = hop {
            Some(*server_id)
        } else {
            None
        }
    }));
    hosts.sort_unstable();
    hosts.dedup();
    hosts
}

/// The mixed generation a legacy chain received when it was created; only
/// test fixtures create legacy chains now.
#[cfg(test)]
pub(crate) async fn seed_legacy_on(tx: &mut Transaction<'_, Postgres>, id: i64) -> ApiResult<()> {
    let (entry,exit,identity):(i64,i64,Uuid)=sqlx::query_as("SELECT entry_node_id,exit_node_id,relay_uuid FROM singbox_live_chains WHERE id=$1 AND path_kind='legacy'").bind(id).fetch_one(&mut **tx).await?;
    let entry = node_on(tx, entry).await?;
    let exit = node_on(tx, exit).await?;
    let endpoint = exit.model(Vec::new())?;
    let path = Path {
        chain_id: id,
        generation: 1,
        entry_server_id: entry.server_id,
        entry_node_id: entry.id,
        active: true,
        hops: vec![Hop::Managed {
            server_id: exit.server_id,
            endpoint: Box::new(endpoint.clone()),
            identity,
        }],
    };
    sqlx::query("INSERT INTO singbox_chain_versions(chain_id,generation,legacy,path_json,semantic_hash,networks,stage,created_at,updated_at) VALUES($1,1,TRUE,$2,'legacy-preserved','{\"tcp\":true,\"udp\":true}','active',$3,$3)").bind(id).bind(serde_json::to_value(&path).map_err(anyhow::Error::from)?).bind(apsp_protocol::now_timestamp()).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO singbox_chain_hops(chain_id,generation,position,kind,managed_node_id,managed_server_id,endpoint_json,relay_uuid) VALUES($1,1,0,'managed',$2,$3,$4,$5)").bind(id).bind(exit.id).bind(exit.server_id).bind(serde_json::to_value(endpoint).map_err(anyhow::Error::from)?).bind(identity).execute(&mut **tx).await?;
    sqlx::query("UPDATE singbox_chains SET active_generation=1 WHERE id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn validate_frozen_on(
    tx: &mut Transaction<'_, Postgres>,
    paths: &[Path],
) -> ApiResult<()> {
    let mut active = paths.to_vec();
    for path in &mut active {
        path.active = true;
    }
    let hosts: std::collections::BTreeSet<i64> = paths.iter().flat_map(servers).collect();
    for server in hosts {
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM singbox_live_chains c JOIN nodes n ON n.id=c.entry_node_id WHERE c.path_kind='mixed' AND n.server_id=$1").bind(server).fetch_one(&mut **tx).await?;
        if count > 64 {
            return Err(ApiError::Conflict(
                "同一入口服务器最多保留 64 条混合链路，以预留候选验证预算".into(),
            ));
        }
        // Converted chains keep their scopes on the device as tombstones.
        let scopes:i64=sqlx::query_scalar("SELECT (SELECT COUNT(DISTINCT c.id) FROM singbox_chains c JOIN nodes n ON n.id=c.entry_node_id LEFT JOIN singbox_chain_hops h ON h.chain_id=c.id WHERE c.path_kind='mixed' AND (n.server_id=$1 OR h.managed_server_id=$1))+(SELECT COUNT(*) FROM singbox_retired_path_scopes t JOIN singbox_chains c ON c.id=t.chain_id WHERE t.server_id=$1 AND c.path_kind<>'mixed')").bind(server).fetch_one(&mut **tx).await?;
        if scopes > 1024 {
            return Err(ApiError::Conflict(
                "服务器的路径恢复记录已达到上限，无法再建立新的链路引用".into(),
            ));
        }
        let query = format!(
            "SELECT {} FROM nodes n WHERE n.server_id=$1 AND n.deleted_at IS NULL ORDER BY n.id",
            super::super::business::NODE_COLUMNS
        );
        let rows: Vec<super::super::business::NodeRow> = sqlx::query_as(&query)
            .bind(server)
            .fetch_all(&mut **tx)
            .await?;
        let nodes = rows
            .iter()
            .map(|node| node.model(Vec::new()))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let control = sqlx::query_as::<_, (String, String)>(
            "SELECT secret,test_url FROM singbox_path_controls WHERE server_id=$1",
        )
        .bind(server)
        .fetch_optional(&mut **tx)
        .await?
        .map(|(secret, test_url)| apsp_compiler::paths::Control { secret, test_url });
        apsp_compiler::paths::compile(
            server,
            &nodes,
            &[],
            &active,
            &[],
            Default::default(),
            control.as_ref(),
        )
        .map_err(path_error)?;
    }
    Ok(())
}
