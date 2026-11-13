use super::*;
use apsp_protocol::telemetry::TelemetrySettings;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const MAX_LIVE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct LiveSample {
    pub sample: TelemetrySample,
    pub received_at: i64,
}

struct Entry {
    value: LiveSample,
    /// The sample's metrics as JSON, built once for the dashboard's polls.
    metrics: Arc<Value>,
    bytes: usize,
}

#[derive(Default)]
pub struct LiveStore {
    entries: Mutex<HashMap<i64, Entry>>,
}

impl LiveStore {
    pub fn get(&self, server: i64) -> Option<LiveSample> {
        self.entries
            .lock()
            .ok()?
            .get(&server)
            .filter(|entry| now_millis() - entry.value.received_at <= 600_000)
            .map(|entry| entry.value.clone())
    }
    /// Time sampled, time received and metrics of a server's live sample.
    fn metrics(&self, server: i64) -> Option<(i64, i64, Arc<Value>)> {
        self.entries
            .lock()
            .ok()?
            .get(&server)
            .filter(|entry| now_millis() - entry.value.received_at <= 600_000)
            .map(|entry| {
                (
                    entry.value.sample.sampled_at,
                    entry.value.received_at,
                    entry.metrics.clone(),
                )
            })
    }
    fn publish(&self, server: i64, sample: TelemetrySample, now: i64) -> ApiResult<()> {
        let bytes = serde_json::to_vec(&sample)
            .map_err(anyhow::Error::from)?
            .len();
        if bytes > 128 * 1024 {
            return Err(ApiError::BadRequest("实时样本超过大小上限".into()));
        }
        let metrics = Arc::new(serde_json::to_value(&sample.metrics).map_err(anyhow::Error::from)?);
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("live cache lock poisoned"))?;
        if entries
            .get(&server)
            .is_some_and(|entry| entry.value.sample.sampled_at >= sample.sampled_at)
        {
            return Ok(());
        }
        entries.retain(|_, entry| now - entry.value.received_at <= 600_000);
        entries.insert(
            server,
            Entry {
                value: LiveSample {
                    sample,
                    received_at: now,
                },
                metrics,
                bytes,
            },
        );
        let mut total: usize = entries.values().map(|entry| entry.bytes).sum();
        while total > MAX_LIVE_BYTES {
            let oldest = entries
                .iter()
                .min_by_key(|(_, entry)| entry.value.received_at)
                .map(|(id, _)| *id);
            let Some(oldest) = oldest else { break };
            if let Some(entry) = entries.remove(&oldest) {
                total -= entry.bytes;
            }
        }
        Ok(())
    }
}

pub fn latest_live(state: &AppState, server: i64) -> Option<LiveSample> {
    store(state).get(server)
}

pub async fn live(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(sample): Json<TelemetrySample>,
) -> ApiResult<Json<Value>> {
    let server = crate::device_channel::require_agent(&state, &headers).await?;
    let now = now_millis();
    if !valid_sample(&sample, now) {
        return Err(ApiError::BadRequest("实时样本格式或时间无效".into()));
    }
    let sampled_at = sample.sampled_at;
    store(&state).publish(server, sample, now)?;
    // Receipt is explicitly not a durable ACK; the Agent keeps its outbox.
    Ok(Json(json!({"accepted":true,"sampled_at":sampled_at})))
}

/// The live samples of this process, shared by every copy of the application state.
fn store(state: &AppState) -> Arc<LiveStore> {
    state.domain_states.get_or_init(LiveStore::default)
}

pub async fn attach_live(
    state: &AppState,
    servers: &mut [crate::servers::Server],
) -> ApiResult<()> {
    let ids: Vec<i64> = servers.iter().map(|server| server.id).collect();
    let rows = sqlx::query("SELECT id,telemetry_settings FROM servers WHERE id=ANY($1)")
        .bind(&ids)
        .fetch_all(&state.pool)
        .await?;
    let settings: HashMap<i64, TelemetrySettings> = rows
        .into_iter()
        .map(|row| {
            Ok((
                row.get("id"),
                serde_json::from_value(row.get("telemetry_settings"))?,
            ))
        })
        .collect::<Result<_, serde_json::Error>>()
        .map_err(anyhow::Error::from)?;
    let now = now_millis();
    for server in servers {
        server.metrics_persisted_at = server.metrics_sampled_at;
        server.served_at = now;
        server.telemetry_settings = settings.get(&server.id).cloned();
        if let Some(value) = latest_live(state, server.id)
            .filter(|value| Some(value.sample.sampled_at) >= server.metrics_sampled_at)
        {
            server.metrics_sampled_at = Some(value.sample.sampled_at);
            server.latest_metrics =
                serde_json::to_value(value.sample.metrics).map_err(anyhow::Error::from)?;
            server.metrics_received_at = Some(value.received_at);
        }
        server.metrics_stale = is_stale(server.metrics_sampled_at, &server.agent_settings, now);
        without_process_detail(&mut server.latest_metrics);
    }
    Ok(())
}

