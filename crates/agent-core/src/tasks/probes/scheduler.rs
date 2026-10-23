use super::*;
use apsp_protocol::{AuthorizedProbe, ProbeExecution};

pub(super) async fn sample_loop(
    state: SharedState,
    ops: Arc<dyn Privileged>,
    mut clients: watch::Receiver<Option<Arc<PanelClient>>>,
    mut leases: watch::Receiver<Option<AcceptedLease>>,
    retirement: Arc<crate::retirement::Retirement>,
) -> Result<()> {
    let mut storage = crate::state::StorageRetry::default();
    let mut due = HashMap::<Uuid, Instant>::new();
    let mut configured = HashMap::<Uuid, AuthorizedProbe>::new();
    let mut running = HashMap::<Uuid, (AuthorizedProbe, Uuid, AcceptedLease, AbortHandle)>::new();
    let mut tasks = JoinSet::<(Uuid, AuthorizedProbe, Option<ProbeResult>)>::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let lease = leases
            .borrow()
            .clone()
            .filter(|lease| !retirement.requested() && lease.current(clients.borrow().as_ref()));
        let deadline = lease.as_ref().map(|lease| lease.deadline);
        tokio::select! {
            biased;
            changed = clients.changed() => {
                if changed.is_err() { return Ok(()); }
            }
            changed = leases.changed() => {
                if changed.is_err() { return Ok(()); }
            }
            _ = async {
                if let Some(deadline) = deadline { tokio::time::sleep_until(deadline).await; }
                else { std::future::pending::<()>().await; }
            } => {}
            completed = tasks.join_next(), if !tasks.is_empty() => {
                let Some(completed) = completed else { continue };
                match completed {
                    Ok((run_id, probe, Some(result))) => {
                        let Some((_, current_run, initial, _)) = running.get(&probe.spec.id) else { continue };
                        if *current_run != run_id { continue; }
                        let initial = initial.clone();
                        running.remove(&probe.spec.id);
                        let _guard = retirement.gate.read().await;
                        let current = leases.borrow().clone().filter(|lease| !retirement.requested()
                            && lease.current(clients.borrow().as_ref()));
                        if initial.current(clients.borrow().as_ref())
                            && current.as_ref().is_some_and(|lease| lease.snapshot.revision == initial.snapshot.revision && lease.snapshot.probes.contains(&probe)) {
                            let mut state = state.lock().map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                            storage.finish("persist probe sample", state.save_probe_result(&result))?;
                        }
                    }
                    Ok((run_id, probe, None)) => {
                        if running.get(&probe.spec.id).is_some_and(|(_, current, _, _)| *current == run_id) {
                            running.remove(&probe.spec.id);
                        }
                    }
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => return Err(error.into()),
                }
            }
            _ = tick.tick() => {}
        }
        let lease = leases
            .borrow()
            .clone()
            .filter(|lease| !retirement.requested() && lease.current(clients.borrow().as_ref()));
        let probes = lease
            .as_ref()
            .map(|lease| lease.snapshot.probes.as_slice())
            .unwrap_or_default();
        running.retain(|_, (probe, _, initial, task)| {
            let keep = initial.current(clients.borrow().as_ref())
                && lease
                    .as_ref()
                    .is_some_and(|lease| lease.snapshot.revision == initial.snapshot.revision)
                && probes.contains(probe);
            if !keep {
                task.abort();
            }
            keep
        });
        due.retain(|id, _| probes.iter().any(|probe| probe.spec.id == *id));
        let Some(lease) = lease else {
            configured.clear();
            continue;
        };
        let schedule = match leases::schedule(&state) {
            Ok(schedule) => schedule,
            Err(error) => {
                for (_, _, _, task) in running.values() {
                    task.abort();
                }
                running.clear();
                tracing::warn!(%error, "probe schedule unavailable; no measurements started");
                continue;
            }
        };
        let now = Instant::now();
        let panel_now = lease.panel_time();
        if schedule.values().any(|entry| {
            entry.last_started_at > panel_now.saturating_add(apsp_protocol::MAX_PROBE_LEASE_SECS)
        }) {
            for (_, _, _, task) in running.values() {
                task.abort();
            }
            running.clear();
            tracing::warn!("probe schedule has an unknown future clock; no measurements started");
            continue;
        }
        for probe in &lease.snapshot.probes {
            let id = probe.spec.id;
            let interval_changed = configured
                .get(&id)
                .is_some_and(|old| old.spec.interval_secs != probe.spec.interval_secs);
            if interval_changed || !due.contains_key(&id) {
                let wait = schedule
                    .get(&id)
                    .map(|previous| {
                        previous
                            .last_started_at
                            .saturating_add(i64::from(probe.spec.interval_secs))
                            .saturating_sub(panel_now)
                            .max(0)
                    })
                    .unwrap_or(0);
                due.insert(id, now + Duration::from_secs(wait as u64));
            }
        }
        configured = lease
            .snapshot
            .probes
            .iter()
            .map(|probe| (probe.spec.id, probe.clone()))
            .collect();
        let mut ready: Vec<_> = lease
            .snapshot
            .probes
            .iter()
            .filter(|probe| {
                !running.contains_key(&probe.spec.id)
                    && due.get(&probe.spec.id).is_some_and(|next| *next <= now)
            })
            .cloned()
            .collect();
        ready.sort_by_key(|probe| (due.get(&probe.spec.id).copied(), probe.spec.id));
        for probe in ready.into_iter().take(4usize.saturating_sub(tasks.len())) {
            let _guard = retirement.gate.read().await;
            if retirement.requested() || !lease.current(clients.borrow().as_ref()) {
                break;
            }
            if let Err(error) = leases::record_start(&state, &lease, &probe.spec) {
                tracing::warn!(%error, "probe schedule was not persisted; measurement was not started");
                break;
            }
            due.insert(
                probe.spec.id,
                now + Duration::from_secs(u64::from(probe.spec.interval_secs)),
            );
            let ops = ops.clone();
            let retirement = retirement.clone();
            let saved = probe.clone();
            let execution = ProbeExecution {
                lease_id: lease.snapshot.id,
                revision: lease.snapshot.revision,
                issued_at: lease.snapshot.issued_at,
                expires_at: lease.snapshot.expires_at,
                probe: probe.clone(),
            };
            let mut authority = leases.clone();
            let mut connection = clients.clone();
            let initial = lease.clone();
            let running_lease = initial.clone();
            let run_id = Uuid::new_v4();
            let task = tasks.spawn(async move {
                let _guard = retirement.gate.read().await;
                if retirement.requested() || !initial.current(connection.borrow().as_ref()) { return (run_id, probe, None); }
                let measured_spec = probe.spec.clone();
                let measure = sample(&measured_spec, ops.as_ref(), initial.panel_millis().saturating_sub(now_millis()));
                tokio::pin!(measure);
                let result = loop {
                    let current = authority.borrow().clone().filter(|lease| !retirement.requested()
                        && initial.current(connection.borrow().as_ref())
                        && lease.current(connection.borrow().as_ref()) && lease.snapshot.revision == initial.snapshot.revision && lease.snapshot.probes.contains(&probe));
                    let Some(current) = current else { break None };
                    tokio::select! {
                        biased;
                        changed = connection.changed() => { if changed.is_err() { break None; } }
                        changed = authority.changed() => { if changed.is_err() { break None; } }
                        _ = tokio::time::sleep_until(current.deadline.min(initial.deadline)) => { break None; }
                        result = &mut measure => {
                            let current = authority.borrow().clone();
                            let allowed = !retirement.requested() && initial.current(connection.borrow().as_ref())
                                && current.as_ref().is_some_and(|lease| lease.current(connection.borrow().as_ref())
                                    && lease.snapshot.revision == initial.snapshot.revision && lease.snapshot.probes.contains(&probe));
                            break allowed.then_some(result);
                        }
                    }
                };
                let result = result.map(|mut result| {
                    result.sampled_at = initial.panel_millis();
                    result.execution = Some(execution);
                    result
                });
                (run_id, probe, result)
            });
            running.insert(saved.spec.id, (saved, run_id, running_lease, task));
        }
    }
}
