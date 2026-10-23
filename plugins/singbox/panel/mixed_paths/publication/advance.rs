use crate::{AppState, error::ApiResult};
use apsp_compiler::paths::{PATH_CAPABILITY, Path, scope};
use apsp_protocol::now_timestamp;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

enum Proof {
    Waiting,
    Ready,
    Failed,
}

pub async fn advance(state: &AppState) -> ApiResult<()> {
    let mut tx = state.pool.begin().await?;
    super::super::super::entitlements::lock(&mut tx).await?;
    let rows=sqlx::query("SELECT v.*,c.active_generation,c.minimum_generation,singbox_path_resources_available(c.id) AS available FROM singbox_live_chains c JOIN singbox_chain_versions v ON v.chain_id=c.id AND v.generation=c.pending_generation WHERE c.path_kind='mixed' AND v.stage NOT IN ('failed','active') ORDER BY c.id FOR UPDATE OF c,v").fetch_all(&mut *tx).await?;
    for row in rows {
        let path: Path =
            serde_json::from_value(row.get("path_json")).map_err(anyhow::Error::from)?;
        let stage: String = row.get("stage");
        let generation: i64 = row.get("generation");
        let active: Option<i64> = row.get("active_generation");
        let hosts = super::super::models::servers(&path);
        if !row.get::<bool, _>("available") {
            reason(
                &mut tx,
                &path,
                "受管节点已停用或删除；入口授权已经撤下，等待恢复资源",
            )
            .await?;
            continue;
        }
        let missing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM servers WHERE id=ANY($1) AND NOT capabilities ? $2)",
        )
        .bind(&hosts)
        .bind(PATH_CAPABILITY)
        .fetch_one(&mut *tx)
        .await?;
        if missing {
            reason(
                &mut tx,
                &path,
                "请先升级所有受管服务器的 Agent；当前设备缺少路径验证与恢复保护能力",
            )
            .await?;
            continue;
        }
        let dependencies: Vec<i64> = hosts
            .iter()
            .copied()
            .filter(|id| *id != path.entry_server_id)
            .collect();
        if stage == "waiting_dependencies" {
            // A capability upgrade can arrive after a blocked publication cleared
            // dirty_at. Schedule the never-prepared dependencies once, preserving
            // the original debounce time on subsequent polling turns.
            sqlx::query("UPDATE servers s SET dirty_at=COALESCE(dirty_at,FLOOR(EXTRACT(EPOCH FROM clock_timestamp())*1000)::bigint) WHERE s.id=ANY($1) AND s.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM singbox_path_deployments d WHERE d.server_id=s.id AND d.chain_id=$2 AND d.generation=$3 AND d.role='dependency')").bind(&dependencies).bind(path.chain_id).bind(generation).execute(&mut *tx).await?;
        }
        match stage.as_str() {
            "waiting_dependencies" => match all_ready(&mut tx, &path, &dependencies, "dependency")
                .await?
            {
                Proof::Ready => {
                    transition(&mut tx, &path, "preparing_entry", &[path.entry_server_id]).await?
                }
                Proof::Failed => {
                    fail(&mut tx, &path, "内部节点准备失败，请检查其配置部署结果").await?
                }
                Proof::Waiting => {}
            },
            "preparing_entry" => {
                match ready(&mut tx, &path, path.entry_server_id, "candidate").await? {
                    Proof::Ready => transition(&mut tx, &path, "checking_candidate", &[]).await?,
                    Proof::Failed => {
                        fail(&mut tx, &path, "入口候选配置应用失败，保留原路径并撤回候选").await?
                    }
                    Proof::Waiting => {}
                }
            }
            "checking_candidate" => {
                match validate(&mut tx, &path, path.entry_server_id, "candidate", "probe").await? {
                    Proof::Ready => {
                        transition(&mut tx, &path, "switching_entry", &[path.entry_server_id])
                            .await?
                    }
                    Proof::Failed => {
                        fail(&mut tx, &path, "候选路径连接验证失败，保留原路径").await?
                    }
                    Proof::Waiting => {}
                }
            }
            "switching_entry" => match ready(&mut tx, &path, path.entry_server_id, "active").await?
            {
                Proof::Ready => transition(&mut tx, &path, "checking_active", &[]).await?,
                Proof::Failed => fail(&mut tx, &path, "入口切换失败，正在恢复原路径").await?,
                Proof::Waiting => {}
            },
            "checking_active" => {
                match validate(&mut tx, &path, path.entry_server_id, "active", "probe").await? {
                    Proof::Ready => transition(&mut tx, &path, "establishing_barrier", &[]).await?,
                    Proof::Failed => {
                        fail(&mut tx, &path, "切换后的路径验证失败，正在恢复原路径").await?
                    }
                    Proof::Waiting => {}
                }
            }
            "establishing_barrier" => {
                // Once any floor is recorded, rollback could resurrect removed
                // identities. Retry durable barriers; never guess from a timeout.
                let mut complete = true;
                for host in &hosts {
                    let role = if *host == path.entry_server_id {
                        "active"
                    } else {
                        "dependency"
                    };
                    complete &= matches!(
                        validate(&mut tx, &path, *host, role, "barrier").await?,
                        Proof::Ready
                    );
                }
                if complete {
                    sqlx::query("UPDATE singbox_chains SET active_generation=$2,minimum_generation=GREATEST(minimum_generation,$2) WHERE id=$1").bind(path.chain_id).bind(generation).execute(&mut *tx).await?;
                    transition(&mut tx, &path, "retiring_old", &hosts).await?;
                }
            }
            "retiring_old" => {
                let previous: Option<i64> = row.get("previous_generation");
                let mut complete = true;
                if let Some(previous) = previous {
                    let mut old = path.clone();
                    old.generation = previous.try_into().map_err(anyhow::Error::from)?;
                    for host in &hosts {
                        complete &=
                            matches!(ready(&mut tx, &old, *host, "retired").await?, Proof::Ready);
                    }
                } else {
                    complete = matches!(
                        all_ready(&mut tx, &path, &dependencies, "dependency").await?,
                        Proof::Ready
                    ) && matches!(
                        ready(&mut tx, &path, path.entry_server_id, "active").await?,
                        Proof::Ready
                    );
                }
                if complete {
                    if let Some(previous) = previous {
                        sqlx::query("UPDATE singbox_chain_versions SET stage='retired',updated_at=$3 WHERE chain_id=$1 AND generation=$2").bind(path.chain_id).bind(previous).bind(now_timestamp()).execute(&mut *tx).await?;
                    }
                    sqlx::query("UPDATE singbox_chains SET pending_generation=NULL WHERE id=$1")
                        .bind(path.chain_id)
                        .execute(&mut *tx)
                        .await?;
                    transition(&mut tx, &path, "active", &[]).await?;
                }
            }
            "rolling_back" => {
                let entry_restored = if let Some(previous) = active {
                    let mut old = path.clone();
                    old.generation = previous.try_into().map_err(anyhow::Error::from)?;
                    matches!(
                        ready(&mut tx, &old, path.entry_server_id, "active").await?,
                        Proof::Ready
                    )
                } else {
                    matches!(
                        ready(&mut tx, &path, path.entry_server_id, "retired").await?,
                        Proof::Ready
                    )
                };
                if entry_restored {
                    sqlx::query("UPDATE singbox_chain_versions SET stage='failed',updated_at=$3 WHERE chain_id=$1 AND generation=$2").bind(path.chain_id).bind(generation).bind(now_timestamp()).execute(&mut *tx).await?;
                    super::super::super::business::mark_dirty(&mut tx, &hosts).await?;
                }
            }
            _ => {}
        }
    }
    tx.commit().await?;
    Ok(())
}

