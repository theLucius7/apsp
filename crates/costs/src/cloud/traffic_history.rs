//! Display history from the existing account-wide CDT observations.
use crate::cloud::accounts::Traffic;
use crate::{AppState, auth, error::ApiResult};
use apsp_resources::cloud::{client::Keys, model::Account};
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

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/resources/alicloud/accounts/{id}/traffic-history",
        get(history),
    )
}

/// Which CDT pool the readings come from: the account's site and access key.
pub fn identity(account: &Account, keys: &Keys) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{}\0{}", account.site, keys.access_key_id))
    )
}

pub async fn record(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    traffic: &Traffic,
    identity: &str,
) -> ApiResult<()> {
    let previous: Option<String> = sqlx::query_scalar(
        "SELECT traffic_history_identity FROM cloud_account_billing WHERE account_id=$1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    if previous.as_deref() != Some(identity) {
        sqlx::query("DELETE FROM alicloud_traffic_history WHERE account_id=$1")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO cloud_account_billing(account_id,traffic_history_identity) VALUES($1,$2) ON CONFLICT(account_id) DO UPDATE SET traffic_history_identity=EXCLUDED.traffic_history_identity")
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
    let mut tx = apsp_resources::cloud::lock(&state.pool, id).await?;
    let _account: Account = apsp_resources::cloud::account_on(&mut tx, id).await?;
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

/// Stores the CDT traffic an account refresh read, adding it to the history, or the
/// reason it could not be read.
pub async fn store_traffic(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    traffic: Result<(Traffic, String), apsp_cloud_api::Failure>,
) -> ApiResult<()> {
    match traffic {
        Ok((traffic, identity)) => {
            record(tx, id, &traffic, &identity).await?;
            sqlx::query("INSERT INTO cloud_account_billing(account_id,traffic) VALUES($1,$2) ON CONFLICT(account_id) DO UPDATE SET traffic=EXCLUDED.traffic,traffic_error=NULL")
                .bind(id)
                .bind(sqlx::types::Json(traffic))
                .execute(&mut **tx)
                .await?;
        }
        Err(error) => {
            sqlx::query("INSERT INTO cloud_account_billing(account_id,traffic_error) VALUES($1,$2) ON CONFLICT(account_id) DO UPDATE SET traffic_error=EXCLUDED.traffic_error")
                .bind(id)
                .bind(error.code)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

/// Drops history buckets that started before `before`.
pub async fn prune(pool: &sqlx::PgPool, before: i64) -> ApiResult<()> {
    sqlx::query("DELETE FROM alicloud_traffic_history WHERE bucket_start<$1")
        .bind(before)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[sqlx::test(migrations = "../panel/migrations")]
    async fn samples_replace_only_older_observations_and_keep_pools_separate(pool: sqlx::PgPool) {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO alicloud_accounts(id,name) VALUES($1,'测试账号')")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
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
