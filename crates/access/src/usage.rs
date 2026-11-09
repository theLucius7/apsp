use crate::{
    AppState, agent_api, auth,
    error::{ApiError, ApiResult},
};
use anyhow::{Context, ensure};
use apsp_protocol::{Envelope, UsageAck, UsageBatch};
use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

/// Commits an immutable batch before sending its acknowledgement.
pub async fn ingest(state: &AppState, server_id: i64, mut batch: UsageBatch) -> anyhow::Result<()> {
    ensure!(
        batch.period_start >= 0 && batch.period_end >= batch.period_start,
        "invalid usage period"
    );
    ensure!(batch.records.len() <= 10_000, "too many usage records");
    batch.records.sort_by(|a, b| a.stat_name.cmp(&b.stat_name));
    ensure!(
        batch
            .records
            .windows(2)
            .all(|pair| pair[0].stat_name != pair[1].stat_name),
        "duplicate usage record"
    );
    let identities = batch
        .records
        .iter()
        .map(|record| parse_name(&record.stat_name))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&batch)?));
    let seq = batch.seq.to_string();
    let mut tx = state.pool.begin().await?;
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE id=$1 AND deleted_at IS NULL)",
    )
    .bind(server_id)
    .fetch_one(&mut *tx)
    .await?;
    ensure!(active, "unknown usage server");
    let inserted = sqlx::query("INSERT INTO usage_batches(server_id,epoch,seq,payload_hash,received_at) VALUES($1,$2,$3::text::numeric,$4,$5) ON CONFLICT DO NOTHING")
        .bind(server_id).bind(batch.epoch).bind(&seq).bind(&hash).bind(apsp_protocol::now_timestamp()).execute(&mut *tx).await?.rows_affected() != 0;
    if inserted {
        // Nearly every record names an identity of the newest deployment, so read
        // that snapshot and the server's nodes once. Anything else falls back to
        // the search through the whole deployment history below.
        let nodes: std::collections::HashSet<i64> =
            sqlx::query_scalar("SELECT id FROM nodes WHERE server_id=$1")
                .bind(server_id)
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .collect();
        let latest: Option<Value> = sqlx::query_scalar("SELECT source_json FROM deployments WHERE server_id=$1 AND module='singbox' ORDER BY rev DESC LIMIT 1")
            .bind(server_id).fetch_optional(&mut *tx).await?;
        for (record, (user_id, node_id)) in batch.records.iter().zip(&identities) {
            if nodes.contains(node_id)
                && latest
                    .as_ref()
                    .is_some_and(|source| publishes(source, *user_id, *node_id, &record.stat_name))
            {
                continue;
            }
            // Revoked identities remain valid for terminal samples and offline outbox replay.
            // Deployment history is retained, so search newest first: current identities match the
            // latest revision without decoding every historical snapshot. EXISTS would discard the
            // ordering, hence the scalar subquery with LIMIT.
            let authorized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1 AND server_id=$2) AND COALESCE((SELECT TRUE FROM deployments d WHERE d.server_id=$2 AND d.module='singbox' AND (d.source_json @> $3 OR d.source_json @> $4) ORDER BY d.rev DESC LIMIT 1),FALSE)")
                .bind(node_id).bind(server_id).bind(json!([{"id":node_id,"users":[{"user_id":user_id}]}])).bind(json!({"schema_version":1,"accounting_users":[{"user_id":user_id,"node_id":node_id,"stat_name":record.stat_name}]}))
                .fetch_one(&mut *tx).await?;
            ensure!(
                authorized,
                "usage identity was never published for this server"
            );
        }
        // One statement, so the daily rollup trigger aggregates the batch once.
        let stat_names: Vec<&str> = batch.records.iter().map(|r| r.stat_name.as_str()).collect();
        let uplinks: Vec<String> = batch.records.iter().map(|r| r.uplink.to_string()).collect();
        let downlinks: Vec<String> = batch
            .records
            .iter()
            .map(|r| r.downlink.to_string())
            .collect();
        let (users, nodes): (Vec<i64>, Vec<i64>) = identities.iter().copied().unzip();
        sqlx::query("INSERT INTO usage_records(server_id,epoch,seq,stat_name,user_id,node_id,uplink,downlink,period_start,period_end) SELECT $1,$2,$3::text::numeric,r.stat_name,r.user_id,r.node_id,r.uplink::numeric,r.downlink::numeric,$4,$5 FROM UNNEST($6::text[],$7::bigint[],$8::bigint[],$9::text[],$10::text[]) AS r(stat_name,user_id,node_id,uplink,downlink) ON CONFLICT DO NOTHING")
            .bind(server_id).bind(batch.epoch).bind(&seq).bind(batch.period_start).bind(batch.period_end)
            .bind(&stat_names).bind(&users).bind(&nodes).bind(&uplinks).bind(&downlinks)
            .execute(&mut *tx).await?;
    } else {
        let previous: String = sqlx::query_scalar("SELECT payload_hash FROM usage_batches WHERE server_id=$1 AND epoch=$2 AND seq=$3::text::numeric")
            .bind(server_id).bind(batch.epoch).bind(&seq).fetch_one(&mut *tx).await?;
        ensure!(previous == hash, "usage batch identity changed payload");
        sqlx::query("UPDATE usage_batches SET last_replayed_at=$4,replay_count=LEAST(replay_count,9223372036854775806)+1 WHERE server_id=$1 AND epoch=$2 AND seq=$3::text::numeric")
            .bind(server_id).bind(batch.epoch).bind(&seq).bind(apsp_protocol::now_timestamp()).execute(&mut *tx).await?;
    }
    crate::nexus::bridge::enqueue_usage(&mut tx, server_id, &batch).await?;
    tx.commit().await?;
    agent_api::notify(
        state,
        server_id,
        Envelope::new(
            "usage.ack",
            UsageAck {
                epoch: batch.epoch,
                seq: batch.seq,
            },
        )?,
    )
    .await;
    Ok(())
}

