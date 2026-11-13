//! Server health, the change and event timeline, and comparisons between two servers
//! (observability). Device operation receipts add service events here.

mod service_events;
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
pub use service_events::observe as observe_service_result;
use sqlx::Row;

const COMPARISON_METRICS: [&str; 7] = [
    "cpu_percent",
    "memory_used",
    "disk_used",
    "load_1",
    "swap_used",
    "tcp_connections",
    "udp_connections",
];

/// Keep the last observation used by comparisons without duplicating inventories,
/// per-process details or network counters already held by telemetry storage.
pub(crate) fn comparison_metrics(metrics: &Value) -> Value {
    Value::Object(
        COMPARISON_METRICS
            .into_iter()
            .filter_map(|key| {
                let value = metrics.get(key)?;
                value
                    .as_f64()
                    .is_some_and(|number| number.is_finite() && number >= 0.0)
                    .then(|| (key.to_owned(), value.clone()))
            })
            .collect(),
    )
}

pub async fn health(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    crate::control_center::require_capability(&state, &headers, "monitoring:read").await?;
    let database:Value=sqlx::query_scalar("SELECT jsonb_build_object('size_bytes',pg_database_size(current_database()),'connections',(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()),'checked_at',$1::bigint)").bind(now_timestamp()).fetch_one(&state.pool).await?;
    let pending:Value=sqlx::query_scalar("SELECT jsonb_build_object('commands',(SELECT count(*) FROM remote_commands WHERE state IN ('queued','claimed','running','cancel_requested')),'diagnostics',(SELECT count(*) FROM diagnostic_jobs WHERE status IN ('queued','running','cancel_requested')),'fleet',(SELECT count(*) FROM fleet_operations WHERE status IN ('queued','dispatched','unknown') AND reconciled_at IS NULL),'notifications',(SELECT count(*) FROM notification_outbox WHERE status='pending'),'notification_failed',(SELECT count(*) FROM notification_outbox WHERE status='failed'),'oldest_fleet_request',(SELECT min(requested_at) FROM fleet_operations WHERE status='queued'))").fetch_one(&state.pool).await?;
    Ok(Json(
        json!({"database":database,"queues":pending,"started_at":state.started_at,"checked_at":now_timestamp(),"connected_agents":apsp_resources::device_channel::devices(&state).connections.read().await.len(),"system_health":{"endpoint":"/api/control-center/health","requires":"owner","scope":["host_disk_bytes","actual_worker_heartbeats","backup_and_drills","signed_update_inventory","independent_observers"]},"independent_observer":{"endpoint":"/healthz","authenticated_endpoint":"/api/fleet/health"}}),
    ))
}

pub async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<Value>>> {
    crate::control_center::require_server(&state, &headers, id, "monitoring:read").await?;
    let events=sqlx::query_scalar("SELECT item FROM (SELECT to_jsonb(e)-'server_id' AS item,occurred_at FROM fleet_events e WHERE server_id=$1 UNION ALL SELECT jsonb_build_object('id',id,'kind',category,'source','panel_monitoring','occurred_at',opened_at,'detail',details,'resolved_at',resolved_at,'resolution',resolution),opened_at FROM server_alert_events WHERE server_id=$1) events ORDER BY occurred_at DESC LIMIT 500").bind(id).fetch_all(&state.pool).await?;
    Ok(Json(events))
}

#[derive(Deserialize)]
pub struct CompareInput {
    server_id: Option<i64>,
    from: i64,
    until: i64,
    other_from: Option<i64>,
    other_until: Option<i64>,
    metric: String,
}
pub async fn compare(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(input): Query<CompareInput>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_server(&state, &headers, id, "monitoring:read").await?;
    let other = input.server_id.unwrap_or(id);
    crate::control_center::require_server(&state, &headers, other, "monitoring:read").await?;
    let other_from = input.other_from.unwrap_or(input.from - 86400);
    let other_until = input.other_until.unwrap_or(input.until - 86400);
    if !COMPARISON_METRICS.contains(&input.metric.as_str())
        || input.until <= input.from
        || other_until <= other_from
        || input.until - input.from > 7 * 86400
        || other_until - other_from > 7 * 86400
    {
        return Err(ApiError::BadRequest(
            "指标或比较时间范围无效，每段最多七天".into(),
        ));
    }
    let left = window(&state, id, input.from, input.until, &input.metric).await?;
    let right = window(&state, other, other_from, other_until, &input.metric).await?;
    Ok(Json(
        json!({"metric":input.metric,"left":{"server_id":id,"from":input.from,"until":input.until,"samples":left},"right":{"server_id":other,"from":other_from,"until":other_until,"samples":right},"missing_is_zero":false,"resolution_seconds":60}),
    ))
}
async fn window(
    state: &AppState,
    id: i64,
    from: i64,
    until: i64,
    metric: &str,
) -> ApiResult<Vec<Value>> {
    let rows=sqlx::query("SELECT bucket,metrics->$4 AS value FROM metrics_minutely WHERE server_id=$1 AND bucket>=$2 AND bucket<$3 ORDER BY bucket LIMIT 10080")
        .bind(id).bind(from).bind(until).bind(metric).fetch_all(&state.pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| json!({"at":r.get::<i64,_>("bucket"),"value":r.get::<Option<Value>,_>("value")}))
        .collect())
}

