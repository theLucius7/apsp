//! Display history from the existing account-wide CDT observations.
use super::{billing::Traffic, model::Account};
use crate::{AppState, auth, error::ApiResult};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::get,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

pub(super) fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/plugins/alicloud/accounts/{id}/traffic-history",
        get(history),
    )
}

pub(super) fn identity(account: &Account) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{}\0{}", account.site, account.access_key_id))
    )
}

pub(super) async fn record(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    traffic: &Traffic,
    identity: &str,
) -> ApiResult<()> {
    let previous: Option<String> =
        sqlx::query_scalar("SELECT traffic_history_identity FROM alicloud_accounts WHERE id=$1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    if previous.as_deref() != Some(identity) {
        sqlx::query("DELETE FROM alicloud_traffic_history WHERE account_id=$1")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE alicloud_accounts SET traffic_history_identity=$2 WHERE id=$1")
            .bind(id)
            .bind(identity)
            .execute(&mut **tx)
            .await?;
    }
    for seconds in [3600_i64, 86400] {
        // Days follow the CDT billing calendar; hours are aligned on UTC hours.
        let offset = if seconds == 86400 { 28800 } else { 0 };
        let bucket = (traffic.queried_at + offset).div_euclid(seconds) * seconds - offset;
        sqlx::query("INSERT INTO alicloud_traffic_history(account_id,bucket_seconds,bucket_start,sampled_at,mainland_bytes,overseas_bytes) VALUES($1,$2,$3,$4,$5::text::numeric,$6::text::numeric) ON CONFLICT(account_id,bucket_seconds,bucket_start) DO UPDATE SET sampled_at=EXCLUDED.sampled_at,mainland_bytes=EXCLUDED.mainland_bytes,overseas_bytes=EXCLUDED.overseas_bytes WHERE alicloud_traffic_history.sampled_at < EXCLUDED.sampled_at")
            .bind(id).bind(seconds as i32).bind(bucket).bind(traffic.queried_at).bind(&traffic.mainland_bytes).bind(&traffic.overseas_bytes).execute(&mut **tx).await?;
    }
    Ok(())
}

#[derive(Serialize, FromRow)]
struct Sample {
    bucket_start: i64,
    sampled_at: i64,
    mainland_bytes: String,
    overseas_bytes: String,
}
async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    auth::require_admin(&state, &headers).await?;
    // A repeatable snapshot cannot combine an old credential's history with a new account.
    let mut tx = super::lock(&state.pool, id).await?;
    let _account: Account = super::account_on(&mut tx, id).await?;
    let now = apsp_protocol::now_timestamp();
    let mut groups = Vec::new();
    for (seconds, count) in [(3600_i64, 25_i64), (86400, 31)] {
        let offset = if seconds == 86400 { 28800 } else { 0 };
        let end = (now + offset).div_euclid(seconds) * seconds - offset;
        let rows: Vec<Sample> = sqlx::query_as("SELECT bucket_start,sampled_at,mainland_bytes::text,overseas_bytes::text FROM alicloud_traffic_history WHERE account_id=$1 AND bucket_seconds=$2 AND bucket_start BETWEEN $3 AND $4 ORDER BY bucket_start LIMIT 32")
            .bind(id).bind(seconds as i32).bind(end - (count - 1) * seconds).bind(end).fetch_all(&mut *tx).await?;
        groups.push(serde_json::json!({"seconds":seconds,"from":end-(count-1)*seconds,"to":end,"samples":rows}));
    }
    tx.commit().await?;
    Ok(Json(
        serde_json::json!({"account_id":id,"queried_at":now,"groups":groups}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[sqlx::test(migrations = "../../../crates/panel/migrations")]
    async fn samples_replace_only_older_observations_and_keep_pools_separate(pool: sqlx::PgPool) {
        let (id, _) = super::super::tests::seed(&pool, "ecs").await;
        let mut tx = pool.begin().await.unwrap();
        let sample = |at, inland: &str| Traffic {
            queried_at: at,
            mainland_bytes: inland.into(),
            overseas_bytes: "9007199254740993".into(),
            regions: vec![],
        };
        record(&mut tx, id, &sample(1793923200, "10"), "TEST_ONLY_IDENTITY")
            .await
            .unwrap();
        record(&mut tx, id, &sample(1793923260, "20"), "TEST_ONLY_IDENTITY")
            .await
            .unwrap();
        record(&mut tx, id, &sample(1793923230, "15"), "TEST_ONLY_IDENTITY")
            .await
            .unwrap();
        let rows: Vec<(String,String)> = sqlx::query_as("SELECT mainland_bytes::text,overseas_bytes::text FROM alicloud_traffic_history WHERE account_id=$1").bind(id).fetch_all(&mut *tx).await.unwrap();
        assert_eq!(rows, vec![("20".into(), "9007199254740993".into()); 2]);
        record(
            &mut tx,
            id,
            &sample(1793926860, "1"),
            "TEST_ONLY_ROTATED_IDENTITY",
        )
        .await
        .unwrap();
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT mainland_bytes::text FROM alicloud_traffic_history WHERE account_id=$1",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        assert_eq!(rows, vec!["1"; 2]);
    }
}