/// Whether a deployment's source snapshot contains this accounting identity,
/// in either stored form. Answers the same as the `@>` containment checks in
/// `ingest`, which still decide every identity this does not find.
fn publishes(source: &Value, user_id: i64, node_id: i64, stat_name: &str) -> bool {
    if let Some(nodes) = source.as_array() {
        return nodes.iter().any(|node| {
            node["id"].as_i64() == Some(node_id)
                && node["users"].as_array().is_some_and(|users| {
                    users
                        .iter()
                        .any(|user| user["user_id"].as_i64() == Some(user_id))
                })
        });
    }
    source["schema_version"].as_i64() == Some(1)
        && source["accounting_users"].as_array().is_some_and(|users| {
            users.iter().any(|user| {
                user["user_id"].as_i64() == Some(user_id)
                    && user["node_id"].as_i64() == Some(node_id)
                    && user["stat_name"].as_str() == Some(stat_name)
            })
        })
}

fn parse_name(name: &str) -> anyhow::Result<(i64, i64)> {
    let (user, node) = name
        .strip_prefix('u')
        .and_then(|name| name.split_once("_n"))
        .context("invalid usage identity")?;
    let user: i64 = user.parse().context("invalid usage user")?;
    let node: i64 = node.parse().context("invalid usage node")?;
    ensure!(
        user > 0 && node > 0 && name == format!("u{user}_n{node}"),
        "invalid usage identity"
    );
    Ok((user, node))
}

#[derive(Default, Deserialize)]
pub struct UsageQuery {
    pub user_id: Option<i64>,
    pub node_id: Option<i64>,
}

