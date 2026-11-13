//! What governance observes of the panel itself for the system health view (系统自检),
//! and the reports independent observers submit. The application composes the view with
//! the work the other owners have not finished and the panel backups.

use super::require_capability;
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_panel_host::maintenance;
use apsp_protocol::now_timestamp;
use axum::{Json, extract::State, http::HeaderMap};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use std::time::Duration;

/// The panel itself as the system health view shows it: the database, the data
/// directory's disk, the background tasks' heartbeats, the latest independent observer
/// reports and the signed artifacts on record, each as the view's JSON.
pub struct PanelHealth {
    pub version: &'static str,
    pub sampled_at: i64,
    pub uptime_seconds: i64,
    pub database: Value,
    pub disk: Value,
    pub workers: Vec<Value>,
    pub independent_observers: Vec<Value>,
    pub update: Value,
}

/// Observes the panel itself. `expected` names the heartbeats the panel's background tasks
/// write, so a task that has never reported is listed as unknown.
pub async fn panel_health(state: &AppState, expected: &[&str]) -> ApiResult<PanelHealth> {
    let now = now_timestamp();
    let start = std::time::Instant::now();
    tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query("SELECT 1").execute(&state.pool),
    )
    .await
    .map_err(|_| ApiError::Busy)??;
    let database_elapsed_ms = start.elapsed().as_millis();
    let database_bytes: i64 = sqlx::query_scalar("SELECT pg_database_size(current_database())")
        .fetch_one(&state.pool)
        .await?;
    // The platform stores heartbeat details bounded; the view redacts them as the audit does.
    let mut workers: Vec<Value> = maintenance::heartbeats(&state.pool)
        .await?
        .into_iter()
        .map(|beat| {
            json!({"name":beat.name,"status":if now-beat.observed_at>120 {"stale".to_owned()} else {beat.status},"observed_at":beat.observed_at,"last_success_at":beat.last_success_at,"details":super::guard::redact(&beat.details)})
        })
        .collect();
    for name in expected.iter().copied() {
        if !workers
            .iter()
            .any(|value| value["name"].as_str() == Some(name))
        {
            workers.push(json!({"name":name,"status":"unknown","observed_at":null,"reason":"尚无此执行器周期观察"}));
        }
    }
    let reports =
        sqlx::query("SELECT * FROM system_observer_reports ORDER BY received_at DESC LIMIT 20")
            .fetch_all(&state.pool)
            .await?;
    let reports:Vec<Value>=reports.iter().map(|r|Ok(json!({"observer_name":r.try_get::<String,_>("observer_name")?,"target_origin":r.try_get::<String,_>("target_origin")?,"observed_at":r.try_get::<i64,_>("observed_at")?,"received_at":r.try_get::<i64,_>("received_at")?,"available":r.try_get::<Option<bool>,_>("available")?,"elapsed_ms":r.try_get::<Option<i64>,_>("elapsed_ms")?,"evidence":r.try_get::<Value,_>("evidence")?}))).collect::<Result<_,sqlx::Error>>()?;
    let disk = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new("df")
            .args(["-Pk", "--"])
            .arg(&state.config.data_dir)
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let disk = match disk {
        Ok(Ok(output)) if output.status.success() => {
            disk_observation(&String::from_utf8_lossy(&output.stdout), now)
        }
        _ => json!({"status":"unknown","reason":"磁盘观察工具不可用或请求超时"}),
    };
    let update = match tokio::time::timeout(Duration::from_secs(5), crate::releases::entries(state))
        .await
    {
        Ok(Ok(entries)) => {
            json!({"status":"observed","source":"local-signed-artifact-inventory","observed_at":now,"entries":entries,"remote_checked":false})
        }
        _ => {
            json!({"status":"unknown","reason":"本地签名制品清单不可用或读取超时","remote_checked":false})
        }
    };
    Ok(PanelHealth {
        version: env!("CARGO_PKG_VERSION"),
        sampled_at: now,
        uptime_seconds: now - state.started_at,
        database: json!({"available":true,"elapsed_ms":database_elapsed_ms,"bytes":database_bytes,"pool_size":state.pool.size(),"idle":state.pool.num_idle()}),
        disk,
        workers,
        independent_observers: reports,
        update,
    })
}

fn disk_observation(raw: &str, now: i64) -> Value {
    let columns: Vec<_> = raw
        .lines()
        .last()
        .unwrap_or("")
        .split_whitespace()
        .collect();
    if columns.len() < 6 {
        return json!({"status":"unknown","reason":"磁盘工具未返回可解释的容量记录"});
    }
    let parse = |index: usize| {
        columns[index]
            .parse::<u64>()
            .ok()
            .and_then(|value| value.checked_mul(1024))
    };
    match (parse(1), parse(2), parse(3)) {
        (Some(total), Some(used), Some(available)) => {
            json!({"status":"observed","source":"df -Pk","observed_at":now,"total_bytes":total,"used_bytes":used,"available_bytes":available,"raw":raw.chars().take(2048).collect::<String>()})
        }
        _ => json!({"status":"unknown","reason":"磁盘容量记录无效"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disk_evidence_keeps_invalid_and_overflowing_results_unknown() {
        let observed = disk_observation(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/data 1024 100 924 10% /panel data\n",
            7,
        );
        assert_eq!(observed["available_bytes"], 924 * 1024);
        assert_eq!(observed["observed_at"], 7);
        assert_eq!(disk_observation("unavailable", 7)["status"], "unknown");
        assert_eq!(
            disk_observation("fs 18446744073709551615 1 1 0% /", 7)["status"],
            "unknown"
        );
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserverInput {
    observer_name: String,
    target_origin: String,
    observed_at: i64,
    available: Option<bool>,
    elapsed_ms: Option<i64>,
    evidence: Value,
}
pub async fn observe(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ObserverInput>,
) -> ApiResult<Json<Value>> {
    require_capability(&state, &headers, "monitoring:write").await?;
    let now = now_timestamp();
    if input.observer_name.trim().is_empty()
        || input.observer_name.len() > 200
        || input.target_origin != state.config.public_url
        || input.observed_at > now + 60
        || input.observed_at < now - 86400
        || input.elapsed_ms.is_some_and(|v| !(0..=300000).contains(&v))
        || serde_json::to_vec(&input.evidence)
            .map_err(anyhow::Error::from)?
            .len()
            > 16384
    {
        return Err(ApiError::BadRequest(
            "观察点、面板地址或证据时间无效".into(),
        ));
    }
    let id:i64=sqlx::query_scalar("INSERT INTO system_observer_reports(observer_name,target_origin,observed_at,received_at,available,elapsed_ms,evidence) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id")
        .bind(input.observer_name).bind(input.target_origin).bind(input.observed_at).bind(now).bind(input.available).bind(input.elapsed_ms).bind(super::guard::redact(&input.evidence)).fetch_one(&state.pool).await?;
    Ok(Json(
        json!({"id":id,"received_at":now,"source":"independent-observer-submitted-evidence"}),
    ))
}