async fn reason(tx: &mut Transaction<'_, Postgres>, path: &Path, message: &str) -> ApiResult<()> {
    sqlx::query("UPDATE singbox_chain_versions SET last_error=$3 WHERE chain_id=$1 AND generation=$2 AND last_error IS DISTINCT FROM $3").bind(path.chain_id).bind(path.generation as i64).bind(message).execute(&mut **tx).await?;
    Ok(())
}
async fn transition(
    tx: &mut Transaction<'_, Postgres>,
    path: &Path,
    stage: &str,
    hosts: &[i64],
) -> ApiResult<()> {
    sqlx::query("UPDATE singbox_chain_versions SET stage=$3,last_error=NULL,updated_at=$4 WHERE chain_id=$1 AND generation=$2").bind(path.chain_id).bind(path.generation as i64).bind(stage).bind(now_timestamp()).execute(&mut **tx).await?;
    if !hosts.is_empty() {
        super::super::super::business::mark_dirty(tx, hosts).await?;
    }
    Ok(())
}
async fn fail(tx: &mut Transaction<'_, Postgres>, path: &Path, message: &str) -> ApiResult<()> {
    sqlx::query("UPDATE singbox_chain_versions SET failure_stage=stage,stage='rolling_back',last_error=$3,updated_at=$4 WHERE chain_id=$1 AND generation=$2").bind(path.chain_id).bind(path.generation as i64).bind(message).bind(now_timestamp()).execute(&mut **tx).await?;
    super::super::super::business::mark_dirty(tx, &[path.entry_server_id]).await?;
    Ok(())
}