/// Per-process and pressure detail is tens of kilobytes per server, and only the
/// fleet profile (which reads the stored row) shows it. Every polled server read
/// leaves it out: with it, the list was over half a megabyte every few seconds.
pub(crate) fn without_process_detail(metrics: &mut Value) {
    if let Some(object) = metrics.as_object_mut() {
        object.remove("process_resources");
        object.remove("system_pressure");
    }
}

fn is_stale(sampled: Option<i64>, settings: &Value, now: i64) -> bool {
    let settings: AgentSettings = serde_json::from_value(settings.clone()).unwrap_or_default();
    let allowance = (settings.sample_interval_secs * 3 + settings.upload_interval_secs * 2).max(15)
        as i64
        * 1000;
    sampled.is_none_or(|at| now.saturating_sub(at) > allowance)
}

pub(crate) async fn dashboard_live(state: &AppState, admin: bool) -> ApiResult<Value> {
    let rows=sqlx::query(&format!("SELECT id,last_seen,last_heartbeat_at,NULLIF(metrics_sampled_at,0) AS metrics_sampled_at,agent_settings,asset_settings FROM servers WHERE deleted_at IS NULL AND NOT {} ORDER BY id", crate::server_assets::sql::hidden("servers"))).fetch_all(&state.pool).await?;
    // A live sample at least as new as the stored one replaces it, so stored
    // metrics are read only for servers without one.
    let live_store = store(state);
    let live: HashMap<i64, (i64, i64, Arc<Value>)> = rows
        .iter()
        .filter_map(|row| {
            let id: i64 = row.get("id");
            let persisted: Option<i64> = row.get("metrics_sampled_at");
            live_store
                .metrics(id)
                .filter(|(sampled, _, _)| Some(*sampled) >= persisted)
                .map(|value| (id, value))
        })
        .collect();
    let missing: Vec<i64> = rows
        .iter()
        .map(|row| row.get::<i64, _>("id"))
        .filter(|id| !live.contains_key(id))
        .collect();
    let mut stored: HashMap<i64, Value> = if missing.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as("SELECT id,latest_metrics FROM servers WHERE id=ANY($1)")
            .bind(&missing)
            .fetch_all(&state.pool)
            .await?
            .into_iter()
            .collect()
    };
    let now = now_millis();
    let mut servers = Vec::with_capacity(rows.len());
    for row in rows {
        let id: i64 = row.get("id");
        let persisted: Option<i64> = row.get("metrics_sampled_at");
        let (sampled, received, metrics) = match live.get(&id) {
            Some((sampled, received, metrics)) => {
                let mut metrics = Value::clone(metrics);
                without_process_detail(&mut metrics);
                (Some(*sampled), Some(*received), metrics)
            }
            None => {
                let mut metrics = stored.remove(&id).unwrap_or(Value::Null);
                without_process_detail(&mut metrics);
                (persisted, None, metrics)
            }
        };
        let last_seen: Option<i64> = row.get("last_seen");
        servers.push(json!({"id":id,"last_seen":last_seen,"last_heartbeat_at":row.get::<Option<i64>,_>("last_heartbeat_at"),"online":last_seen.is_some_and(|seen|now/1000-seen<=60),"metrics_sampled_at":sampled,"metrics_received_at":received,"metrics_persisted_at":persisted,"metrics_stale":is_stale(sampled,&row.get("agent_settings"),now),"latest_metrics":if admin {metrics} else {crate::dashboard::public_metrics_for_asset(&metrics,&row.get("asset_settings"))}}));
    }
    Ok(json!({"served_at":now,"public_view":!admin,"servers":servers}))
}