pub async fn summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UsageQuery>,
) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    if query.user_id.is_some_and(|id| id <= 0) || query.node_id.is_some_and(|id| id <= 0) {
        return Err(ApiError::BadRequest("用户和节点编号必须为正整数".into()));
    }
    let mut tx = apsp_traffic::read_snapshot(&state.pool).await?;
    // All-time totals read the per-day rollup that the ledger trigger maintains (ADR 0077).
    let totals = sqlx::query("SELECT COALESCE(SUM(uplink),0)::text AS uplink,COALESCE(SUM(downlink),0)::text AS downlink,(COALESCE(SUM(uplink),0)+COALESCE(SUM(downlink),0))::text AS total FROM singbox_usage_daily WHERE ($1::bigint IS NULL OR user_id=$1) AND ($2::bigint IS NULL OR node_id=$2)")
        .bind(query.user_id).bind(query.node_id).fetch_one(&mut *tx).await?;
    let users = sqlx::query("SELECT u.id,u.name,u.deleted_at IS NOT NULL AS deleted,SUM(r.uplink)::text AS uplink,SUM(r.downlink)::text AS downlink FROM singbox_usage_daily r JOIN users u ON u.id=r.user_id WHERE ($1::bigint IS NULL OR r.user_id=$1) AND ($2::bigint IS NULL OR r.node_id=$2) GROUP BY u.id ORDER BY u.id")
        .bind(query.user_id).bind(query.node_id).fetch_all(&mut *tx).await?;
    let nodes = sqlx::query("SELECT n.id,n.name,(n.deleted_at IS NOT NULL OR s.deleted_at IS NOT NULL) AS deleted,SUM(r.uplink)::text AS uplink,SUM(r.downlink)::text AS downlink FROM singbox_usage_daily r JOIN nodes n ON n.id=r.node_id JOIN servers s ON s.id=n.server_id WHERE ($1::bigint IS NULL OR r.user_id=$1) AND ($2::bigint IS NULL OR r.node_id=$2) GROUP BY n.id,s.deleted_at ORDER BY n.id")
        .bind(query.user_id).bind(query.node_id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let by_user: Vec<_> = users.iter().map(|row| json!({"user_id":row.get::<i64,_>("id"),"name":row.get::<String,_>("name"),"deleted":row.get::<bool,_>("deleted"),"uplink":row.get::<String,_>("uplink"),"downlink":row.get::<String,_>("downlink")})).collect();
    let by_node: Vec<_> = nodes.iter().map(|row| json!({"node_id":row.get::<i64,_>("id"),"name":row.get::<String,_>("name"),"deleted":row.get::<bool,_>("deleted"),"uplink":row.get::<String,_>("uplink"),"downlink":row.get::<String,_>("downlink")})).collect();
    Ok(Json(
        json!({"uplink":totals.get::<String,_>("uplink"),"downlink":totals.get::<String,_>("downlink"),"total":totals.get::<String,_>("total"),"by_user":by_user,"by_node":by_node}),
    ))
}

#[cfg(test)]
mod tests {
    use super::publishes;
    use serde_json::json;

    #[test]
    fn current_snapshots_publish_their_accounting_identities() {
        let source = json!({"schema_version":1,"public_nodes":[],"accounting_users":[
            {"user_id":7,"node_id":3,"stat_name":"u7_n3"},
            {"user_id":8,"node_id":3,"stat_name":"u8_n3","extra":true}
        ]});
        assert!(publishes(&source, 7, 3, "u7_n3"));
        assert!(publishes(&source, 8, 3, "u8_n3"));
        assert!(!publishes(&source, 7, 4, "u7_n4"));
        assert!(!publishes(&source, 7, 3, "u7_n4"));
        let newer = json!({"schema_version":2,"accounting_users":[{"user_id":7,"node_id":3,"stat_name":"u7_n3"}]});
        assert!(!publishes(&newer, 7, 3, "u7_n3"));
    }

    #[test]
    fn legacy_snapshots_publish_node_users() {
        let source =
            json!([{"id":3,"users":[{"user_id":7,"uuid":"TEST_ONLY"}]},{"id":4,"users":[]}]);
        assert!(publishes(&source, 7, 3, "u7_n3"));
        assert!(!publishes(&source, 7, 4, "u7_n4"));
        assert!(!publishes(&json!([]), 7, 3, "u7_n3"));
    }
}