async fn all_ready(
    tx: &mut Transaction<'_, Postgres>,
    path: &Path,
    hosts: &[i64],
    role: &str,
) -> ApiResult<Proof> {
    let mut pending = false;
    for host in hosts {
        match ready(tx, path, *host, role).await? {
            Proof::Failed => return Ok(Proof::Failed),
            Proof::Waiting => pending = true,
            Proof::Ready => {}
        }
    }
    Ok(if pending {
        Proof::Waiting
    } else {
        Proof::Ready
    })
}

async fn applied(
    tx: &mut Transaction<'_, Postgres>,
    path: &Path,
    host: i64,
    role: &str,
) -> ApiResult<Option<(i64, String)>> {
    Ok(sqlx::query_as("SELECT d.rev,d.bundle_sha256 FROM singbox_path_deployments d JOIN server_module_status m ON m.server_id=d.server_id AND m.module='singbox' AND m.applied_rev=d.rev JOIN servers s ON s.id=d.server_id JOIN deployments b ON b.server_id=d.server_id AND b.module='singbox' AND b.rev=d.rev AND b.bundle_sha256=d.bundle_sha256 WHERE d.chain_id=$1 AND d.generation=$2 AND d.server_id=$3 AND d.role=$4 AND m.healthy AND m.applied_rev=m.target_rev AND s.dirty_at IS NULL AND s.deleted_at IS NULL AND s.last_seen>=$5 ORDER BY d.rev DESC LIMIT 1")
        .bind(path.chain_id).bind(path.generation as i64).bind(host).bind(role).bind(now_timestamp()-60).fetch_optional(&mut **tx).await?)
}

async fn ready(
    tx: &mut Transaction<'_, Postgres>,
    path: &Path,
    host: i64,
    role: &str,
) -> ApiResult<Proof> {
    if applied(tx, path, host, role).await?.is_some() {
        return Ok(Proof::Ready);
    }
    let failed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_path_deployments d JOIN server_module_status m ON m.server_id=d.server_id AND m.module='singbox' AND m.target_rev=d.rev AND m.last_result_rev=d.rev JOIN servers s ON s.id=d.server_id WHERE d.chain_id=$1 AND d.generation=$2 AND d.server_id=$3 AND d.role=$4 AND m.last_error IS NOT NULL AND s.dirty_at IS NULL)").bind(path.chain_id).bind(path.generation as i64).bind(host).bind(role).fetch_one(&mut **tx).await?;
    Ok(if failed {
        Proof::Failed
    } else {
        Proof::Waiting
    })
}

async fn validate(
    tx: &mut Transaction<'_, Postgres>,
    path: &Path,
    host: i64,
    role: &str,
    operation: &str,
) -> ApiResult<Proof> {
    let Some((revision, hash)) = applied(tx, path, host, role).await? else {
        return Ok(Proof::Waiting);
    };
    let previous=sqlx::query("SELECT result,expires_at FROM runtime_validations WHERE server_id=$1 AND module='singbox' AND scope=$2 AND generation=$3 AND operation=$4 AND revision=$5 AND config_hash=$6 ORDER BY expires_at DESC,requested_at DESC,id DESC LIMIT 1")
        .bind(host).bind(scope(path.chain_id)).bind(path.generation as i64).bind(operation).bind(revision).bind(&hash).fetch_optional(&mut **tx).await?;
    if let Some(row) = previous {
        let result: Option<serde_json::Value> = row.get("result");
        if let Some(result) = result {
            if result["success"] == true {
                return Ok(Proof::Ready);
            }
            if operation == "probe" {
                return Ok(Proof::Failed);
            }
            if row.get::<i64, _>("expires_at") > now_timestamp() {
                return Ok(Proof::Waiting);
            }
        } else if row.get::<i64, _>("expires_at") > now_timestamp() {
            return Ok(Proof::Waiting);
        } else if operation == "probe" {
            return Ok(Proof::Failed);
        }
    }
    let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM runtime_validations WHERE server_id=$1 AND result IS NULL AND expires_at>$2").bind(host).bind(now_timestamp()).fetch_one(&mut **tx).await?;
    if pending >= 4 {
        return Ok(Proof::Waiting);
    }
    sqlx::query("INSERT INTO runtime_validations(id,server_id,module,scope,generation,operation,revision,config_hash,expires_at,requested_at) VALUES($1,$2,'singbox',$3,$4,$5,$6,$7,$8,$9)")
        .bind(Uuid::new_v4()).bind(host).bind(scope(path.chain_id)).bind(path.generation as i64).bind(operation).bind(revision).bind(hash).bind(now_timestamp()+300).bind(now_timestamp()).execute(&mut **tx).await?;
    Ok(Proof::Waiting)
}
