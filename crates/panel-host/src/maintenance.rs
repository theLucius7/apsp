//! The panel's background work: supervision of its long-running tasks, the periodic
//! executor every background loop runs on, and the heartbeat each loop records for the
//! system health view.
//!
//! A loop states only what one pass does; [`periodic`] paces the passes, logs a failed
//! pass and records the loop's heartbeat. Heartbeat names are persisted identifiers that
//! the loops pass in, so the platform names no domain; rows stay continuous only while a
//! name stays the same.

use crate::ports::BoxFuture;
use apsp_protocol::now_timestamp;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{fmt::Display, future::Future, time::Duration};
use tokio::{
    sync::Notify,
    time::{Interval, MissedTickBehavior},
};

const RESTART_DELAY: Duration = Duration::from_secs(5);

/// Aborts the supervised attempt when its supervisor is cancelled at shutdown.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Keeps a long-running background loop alive for the lifetime of the panel.
///
/// Each attempt runs in its own task so a panic is observed here instead of
/// silently ending the work while HTTP requests continue to be served. The
/// loops keep their durable state in PostgreSQL, so a restart resumes them.
pub async fn supervise<F, Fut>(name: &'static str, start: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    supervise_with(name, RESTART_DELAY, start).await
}

async fn supervise_with<F, Fut>(name: &'static str, delay: Duration, mut start: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    loop {
        let attempt = tokio::spawn(start());
        let _abort = AbortOnDrop(attempt.abort_handle());
        match attempt.await {
            Ok(()) => tracing::error!(task = name, "background task stopped; restarting"),
            Err(error) if error.is_panic() => {
                tracing::error!(task = name, "background task panicked; restarting")
            }
            Err(_) => return,
        }
        tokio::time::sleep(delay).await;
    }
}

/// When a periodic task runs its passes, and how often it records its heartbeat.
#[derive(Clone, Copy, Debug)]
pub struct Cadence {
    pace: Pace,
    report_every: Option<Duration>,
}

#[derive(Clone, Copy, Debug)]
enum Pace {
    Every {
        period: Duration,
        missed: MissedTickBehavior,
    },
    After {
        pause: Duration,
        wake: &'static Notify,
    },
}

impl Cadence {
    /// A pass every `period`, the first at once. A pass that overruns its period starts
    /// the next one late and skips the ticks it missed.
    pub const fn every(period: Duration) -> Self {
        Self::paced(Pace::Every {
            period,
            missed: MissedTickBehavior::Skip,
        })
    }

    /// A pass every `period`, the first at once. The ticks missed while a pass overran
    /// are run back to back until the schedule has caught up.
    pub const fn catching_up(period: Duration) -> Self {
        Self::paced(Pace::Every {
            period,
            missed: MissedTickBehavior::Burst,
        })
    }

    /// A pass at once, then each one `pause` after the previous pass ended, or as soon
    /// as `wake` is notified.
    pub const fn after(pause: Duration, wake: &'static Notify) -> Self {
        Self::paced(Pace::After { pause, wake })
    }

    /// Records the heartbeat only when the health of a pass differs from the last one
    /// recorded or `period` has passed since that record, instead of after every pass.
    pub const fn reporting_every(self, period: Duration) -> Self {
        Self {
            report_every: Some(period),
            ..self
        }
    }

    const fn paced(pace: Pace) -> Self {
        Self {
            pace,
            report_every: None,
        }
    }
}

/// What one pass reports: the details recorded with a healthy heartbeat, or a failure.
pub type Pass = Result<Value, Failure>;

/// A failed pass: the details recorded with its heartbeat and, unless the pass logged
/// its errors itself, the error the executor logs.
#[derive(Debug)]
pub struct Failure {
    details: Value,
    error: Option<String>,
}

impl Failure {
    /// A failed pass whose error the executor logs under the task's name.
    pub fn new(error: impl Display, details: Value) -> Self {
        Self {
            details,
            error: Some(error.to_string()),
        }
    }

    /// A failed pass whose steps logged their own errors.
    pub fn logged(details: Value) -> Self {
        Self {
            details,
            error: None,
        }
    }
}

/// The pass of a single step whose heartbeat details are the same whatever its result.
pub fn outcome<T, E: Display>(result: Result<T, E>, details: Value) -> Pass {
    match result {
        Ok(_) => Ok(details),
        Err(error) => Err(Failure::new(error, details)),
    }
}