/// Device events implied by a sample that is newer than the server's previous
/// latest metrics: a new boot, OOM kills, a mount turning read-only, or a
/// service whose processes disappeared. Telemetry ingest chains the previous
/// value through a batch and records the events in the same transaction.
pub(crate) fn changes(
    previous: &Value,
    sample: &apsp_protocol::TelemetrySample,
) -> Vec<(&'static str, Value)> {
    let current = sample
        .metrics
        .extra
        .get("system_pressure")
        .unwrap_or(&Value::Null);
    let previous_pressure = &previous["system_pressure"];
    let mut events = Vec::new();
    if previous_pressure["boot_id"]
        .as_str()
        .zip(current["boot_id"].as_str())
        .is_some_and(|(old, new)| old != new)
    {
        events.push((
            "reboot",
            json!({"previous_boot_id":previous_pressure["boot_id"],"boot_id":current["boot_id"]}),
        ));
    }
    if let (Some(old), Some(new)) = (
        previous_pressure["oom_kill_count"].as_u64(),
        current["oom_kill_count"].as_u64(),
    ) && new > old
    {
        events.push((
            "oom",
            json!({"increase":new-old,"counter":new,"root_cause":"unknown"}),
        ));
    }
    if let Some(mounts) = sample
        .metrics
        .extra
        .get("disk_mount_read_only")
        .and_then(Value::as_object)
    {
        for (mount, read_only) in mounts {
            if read_only.as_bool() == Some(true)
                && previous["disk_mount_read_only"][mount].as_bool() == Some(false)
            {
                events.push((
                    "disk_read_only",
                    json!({"mount_point":mount,"source":"proc_mounts","root_cause":"unknown"}),
                ));
            }
        }
    }
    if let Some(previous_services) = previous["process_resources"]["services"].as_array()
        && sample
            .metrics
            .extra
            .get("process_resources")
            .is_some_and(|value| value["services_truncated"] != true)
        && let Some(current_services) = sample
            .metrics
            .extra
            .get("process_resources")
            .and_then(|value| value["services"].as_array())
    {
        for service in previous_services {
            if let Some(name) = service["name"].as_str()
                && name.ends_with(".service")
                && !current_services
                    .iter()
                    .any(|service| service["name"].as_str() == Some(name))
            {
                events.push(("service_processes_disappeared",json!({"service":name,"source":"process_cgroup_snapshot","service_exit_confirmed":false,"root_cause":"unknown"})));
            }
        }
    }
    events
}

pub(crate) async fn record(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    server: i64,
    sampled_at: i64,
    events: Vec<(&'static str, Value)>,
) -> ApiResult<()> {
    for (kind, detail) in events {
        sqlx::query("INSERT INTO fleet_events(id,server_id,kind,source,occurred_at,detail) VALUES($1,$2,$3,'agent_procfs',$4,$5)").bind(uuid::Uuid::new_v4()).bind(server).bind(kind).bind(sampled_at/1000).bind(detail).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Adds one panel event to a server's timeline.
pub async fn record_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    server: i64,
    kind: &str,
    detail: Value,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO fleet_events(id,server_id,kind,source,occurred_at,detail) VALUES($1,$2,$3,'panel',$4,$5)")
        .bind(uuid::Uuid::new_v4()).bind(server).bind(kind).bind(apsp_protocol::now_timestamp()).bind(detail).execute(&mut **tx).await?;
    Ok(())
}
