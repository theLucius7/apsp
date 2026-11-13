use crate::AppState;
use crate::error::ApiError;
use crate::error::ApiResult;
use apsp_compiler::limits::{UserLimits, apply_user_limits};
use apsp_traffic::business;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    expected_revision: i64,
    upload_mbps: u32,
    download_mbps: u32,
    max_connections: u32,
}

async fn current(tx: &mut Transaction<'_, Postgres>, id: i64) -> ApiResult<Value> {
    let value = sqlx::query_scalar("SELECT jsonb_build_object('user_id',u.id,'revision',COALESCE(l.revision,0),'upload_mbps',COALESCE(l.upload_mbps,0),'download_mbps',COALESCE(l.download_mbps,0),'max_connections',COALESCE(l.max_connections,0)) FROM users u LEFT JOIN singbox_user_limits l ON l.user_id=u.id WHERE u.id=$1 AND u.deleted_at IS NULL")
        .bind(id).fetch_optional(&mut **tx).await?;
    value.ok_or(ApiError::NotFound)
}

// Include both ordinary and taken-over nodes, and hosts still holding revoked
// credentials. Publication reuses the same entitlement lock as this write.
pub(crate) async fn servers(tx: &mut Transaction<'_, Postgres>, id: i64) -> ApiResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT DISTINCT n.server_id FROM nodes n JOIN (SELECT node_id FROM accesses WHERE user_id=$1 UNION SELECT node_id FROM singbox_desired_accesses WHERE user_id=$1 UNION SELECT pn.node_id FROM singbox_user_policies up JOIN singbox_policy_nodes pn ON pn.group_id=up.group_id WHERE up.user_id=$1) a ON a.node_id=n.id JOIN servers s ON s.id=n.server_id WHERE s.deleted_at IS NULL ORDER BY n.server_id")
        .bind(id).fetch_all(&mut **tx).await?)
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_capability(&state, &headers, "proxy:read").await?;
    let mut tx = apsp_traffic::read_snapshot(&state.pool).await?;
    let mut value = current(&mut tx, id).await?;
    let ids = servers(&mut tx, id).await?;
    let rows = sqlx::query("SELECT s.id,s.name,s.dirty_at,s.last_seen,m.target_rev,m.applied_rev,m.healthy,m.updated_at,d.bundle FROM servers s LEFT JOIN server_module_status m ON m.server_id=s.id AND m.module='singbox' LEFT JOIN deployments d ON d.server_id=s.id AND d.module='singbox' AND d.rev=m.applied_rev WHERE s.id=ANY($1) ORDER BY s.id")
        .bind(&ids).fetch_all(&mut *tx).await?;
    let wanted = UserLimits {
        upload_mbps: value["upload_mbps"].as_u64().unwrap_or(0) as u32,
        download_mbps: value["download_mbps"].as_u64().unwrap_or(0) as u32,
        max_connections: value["max_connections"].as_u64().unwrap_or(0) as u32,
    };
    let mut observations = Vec::new();
    for row in rows {
        let config: Option<String> = row.try_get("bundle")?;
        let applied = config
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|bundle| {
                bundle["files"]["config.json"]
                    .as_str()
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
            });
        let matches = applied
            .as_ref()
            .is_some_and(|config| matches_limit(config, id, wanted));
        let fresh = row
            .get::<Option<i64>, _>("last_seen")
            .is_some_and(|at| apsp_protocol::now_timestamp().saturating_sub(at) <= 60)
            && row
                .get::<Option<i64>, _>("updated_at")
                .is_some_and(|at| apsp_protocol::now_timestamp().saturating_sub(at) <= 300);
        let applied_revision = row.get::<Option<i64>, _>("applied_rev");
        let confirmed = matches
            && fresh
            && row.get::<Option<i64>, _>("dirty_at").is_none()
            && row.get::<Option<bool>, _>("healthy") == Some(true)
            && applied_revision.is_some()
            && applied_revision == row.get::<Option<i64>, _>("target_rev");
        observations.push(json!({"server_id":row.get::<i64,_>("id"),"name":row.get::<String,_>("name"),"state":if confirmed {"device_confirmed"} else {"pending"},"applied_revision":applied_revision}));
    }
    value["servers"] = json!(observations);
    tx.commit().await?;
    Ok(Json(value))
}