/// Runs one background loop until its supervisor stops it: waits for each pass as
/// `cadence` says, runs `pass` on the loop's `context`, logs the error of a failed pass
/// and records the heartbeat `name` with the health and details the pass reported.
/// Whatever outlives one pass, such as the next run of a slower step or work the pass
/// started, stays in `context`.
pub async fn periodic<S, F>(
    name: &'static str,
    pool: PgPool,
    cadence: Cadence,
    mut context: S,
    mut pass: F,
) where
    S: Send,
    F: for<'a> FnMut(&'a mut S) -> BoxFuture<'a, Pass> + Send,
{
    let mut pacer = Pacer::new(cadence.pace);
    let mut reports = Reports::new(cadence.report_every);
    loop {
        pacer.next().await;
        let (healthy, details) = match pass(&mut context).await {
            Ok(details) => (true, details),
            Err(Failure { details, error }) => {
                if let Some(error) = error {
                    tracing::warn!(task = name, %error, "background pass failed");
                }
                (false, details)
            }
        };
        if reports.due(healthy, now_timestamp()) {
            record(name, &pool, healthy, &details).await;
        }
    }
}

/// Runs `work`, part of a pass that can outlast the health view's freshness window,
/// recording a healthy heartbeat `name` with `details` at once and then every `period`
/// until the work ends.
pub async fn beat_while<T>(
    name: &str,
    pool: &PgPool,
    period: Duration,
    details: &Value,
    work: impl Future<Output = T>,
) -> T {
    tokio::pin!(work);
    let mut beat = tokio::time::interval(period);
    loop {
        tokio::select! {
            value = &mut work => return value,
            _ = beat.tick() => record(name, pool, true, details).await,
        }
    }
}

/// The latest heartbeat of one background task.
#[derive(Debug, sqlx::FromRow)]
pub struct Heartbeat {
    pub name: String,
    pub status: String,
    pub observed_at: i64,
    pub last_success_at: Option<i64>,
    pub details: Value,
}

/// The latest heartbeat of every task that has recorded one, ordered by name.
pub async fn heartbeats(pool: &PgPool) -> Result<Vec<Heartbeat>, sqlx::Error> {
    sqlx::query_as("SELECT name,status,observed_at,last_success_at,details FROM system_worker_heartbeats ORDER BY name")
        .fetch_all(pool)
        .await
}

async fn record(name: &str, pool: &PgPool, healthy: bool, details: &Value) {
    if let Err(error) = store(name, pool, healthy, details).await {
        tracing::warn!(task = name, %error, "background task heartbeat storage failed");
    }
}

/// Stores the heartbeat; the time of the last success moves only with a healthy one.
async fn store(
    name: &str,
    pool: &PgPool,
    healthy: bool,
    details: &Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO system_worker_heartbeats(name,status,observed_at,last_success_at,details) VALUES($1,$2,$3,CASE WHEN $2='healthy' THEN $3 END,$4) ON CONFLICT(name) DO UPDATE SET status=EXCLUDED.status,observed_at=EXCLUDED.observed_at,last_success_at=COALESCE(EXCLUDED.last_success_at,system_worker_heartbeats.last_success_at),details=EXCLUDED.details")
        .bind(name)
        .bind(if healthy { "healthy" } else { "failed" })
        .bind(now_timestamp())
        .bind(bounded(details, 0))
        .execute(pool)
        .await?;
    Ok(())
}

