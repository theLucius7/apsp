use super::cache::{Control, Snapshot};
use crate::{Config, SharedState, state::StorageRetry};
use anyhow::{Result, ensure};
use apsp_protocol::{AgentSettings, telemetry::TelemetrySettings};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[cfg(test)]
mod tests;

pub(crate) fn initial_control(config: &Config, state: &SharedState) -> Result<Control> {
    let state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    let settings = state
        .get_json::<AgentSettings>("agent_settings")?
        .unwrap_or_else(|| config.settings.clone());
    ensure!(settings.valid(), "invalid telemetry settings");
    Ok(Control {
        interval: Duration::from_secs(settings.sample_interval_secs),
        offset_ms: state.get_json::<i64>("clock_offset_ms")?.unwrap_or(0),
        minimum_timestamp: state.latest_telemetry_timestamp()?,
        enabled: true,
    })
}

pub(crate) async fn run(
    config: Config,
    state: SharedState,
    mut snapshots: watch::Receiver<Arc<Snapshot>>,
    control: watch::Sender<Control>,
    mut client: watch::Receiver<Option<Arc<crate::artifacts::PanelClient>>>,
    retirement: Arc<crate::retirement::Retirement>,
) -> Result<()> {
    // Persistence and upload never own or call the collector. Even a kernel call
    // that never returns leaves both async futures and the connection responsive.
    let live_snapshots = snapshots.clone();
    let (delivery_tx, delivery_rx) = watch::channel(None::<TelemetrySettings>);
    let persist = async {
        let mut saved = None;
        let mut stale = false;
        let mut storage = StorageRetry::default();
        let mut poll = tokio::time::interval(Duration::from_secs(1));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            {
                let _guard = retirement.gate.read().await;
                let mut next_control = initial_control(&config, &state)?;
                next_control.enabled = !retirement.requested();
                control.send_replace(next_control);
                let snapshot = snapshots.borrow_and_update().clone();
                let expired = snapshot.timed_out() || snapshot.error.is_some();
                if expired && !stale {
                    tracing::warn!(error = ?snapshot.error, "telemetry collector stalled; last sample retained without refreshing its time");
                }
                stale = expired;
                if let Some(sample) = snapshot
                    .sample
                    .as_ref()
                    .filter(|sample| !retirement.requested() && Some(sample.id) != saved)
                {
                    let persisted = {
                        let mut state = state
                            .lock()
                            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                        storage.finish("persist telemetry", state.save_telemetry(sample))?
                    };
                    if let Some(dropped) = persisted {
                        saved = Some(sample.id);
                        if dropped > 0 {
                            tracing::warn!(
                                dropped,
                                "oldest telemetry samples removed by offline retention limit"
                            );
                        }
                    }
                }
            }
            tokio::select! {
                _ = poll.tick() => {},
                changed = snapshots.changed() => { changed?; },
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let durable_client = client.clone();
    let upload = async {
        let mut last_settings: Option<Instant> = None;
        let mut live_sent = None;
        let mut settings_storage = StorageRetry::default();
        loop {
            let interval = {
                let _guard = retirement.gate.read().await;
                let active = client.borrow_and_update().clone();
                if !retirement.requested()
                    && let Some(active) = active
                {
                    if last_settings.is_none_or(|v| v.elapsed() >= Duration::from_secs(60)) {
                        last_settings = Some(Instant::now());
                        match active.agent_settings().await {
                            Ok(settings) => {
                                let mut state = state
                                    .lock()
                                    .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                                if settings_storage
                                    .finish(
                                        "persist Agent settings",
                                        state.set_json("agent_settings", &settings),
                                    )?
                                    .is_none()
                                {
                                    // Retain the durable settings and retry on the next live cycle.
                                    last_settings = None;
                                }
                            }
                            Err(error) => tracing::warn!(%error, "Agent settings refresh failed"),
                        }
                        let settings = active
                            .get_json::<TelemetrySettings>("/api/agent/v1/telemetry-settings")
                            .await;
                        // Older panels do not expose this endpoint. Keep their
                        // durable upload schedule and strict settings decoder.
                        delivery_tx.send_replace(settings.ok().filter(TelemetrySettings::valid));
                    }
                    if delivery_tx.borrow().is_some() {
                        let sample = live_snapshots.borrow().sample.clone();
                        if let Some(sample) = sample.filter(|sample| Some(sample.id) != live_sent) {
                            match active
                                .post_json::<serde_json::Value>(
                                    "/api/agent/v1/telemetry/live",
                                    &sample,
                                )
                                .await
                            {
                                Ok(_) => live_sent = Some(sample.id),
                                Err(error) => {
                                    tracing::debug!(%error, "live telemetry will retry; durable sample retained")
                                }
                            }
                        }
                    }
                }
                state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
                    .get_json::<AgentSettings>("agent_settings")?
                    .unwrap_or_else(|| config.settings.clone())
                    .upload_interval_secs
            };
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(interval)) => {},
                changed = client.changed() => { changed?; last_settings = None; live_sent = None; },
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let durable = deliver(&config, &state, delivery_rx, durable_client, &retirement);
    tokio::try_join!(persist, upload, durable)?;
    Ok(())
}

async fn deliver(
    config: &Config,
    state: &SharedState,
    mut settings: watch::Receiver<Option<TelemetrySettings>>,
    mut client: watch::Receiver<Option<Arc<crate::artifacts::PanelClient>>>,
    retirement: &crate::retirement::Retirement,
) -> Result<()> {
    let mut saved: Option<Instant> = None;
    let mut acknowledgment_storage = StorageRetry::default();
    loop {
        let upload_interval = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
            .get_json::<AgentSettings>("agent_settings")?
            .unwrap_or_else(|| config.settings.clone())
            .upload_interval_secs;
        let interval = settings
            .borrow_and_update()
            .as_ref()
            .map_or(upload_interval, |s| s.persist_interval_secs);
        let due = saved.is_none_or(|at| at.elapsed() >= Duration::from_secs(interval));
        let active = client.borrow_and_update().clone();
        if due && let Some(active) = active {
            // Bound each catch-up pass; release the read gate between requests
            // so a waiting retirement writer can stop the next batch.
            for _ in 0..16 {
                {
                    let _guard = retirement.gate.read().await;
                    if retirement.requested() {
                        break;
                    }
                    let samples = state
                        .lock()
                        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
                        .pending_telemetry()?;
                    if samples.is_empty() {
                        saved = Some(Instant::now());
                        break;
                    }
                    match active
                        .telemetry(&apsp_protocol::TelemetryBatch { samples })
                        .await
                    {
                        Ok(ack) => {
                            let acknowledged = {
                                let mut state = state
                                    .lock()
                                    .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                                acknowledgment_storage.finish(
                                    "persist telemetry acknowledgment",
                                    state.acknowledge_telemetry(&ack),
                                )?
                            };
                            if acknowledged.is_none() {
                                // Retain samples whose ACK could not commit until the next delivery cycle,
                                // rather than resending up to sixteen batches against a busy store.
                                break;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error,"telemetry upload failed; samples retained for retry");
                            break;
                        }
                    }
                }
                tokio::task::yield_now().await;
            }
        }
        tokio::select! {
            _=tokio::time::sleep(Duration::from_secs(upload_interval.min(3)))=>{},
            changed=settings.changed()=>{changed?;},
            changed=client.changed()=>{changed?; saved=None;},
        }
    }
}