fn matches_limit(config: &Value, id: i64, wanted: UserLimits) -> bool {
    let prefix = format!("u{id}_n");
    let Some(names) = config
        .pointer("/experimental/v2ray_api/stats/users")
        .and_then(Value::as_array)
    else {
        return false;
    };
    let names: Vec<&str> = names
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| name.starts_with(&prefix))
        .collect();
    if names.is_empty() {
        return false;
    }
    let entries = config
        .pointer("/route/apsp_user_limits")
        .and_then(Value::as_array);
    names.iter().all(|name| {
        let entry = entries.and_then(|entries| {
            entries.iter().find(|entry| {
                entry["principals"]
                    .as_array()
                    .is_some_and(|p| p.iter().any(|p| p == name))
            })
        });
        match entry {
            None => wanted == UserLimits::default(),
            Some(entry) => {
                entry["upload_bytes_per_second"].as_u64()
                    == Some(u64::from(wanted.upload_mbps) * 125_000)
                    && entry["download_bytes_per_second"].as_u64()
                        == Some(u64::from(wanted.download_mbps) * 125_000)
                    && entry["max_connections"].as_u64() == Some(u64::from(wanted.max_connections))
            }
        }
    })
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(request): Json<Request>,
) -> ApiResult<Json<Value>> {
    let administrator =
        crate::control_center::require_capability(&state, &headers, "proxy:write").await?;
    crate::control_center::require_recent_proof(&state, &headers).await?;
    let limits = UserLimits {
        upload_mbps: request.upload_mbps,
        download_mbps: request.download_mbps,
        max_connections: request.max_connections,
    };
    limits
        .validate()
        .map_err(|_| ApiError::BadRequest("限制必须是 0 至 1000000 的整数，0 表示不限".into()))?;
    if !(0..business::MAX_SAFE_INTEGER).contains(&request.expected_revision) {
        return Err(ApiError::BadRequest("配置版本无效".into()));
    }
    let mut tx = state.pool.begin().await?;
    apsp_traffic::business::lock_publication(&mut tx).await?;
    crate::proxy_users::lock_user(&mut tx, id).await?;
    let previous = current(&mut tx, id).await?;
    if previous["revision"].as_i64() != Some(request.expected_revision) {
        return Err(ApiError::Conflict(
            "限制已被其他操作修改，请刷新后重新确认".into(),
        ));
    }
    let hosts = servers(&mut tx, id).await?;
    sqlx::query("INSERT INTO singbox_user_limits(user_id,upload_mbps,download_mbps,max_connections,revision) VALUES($1,$2,$3,$4,$5) ON CONFLICT(user_id) DO UPDATE SET upload_mbps=EXCLUDED.upload_mbps,download_mbps=EXCLUDED.download_mbps,max_connections=EXCLUDED.max_connections,revision=EXCLUDED.revision")
        .bind(id).bind(limits.upload_mbps as i32).bind(limits.download_mbps as i32).bind(limits.max_connections as i32).bind(request.expected_revision+1).execute(&mut *tx).await?;
    business::mark_dirty(&mut tx, &hosts).await?;
    crate::operations_workflows::event(
        &mut tx,
        Some(administrator),
        Some(id),
        "user_limits_updated",
        json!({"before":previous,"after":limits,"scope":"per_server","server_ids":hosts}),
    )
    .await?;
    let value = current(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(value))
}

pub async fn compile(tx: &mut Transaction<'_, Postgres>, config: &str) -> anyhow::Result<String> {
    let rows = sqlx::query("SELECT user_id,upload_mbps,download_mbps,max_connections FROM singbox_user_limits WHERE upload_mbps>0 OR download_mbps>0 OR max_connections>0 ORDER BY user_id").fetch_all(&mut **tx).await?;
    let limits: BTreeMap<_, _> = rows
        .into_iter()
        .map(|row| {
            (
                row.get::<i64, _>("user_id"),
                UserLimits {
                    upload_mbps: row.get::<i32, _>("upload_mbps") as u32,
                    download_mbps: row.get::<i32, _>("download_mbps") as u32,
                    max_connections: row.get::<i32, _>("max_connections") as u32,
                },
            )
        })
        .collect();
    Ok(apply_user_limits(config, &limits)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "../panel/migrations")]
    async fn persisted_limits_reach_runtime_and_confirmation_checks(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<()> {
        let id: i64 = sqlx::query_scalar("INSERT INTO users(name,subscription_token) VALUES('TEST_ONLY limits','TEST_ONLY limits token') RETURNING id").fetch_one(&pool).await?;
        sqlx::query("INSERT INTO singbox_user_limits(user_id,upload_mbps,download_mbps,max_connections) VALUES($1,8,16,2)").bind(id).execute(&pool).await?;
        let config = json!({"experimental":{"v2ray_api":{"stats":{"users":[format!("u{id}_n1"),format!("u{id}_n2")]}}}}).to_string();
        let mut tx = pool.begin().await?;
        let compiled = compile(&mut tx, &config).await?;
        let compiled: Value = serde_json::from_str(&compiled)?;
        assert_eq!(
            compiled["route"]["apsp_user_limits"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let desired = UserLimits {
            upload_mbps: 8,
            download_mbps: 16,
            max_connections: 2,
        };
        assert!(matches_limit(&compiled, id, desired));
        assert!(!matches_limit(&compiled, id, UserLimits::default()));
        assert!(!matches_limit(&compiled, id + 1, desired));
        assert!(!matches_limit(&json!({}), id, UserLimits::default()));
        assert!(!matches_limit(&serde_json::from_str(&config)?, id, desired));
        tx.rollback().await?;
        Ok(())
    }
}