/// Bounds stored details with the limits of the request audit: strings to 512
/// characters, objects to 64 entries, arrays to 32 items and nesting to eight levels.
fn bounded(value: &Value, depth: usize) -> Value {
    if depth > 8 {
        return json!("[已截断]");
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .take(64)
                .map(|(key, value)| (key.clone(), bounded(value, depth + 1)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .take(32)
                .map(|value| bounded(value, depth + 1))
                .collect(),
        ),
        Value::String(text) => Value::String(text.chars().take(512).collect()),
        value => value.clone(),
    }
}

/// Waits for the passes of one task.
enum Pacer {
    Every(Interval),
    After {
        pause: Duration,
        wake: &'static Notify,
        waits: bool,
    },
}

impl Pacer {
    fn new(pace: Pace) -> Self {
        match pace {
            Pace::Every { period, missed } => {
                let mut ticks = tokio::time::interval(period);
                ticks.set_missed_tick_behavior(missed);
                Self::Every(ticks)
            }
            Pace::After { pause, wake } => Self::After {
                pause,
                wake,
                waits: false,
            },
        }
    }

    /// Returns when the next pass is due.
    async fn next(&mut self) {
        match self {
            Self::Every(ticks) => {
                ticks.tick().await;
            }
            Self::After { pause, wake, waits } => {
                if *waits {
                    tokio::select! {
                        () = tokio::time::sleep(*pause) => {}
                        () = wake.notified() => {}
                    }
                }
                *waits = true;
            }
        }
    }
}

/// Decides which passes record their heartbeat.
struct Reports {
    every: Option<i64>,
    last: Option<(i64, bool)>,
}

impl Reports {
    fn new(every: Option<Duration>) -> Self {
        Self {
            every: every.map(|every| i64::try_from(every.as_secs()).unwrap_or(i64::MAX)),
            last: None,
        }
    }

    /// Whether a pass of this health, finished at `now`, records the heartbeat.
    fn due(&mut self, healthy: bool, now: i64) -> bool {
        if let (Some(every), Some((recorded_at, recorded))) = (self.every, self.last)
            && recorded == healthy
            && now - recorded_at < every
        {
            return false;
        }
        self.last = Some((now, healthy));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Cadence, Pacer, Reports, bounded, supervise_with};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use tokio::sync::Notify;

    const DELAY: Duration = Duration::from_millis(10);

    #[tokio::test]
    async fn panicking_and_returning_attempts_are_restarted() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();
        let supervisor = tokio::spawn(supervise_with("fixture", DELAY, move || {
            let counter = counter.clone();
            async move {
                match counter.fetch_add(1, Ordering::SeqCst) {
                    0 => panic!("fixture panic"),
                    1 => {}
                    _ => std::future::pending().await,
                }
            }
        }));
        tokio::time::timeout(Duration::from_secs(10), async {
            while attempts.load(Ordering::SeqCst) < 3 {
                tokio::time::sleep(DELAY).await;
            }
        })
        .await
        .expect("panicked and returned attempts must be restarted");
        tokio::time::sleep(DELAY * 5).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        supervisor.abort();
        assert!(supervisor.await.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn cancelling_the_supervisor_stops_the_running_attempt() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<()>(1);
        let (started, ready) = tokio::sync::oneshot::channel::<()>();
        let mut started = Some(started);
        let supervisor = tokio::spawn(supervise_with("fixture", DELAY, move || {
            let sender = sender.clone();
            let started = started.take();
            async move {
                let _held = sender;
                if let Some(started) = started {
                    let _ = started.send(());
                }
                std::future::pending::<()>().await
            }
        }));
        ready.await.unwrap();
        supervisor.abort();
        assert!(supervisor.await.unwrap_err().is_cancelled());
        // The attempt held the last sender; aborting it with its supervisor closes the channel.
        assert!(
            tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                .await
                .expect("the supervised attempt must be aborted with its supervisor")
                .is_none()
        );
    }

    #[test]
    fn heartbeats_are_recorded_on_a_change_of_health_or_after_the_reporting_period() {
        let mut every_pass = Reports::new(None);
        assert!(every_pass.due(true, 100));
        assert!(every_pass.due(true, 100));
        let mut throttled = Reports::new(Some(Duration::from_secs(30)));
        assert!(throttled.due(true, 100), "the first pass is recorded");
        assert!(!throttled.due(true, 129));
        assert!(
            throttled.due(false, 129),
            "a change of health is recorded at once"
        );
        assert!(!throttled.due(false, 158));
        assert!(
            throttled.due(false, 159),
            "an unchanged health is recorded once the period has passed"
        );
    }

    #[test]
    fn recorded_details_keep_the_audit_limits() {
        let nested = (0..12).fold(json!("leaf"), |inner, _| json!({ "next": inner }));
        let details = json!({
            "error": "e".repeat(600),
            "steps": (0..40).collect::<Vec<_>>(),
            "nested": nested,
            "completed": false,
        });
        let stored = bounded(&details, 0);
        assert_eq!(
            stored["error"].as_str().map(|e| e.chars().count()),
            Some(512)
        );
        assert_eq!(stored["steps"].as_array().map(Vec::len), Some(32));
        assert_eq!(stored["completed"], false);
        let (mut value, mut depth) = (&stored["nested"], 1);
        while let Some(next) = value.get("next") {
            (value, depth) = (next, depth + 1);
        }
        assert_eq!((value, depth), (&json!("[已截断]"), 9));
    }

    #[tokio::test]
    async fn a_paused_cadence_runs_at_once_and_wakes_early() {
        let wake: &'static Notify = Box::leak(Box::new(Notify::new()));
        let cadence = Cadence::after(Duration::from_secs(3600), wake);
        let mut pacer = Pacer::new(cadence.pace);
        tokio::time::timeout(Duration::from_secs(5), pacer.next())
            .await
            .expect("the first pass starts at once");
        wake.notify_one();
        tokio::time::timeout(Duration::from_secs(5), pacer.next())
            .await
            .expect("a notification starts the next pass early");
        assert!(
            tokio::time::timeout(DELAY * 5, pacer.next()).await.is_err(),
            "without one the next pass waits for its pause"
        );
    }
}
