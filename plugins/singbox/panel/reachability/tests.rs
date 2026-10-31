//! Storage and scheduling against PostgreSQL, with no network or DNS.

use super::{
    NodeReachability, NodeReference, ReachabilityState, alerts,
    budget::BASE_TESTS,
    globalping::{Method, Request, Response, Transport, TransportFailure},
    latest_node_state, store, targets,
    worker::{Resolver, cycle},
};
use anyhow::Result;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::HashMap, net::IpAddr, sync::Mutex};
use uuid::Uuid;

const IP_A: &str = "198.51.100.10";
const IP_B: &str = "198.51.100.20";
const HOSTNAME: &str = "node.example.com";

/// Answers by target, port and location, and can be retuned between runs.
struct FakeGlobalping {
    posts: Mutex<Vec<Value>>,
    /// `(ip, port, "china" | "europe")` to packets received; anything else is zero.
    open: Mutex<HashMap<(String, u16, &'static str), u32>>,
    limits: Mutex<(u16, Value)>,
    post_status: Mutex<u16>,
}

fn limits_body(limit: u32, remaining: u32, reset: u32) -> Value {
    json!({"rateLimit":{"measurements":{"create":{"limit":limit,"remaining":remaining,"reset":reset}}}})
}

impl FakeGlobalping {
    fn new() -> Self {
        Self {
            posts: Mutex::default(),
            open: Mutex::default(),
            limits: Mutex::new((200, limits_body(250, 250, 600))),
            post_status: Mutex::new(202),
        }
    }
    fn open(&self, ip: &str, port: u16, region: &'static str) {
        self.open
            .lock()
            .unwrap()
            .insert((ip.to_owned(), port, region), 3);
    }
    fn budget(&self, limit: u32, remaining: u32, reset: u32) {
        *self.limits.lock().unwrap() = (200, limits_body(limit, remaining, reset));
    }
    fn posts(&self) -> Vec<Value> {
        self.posts.lock().unwrap().clone()
    }
    fn tests_requested(&self) -> u64 {
        self.posts()
            .iter()
            .flat_map(|post| post["locations"].as_array().unwrap().clone())
            .map(|location| location["limit"].as_u64().unwrap())
            .sum()
    }
}

impl Transport for FakeGlobalping {
    async fn send(&self, request: Request) -> Result<Response, TransportFailure> {
        let reply = |status: u16, body: Value| {
            Ok(Response {
                status,
                body: serde_json::to_vec(&body).unwrap(),
            })
        };
        if request.path == "/v1/limits" {
            let (status, body) = self.limits.lock().unwrap().clone();
            return reply(status, body);
        }
        if request.method == Method::Post {
            let body = request.body.unwrap();
            self.posts.lock().unwrap().push(body.clone());
            let status = *self.post_status.lock().unwrap();
            if status != 202 {
                return reply(status, json!({}));
            }
            let region = if body["locations"][0]["magic"] == "Europe" {
                "europe"
            } else {
                "china"
            };
            let id = format!(
                "{}_{}_{}_{}",
                body["target"].as_str().unwrap().replace([':', '.'], "x"),
                body["measurementOptions"]["port"],
                region,
                body["locations"][0]["limit"],
            );
            return reply(202, json!({"id":id}));
        }
        let id = request.path.rsplit('/').next().unwrap();
        let parts: Vec<_> = id.split('_').collect();
        let ip = parts[0].replace('x', ".");
        let port: u16 = parts[1].parse().unwrap();
        let region = if parts[2] == "china" {
            "china"
        } else {
            "europe"
        };
        let limit: u32 = parts[3].parse().unwrap();
        let rcv = self
            .open
            .lock()
            .unwrap()
            .get(&(ip, port, region))
            .copied()
            .unwrap_or(0);
        let results: Vec<Value> = if region == "china" {
            [
                "China Telecom",
                "China Telecom",
                "China Unicom",
                "China Mobile",
            ]
            .iter()
            .map(|network| {
                json!({"probe":{"network":network,"city":"Fixture","country":"CN"},
                        "result":{"stats":{"rcv":rcv,"avg":null}}})
            })
            .collect()
        } else {
            (0..limit)
                .map(|_| {
                    json!({"probe":{"network":"Example Net","city":"Fixture","country":"DE"},
                        "result":{"stats":{"rcv":rcv,"avg":null}}})
                })
                .collect()
        };
        reply(200, json!({"status":"finished","results":results}))
    }
}

#[derive(Default)]
struct FakeResolver {
    answers: HashMap<String, Vec<IpAddr>>,
}

impl FakeResolver {
    fn with(mut self, host: &str, ip: &str) -> Self {
        self.answers
            .insert(host.to_owned(), vec![ip.parse().unwrap()]);
        self
    }
}

impl Resolver for FakeResolver {
    async fn resolve(&self, host: &str, _port: u16) -> Vec<IpAddr> {
        host.parse()
            .map(|ip| vec![ip])
            .unwrap_or_else(|_| self.answers.get(host).cloned().unwrap_or_default())
    }
}

async fn server(pool: &PgPool, name: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar("INSERT INTO servers(name) VALUES($1) RETURNING id")
            .bind(name)
            .fetch_one(pool)
            .await?,
    )
}

async fn native_node(pool: &PgPool, server: i64, host: &str, port: i32) -> Result<i64> {
    Ok(sqlx::query_scalar("INSERT INTO nodes(name,server_id,port,public_host,sni,private_key,public_key,short_id) VALUES($1,$2,$3,$4,'www.example.com','fixture-private','fixture-public','0123abcd') RETURNING id")
        .bind(format!("node-{port}"))
        .bind(server)
        .bind(port)
        .bind(host)
        .fetch_one(pool)
        .await?)
}

async fn imported_node(pool: &PgPool, server: i64, legacy: i64, display: Value) -> Result<i64> {
    sqlx::query("INSERT INTO singbox_nexus_node_imports(source_id,digest,infrastructure_digest,exported_at,imported_at,node_count) VALUES('fixture',$1,$1,1,1,1) ON CONFLICT DO NOTHING")
        .bind("a".repeat(64))
        .execute(pool)
        .await?;
    let credential = Uuid::new_v4();
    sqlx::query("INSERT INTO credential_entries(id,name,kind,key_id,nonce,ciphertext,created_at,updated_at) VALUES($1,'fixture','backup','fixture',$2,$3,0,0)")
        .bind(credential)
        .bind(vec![0_u8; 12])
        .bind(vec![0_u8; 16])
        .execute(pool)
        .await?;
    Ok(sqlx::query_scalar("INSERT INTO singbox_nexus_nodes(source_id,legacy_node_id,legacy_machine_id,legacy_host_id,server_id,credential_id,display) VALUES('fixture',$1,1,1,$2,$3,$4) RETURNING id")
        .bind(legacy)
        .bind(server)
        .bind(credential)
        .bind(display)
        .fetch_one(pool)
        .await?)
}

async fn enable(pool: &PgPool) -> Result<()> {
    sqlx::query("UPDATE singbox_reachability_settings SET enabled=TRUE WHERE singleton")
        .execute(pool)
        .await?;
    Ok(())
}

/// Moves every clock in the schedule so the next cycle treats targets as due.
async fn time_passes(pool: &PgPool) -> Result<()> {
    sqlx::query("UPDATE singbox_reachability_settings SET next_run_at=0 WHERE singleton")
        .execute(pool)
        .await?;
    sqlx::query("UPDATE singbox_reachability_targets SET checked_at=checked_at-86400")
        .execute(pool)
        .await?;
    Ok(())
}

async fn state_of(pool: &PgPool, host: &str, port: i32) -> Result<(String, Option<String>, i32)> {
    Ok(sqlx::query_as("SELECT state,pending_state,failing_streak FROM singbox_reachability_targets WHERE host=$1 AND port=$2")
        .bind(host)
        .bind(port)
        .fetch_one(pool)
        .await?)
}

async fn count(pool: &PgPool, table: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await?)
}

async fn last_run(pool: &PgPool) -> Result<Value> {
    Ok(store::read_settings(pool).await?.last_run)
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn the_monitor_is_off_by_default_and_sends_nothing(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, IP_A, 443).await?;
    let settings = store::read_settings(&pool).await?;
    assert!(!settings.enabled);
    assert_eq!(settings.interval_minutes, 30);
    assert!(settings.credential_id.is_none());
    let fake = FakeGlobalping::new();
    assert!(!cycle(&pool, &fake, &FakeResolver::default()).await?);
    assert!(fake.posts().is_empty());
    assert_eq!(count(&pool, "singbox_reachability_targets").await?, 0);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn two_failing_runs_enter_a_block_and_one_good_run_leaves_it(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    let node = native_node(&pool, id, IP_A, 8443).await?;
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    fake.open(IP_A, 8443, "europe");
    fake.open(IP_A, 22, "europe");
    let resolver = FakeResolver::default();

    assert!(cycle(&pool, &fake, &resolver).await?);
    assert_eq!(
        state_of(&pool, IP_A, 8443).await?,
        ("unknown".into(), Some("ip_blocked".into()), 1)
    );
    assert_eq!(count(&pool, "singbox_reachability_events").await?, 0);
    // Not yet confirmed, so the takeover gate must not read it as blocked.
    let pending = latest_node_state(&pool, NodeReference::Native(node))
        .await?
        .unwrap();
    assert!(!pending.blocked);
    assert_eq!(pending.pending, Some(ReachabilityState::IpBlocked));

    time_passes(&pool).await?;
    assert!(cycle(&pool, &fake, &resolver).await?);
    assert_eq!(
        state_of(&pool, IP_A, 8443).await?,
        ("ip_blocked".into(), None, 2)
    );
    let (kind, message): (String, String) =
        sqlx::query_as("SELECT kind,message FROM singbox_reachability_events")
            .fetch_one(&pool)
            .await?;
    assert_eq!(kind, "blocked");
    assert!(message.contains("IP 被封锁"));
    assert!(message.contains("电信 0/2"));
    assert!(!message.contains(IP_A), "alerts must not carry the address");
    let blocked = latest_node_state(&pool, NodeReference::Native(node))
        .await?
        .unwrap();
    assert!(matches!(
        blocked,
        NodeReachability {
            state: ReachabilityState::IpBlocked,
            blocked: true,
            pending: None,
            checked_at: Some(_),
        }
    ));

    // Staying blocked raises no further alert.
    time_passes(&pool).await?;
    assert!(cycle(&pool, &fake, &resolver).await?);
    assert_eq!(count(&pool, "singbox_reachability_events").await?, 1);

    fake.open(IP_A, 8443, "china");
    time_passes(&pool).await?;
    assert!(cycle(&pool, &fake, &resolver).await?);
    assert_eq!(
        state_of(&pool, IP_A, 8443).await?,
        ("reachable".into(), None, 0)
    );
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM singbox_reachability_events ORDER BY created_at,kind")
            .fetch_all(&pool)
            .await?;
    assert_eq!(kinds, ["blocked", "cleared"]);
    assert_eq!(count(&pool, "singbox_reachability_history").await?, 4);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn a_closed_control_port_is_not_reported_as_an_ip_block(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, IP_A, 8443).await?;
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    fake.open(IP_A, 8443, "europe");
    for _ in 0..2 {
        assert!(cycle(&pool, &fake, &FakeResolver::default()).await?);
        time_passes(&pool).await?;
    }
    assert_eq!(state_of(&pool, IP_A, 8443).await?.0, "unconfirmed");
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn overseas_failure_is_service_down_and_raises_no_block_alert(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, IP_A, 443).await?;
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    assert!(cycle(&pool, &fake, &FakeResolver::default()).await?);
    assert_eq!(state_of(&pool, IP_A, 443).await?.0, "service_down");
    assert_eq!(count(&pool, "singbox_reachability_events").await?, 0);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn budget_defers_the_remainder_and_the_next_run_takes_it_first(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    for port in [1001, 1002, 1003] {
        native_node(&pool, id, IP_A, port).await?;
    }
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    for port in [1001, 1002, 1003] {
        fake.open(IP_A, port, "china");
        fake.open(IP_A, port, "europe");
    }
    // 80% of 100 is 80; 74 used leaves exactly one target's base measurements.
    fake.budget(100, 26, 900);
    let resolver = FakeResolver::default();

    assert!(cycle(&pool, &fake, &resolver).await?);
    let summary = last_run(&pool).await?;
    assert_eq!(summary["measured"], 1);
    assert_eq!(summary["deferred"], 2);
    assert_eq!(summary["error"], Value::Null);
    assert!(fake.tests_requested() <= 6);
    let deferred: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM singbox_reachability_targets WHERE deferred_at IS NOT NULL",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(deferred, 2);
    let settings = store::read_settings(&pool).await?;
    let now = apsp_protocol::now_timestamp();
    // The retry waits for the provider's window, well before a full interval.
    assert!(settings.next_run_at >= now + 800 && settings.next_run_at <= now + 910);

    // Only the deferred targets are due; the measured one keeps its interval.
    fake.posts.lock().unwrap().clear();
    fake.budget(100, 100, 600);
    sqlx::query("UPDATE singbox_reachability_settings SET next_run_at=0 WHERE singleton")
        .execute(&pool)
        .await?;
    assert!(cycle(&pool, &fake, &resolver).await?);
    let summary = last_run(&pool).await?;
    assert_eq!(summary["measured"], 2);
    assert_eq!(summary["deferred"], 0);
    assert_eq!(fake.posts().len(), 4);
    let deferred: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM singbox_reachability_targets WHERE deferred_at IS NOT NULL",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(deferred, 0);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn a_spent_budget_sends_no_measurement_and_changes_no_state(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, IP_A, 443).await?;
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    fake.budget(250, 50, 300);
    assert!(cycle(&pool, &fake, &FakeResolver::default()).await?);
    assert!(fake.posts().is_empty());
    assert_eq!(
        state_of(&pool, IP_A, 443).await?,
        ("unknown".into(), None, 0)
    );
    assert_eq!(last_run(&pool).await?["deferred"], 1);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn a_failed_budget_query_fails_closed(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, IP_A, 443).await?;
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    *fake.limits.lock().unwrap() = (500, json!({}));
    assert!(cycle(&pool, &fake, &FakeResolver::default()).await?);
    assert!(fake.posts().is_empty());
    assert_eq!(last_run(&pool).await?["error"], "limits_http_500");
    let settings = store::read_settings(&pool).await?;
    assert!(settings.next_run_at <= apsp_protocol::now_timestamp() + 300);
    assert!(settings.lease_until.is_none());
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn a_rate_limit_mid_run_defers_rather_than_recording_a_failure(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, IP_A, 443).await?;
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    *fake.post_status.lock().unwrap() = 429;
    assert!(cycle(&pool, &fake, &FakeResolver::default()).await?);
    assert_eq!(
        state_of(&pool, IP_A, 443).await?,
        ("unknown".into(), None, 0)
    );
    assert_eq!(count(&pool, "singbox_reachability_history").await?, 0);
    assert_eq!(last_run(&pool).await?["deferred"], 1);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn only_ip_literals_leave_the_panel(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, HOSTNAME, 443).await?;
    native_node(&pool, id, "unresolvable.example.com", 8443).await?;
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    fake.open(IP_B, 443, "china");
    fake.open(IP_B, 443, "europe");
    let resolver = FakeResolver::default().with(HOSTNAME, IP_B);
    assert!(cycle(&pool, &fake, &resolver).await?);
    assert_eq!(state_of(&pool, HOSTNAME, 443).await?.0, "reachable");
    for post in fake.posts() {
        assert_eq!(post["target"], IP_B);
        assert!(!post.to_string().contains("example.com"));
    }
    let (state, detail): (String, Value) = sqlx::query_as(
        "SELECT state,detail FROM singbox_reachability_targets WHERE host='unresolvable.example.com'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(state, "unknown");
    assert_eq!(detail["reason"], "no_public_address");
    assert_eq!(last_run(&pool).await?["unresolved"], 1);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn targets_dedupe_by_endpoint_and_share_a_measurement_per_address(
    pool: PgPool,
) -> Result<()> {
    let first = server(&pool, "first").await?;
    let second = server(&pool, "second").await?;
    native_node(&pool, first, HOSTNAME, 443).await?;
    imported_node(
        &pool,
        second,
        1,
        json!({"name":"imported","public_host":"NODE.example.com.","public_port":"443",
            "protocol":"vless","enabled":true,"protocol_summary":{"tls":2}}),
    )
    .await?;
    native_node(&pool, second, "alias.example.com", 443).await?;
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    fake.open(IP_B, 443, "china");
    fake.open(IP_B, 443, "europe");
    let resolver = FakeResolver::default()
        .with(HOSTNAME, IP_B)
        .with("alias.example.com", IP_B);
    assert!(cycle(&pool, &fake, &resolver).await?);
    assert_eq!(count(&pool, "singbox_reachability_targets").await?, 2);
    assert_eq!(
        fake.posts().len(),
        2,
        "one China and one overseas measurement"
    );
    assert_eq!(state_of(&pool, HOSTNAME, 443).await?.0, "reachable");
    assert_eq!(
        state_of(&pool, "alias.example.com", 443).await?.0,
        "reachable"
    );
    let listed = targets::enumerate(&pool).await?;
    let shared = listed
        .iter()
        .find(|target| target.host == HOSTNAME)
        .unwrap();
    assert_eq!(shared.nodes.len(), 2);
    assert_eq!(last_run(&pool).await?["tests_used"], BASE_TESTS);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn udp_only_endpoints_use_no_tests_and_never_read_as_down(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    let udp = imported_node(
        &pool,
        id,
        2,
        json!({"name":"hop","public_host":IP_A,"public_port":"21000-21020",
            "protocol":"hysteria","enabled":true,"protocol_summary":{"version":2}}),
    )
    .await?;
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    assert!(cycle(&pool, &fake, &FakeResolver::default()).await?);
    assert!(fake.posts().is_empty());
    assert_eq!(state_of(&pool, IP_A, 21000).await?.0, "not_applicable");
    assert_eq!(count(&pool, "singbox_reachability_history").await?, 1);
    let state = latest_node_state(&pool, NodeReference::Nexus(udp))
        .await?
        .unwrap();
    assert_eq!(state.state, ReachabilityState::NotApplicable);
    assert!(!state.blocked);
    // The label is written once rather than on every run.
    time_passes(&pool).await?;
    assert!(cycle(&pool, &fake, &FakeResolver::default()).await?);
    assert_eq!(count(&pool, "singbox_reachability_history").await?, 1);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn disabled_and_deleted_nodes_are_not_targets(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    let gone = native_node(&pool, id, IP_A, 1001).await?;
    let off = native_node(&pool, id, IP_A, 1002).await?;
    native_node(&pool, id, IP_A, 1003).await?;
    sqlx::query("UPDATE nodes SET deleted_at=1 WHERE id=$1")
        .bind(gone)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE nodes SET enabled=FALSE WHERE id=$1")
        .bind(off)
        .execute(&pool)
        .await?;
    imported_node(
        &pool,
        id,
        3,
        json!({"name":"off","public_host":IP_A,"public_port":"1004","protocol":"vless","enabled":false}),
    )
    .await?;
    imported_node(
        &pool,
        id,
        4,
        json!({"name":"retired","public_host":IP_A,"public_port":"1005","protocol":"vless","enabled":true,"machine_active":false}),
    )
    .await?;
    let ports: Vec<u16> = targets::enumerate(&pool)
        .await?
        .iter()
        .map(|target| target.port)
        .collect();
    assert_eq!(ports, [1003]);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn opting_out_stops_addresses_already_queued(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, IP_A, 443).await?;
    enable(&pool).await?;
    let fake = FakeGlobalping::new();
    let claim = store::claim_run(&pool, apsp_protocol::now_timestamp())
        .await?
        .unwrap();
    sqlx::query("UPDATE singbox_reachability_settings SET enabled=FALSE WHERE singleton")
        .execute(&pool)
        .await?;
    let (summary, _) =
        super::worker::run_once(&pool, &fake, &FakeResolver::default(), &claim).await?;
    assert!(fake.posts().is_empty());
    assert_eq!(summary.measured, 0);
    assert_eq!(summary.deferred, 1);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn a_requested_run_overrides_the_schedule_but_not_the_lease_or_opt_in(
    pool: PgPool,
) -> Result<()> {
    let now = apsp_protocol::now_timestamp();
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, IP_A, 443).await?;
    let fake = FakeGlobalping::new();
    fake.open(IP_A, 443, "china");
    fake.open(IP_A, 443, "europe");
    let resolver = FakeResolver::default();
    enable(&pool).await?;
    assert!(cycle(&pool, &fake, &resolver).await?);
    // Scheduled runs wait for the interval.
    assert!(!cycle(&pool, &fake, &resolver).await?);
    fake.posts.lock().unwrap().clear();
    sqlx::query("UPDATE singbox_reachability_settings SET run_requested_at=$1 WHERE singleton")
        .bind(now)
        .execute(&pool)
        .await?;
    // A manual run measures targets that are not yet due.
    assert!(cycle(&pool, &fake, &resolver).await?);
    assert_eq!(fake.posts().len(), 2);
    assert_eq!(last_run(&pool).await?["manual"], true);
    assert!(
        store::read_settings(&pool)
            .await?
            .run_requested_at
            .is_none()
    );

    sqlx::query("UPDATE singbox_reachability_settings SET run_requested_at=$1,lease_until=$2 WHERE singleton")
        .bind(now)
        .bind(now + 600)
        .execute(&pool)
        .await?;
    assert!(!cycle(&pool, &fake, &resolver).await?);
    store::release_lease(&pool).await?;
    sqlx::query("UPDATE singbox_reachability_settings SET enabled=FALSE WHERE singleton")
        .execute(&pool)
        .await?;
    assert!(!cycle(&pool, &fake, &resolver).await?);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn alerts_queue_per_ready_channel_and_are_kept_when_none_is_ready(
    pool: PgPool,
) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, IP_A, 8443).await?;
    enable(&pool).await?;
    sqlx::query("UPDATE panel_settings SET settings=settings||$1 WHERE singleton")
        .bind(json!({"telegram_enabled":true,"telegram_token":"fixture-token","telegram_chat_id":"-100"}))
        .execute(&pool)
        .await?;
    let fake = FakeGlobalping::new();
    fake.open(IP_A, 8443, "europe");
    for _ in 0..2 {
        assert!(cycle(&pool, &fake, &FakeResolver::default()).await?);
        time_passes(&pool).await?;
    }
    let (channel, state, payload): (String, String, String) =
        sqlx::query_as("SELECT channel,state,payload FROM singbox_reachability_deliveries")
            .fetch_one(&pool)
            .await?;
    assert_eq!(channel, "telegram");
    assert_eq!(state, "pending");
    assert!(payload.contains("中国可达性告警"));
    assert!(!payload.contains(IP_A));

    // Turning the channel off cancels instead of sending; no network is reached.
    sqlx::query("UPDATE panel_settings SET settings=settings||'{\"telegram_enabled\":false}' WHERE singleton")
        .execute(&pool)
        .await?;
    alerts::deliver(&pool).await?;
    let state: String = sqlx::query_scalar("SELECT state FROM singbox_reachability_deliveries")
        .fetch_one(&pool)
        .await?;
    assert_eq!(state, "cancelled");

    // With no channel at all the event is still recorded.
    fake.open(IP_A, 8443, "china");
    assert!(cycle(&pool, &fake, &FakeResolver::default()).await?);
    assert_eq!(count(&pool, "singbox_reachability_events").await?, 2);
    assert_eq!(count(&pool, "singbox_reachability_deliveries").await?, 1);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn retention_trims_old_history_events_and_abandoned_targets(pool: PgPool) -> Result<()> {
    let now = apsp_protocol::now_timestamp();
    let target = |host: &'static str, seen: i64| {
        sqlx::query_scalar::<sqlx::Postgres, i64>("INSERT INTO singbox_reachability_targets(host,port,seen_at) VALUES($1,443,$2) RETURNING id")
            .bind(host)
            .bind(seen)
    };
    let kept = target("kept.example.com", now).fetch_one(&pool).await?;
    let abandoned = target("gone.example.com", now - 40 * 86400)
        .fetch_one(&pool)
        .await?;
    for (id, at) in [
        (kept, now),
        (kept, now - 31 * 86400),
        (abandoned, now - 5 * 86400),
    ] {
        sqlx::query("INSERT INTO singbox_reachability_history(target_id,checked_at,state,observed_state,detail) VALUES($1,$2,'unknown','unknown','{}')")
            .bind(id)
            .bind(at)
            .execute(&pool)
            .await?;
    }
    for at in [now, now - 91 * 86400] {
        sqlx::query("INSERT INTO singbox_reachability_events(id,target_id,kind,from_state,to_state,message,created_at) VALUES($1,$2,'blocked','unknown','ip_blocked','fixture',$3)")
            .bind(Uuid::new_v4())
            .bind(abandoned)
            .bind(at)
            .execute(&pool)
            .await?;
    }
    store::cleanup(&pool, now).await?;
    let remaining: Vec<i64> =
        sqlx::query_scalar("SELECT checked_at FROM singbox_reachability_history")
            .fetch_all(&pool)
            .await?;
    assert_eq!(remaining, [now]);
    let hosts: Vec<String> = sqlx::query_scalar("SELECT host FROM singbox_reachability_targets")
        .fetch_all(&pool)
        .await?;
    assert_eq!(hosts, ["kept.example.com"]);
    // The event outlives its target; its age alone decides when it goes.
    let events: Vec<Option<i64>> =
        sqlx::query_scalar("SELECT target_id FROM singbox_reachability_events")
            .fetch_all(&pool)
            .await?;
    assert_eq!(events, [None]);
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn latest_state_is_absent_for_unknown_and_unmeasured_nodes(pool: PgPool) -> Result<()> {
    assert!(
        latest_node_state(&pool, NodeReference::Native(9_999))
            .await?
            .is_none()
    );
    assert!(
        latest_node_state(&pool, NodeReference::Nexus(9_999))
            .await?
            .is_none()
    );
    let id = server(&pool, "fixture").await?;
    let node = native_node(&pool, id, IP_A, 443).await?;
    assert!(
        latest_node_state(&pool, NodeReference::Native(node))
            .await?
            .is_none()
    );
    // A row synced but never measured is still absent.
    store::sync_targets(
        &pool,
        &targets::enumerate(&pool).await?,
        apsp_protocol::now_timestamp(),
    )
    .await?;
    assert!(
        latest_node_state(&pool, NodeReference::Native(node))
            .await?
            .is_none()
    );
    Ok(())
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn risk_flags_come_from_stored_configuration(pool: PgPool) -> Result<()> {
    let id = server(&pool, "fixture").await?;
    native_node(&pool, id, IP_A, 8443).await?;
    sqlx::query("UPDATE nodes SET sni='mask.example.ru',settings=settings||$1")
        .bind(json!({"public_port":443,"reality":{"handshake_server":"www.apple.com"}}))
        .execute(&pool)
        .await?;
    imported_node(
        &pool,
        id,
        5,
        json!({"name":"imported","public_host":IP_B,"public_port":"2443","protocol":"vless",
            "enabled":true,"protocol_summary":{"tls":2}}),
    )
    .await?;
    let listed = targets::enumerate(&pool).await?;
    let native = listed.iter().find(|target| target.host == IP_A).unwrap();
    assert_eq!(native.port, 443);
    let codes: Vec<_> = native.risks.iter().map(|risk| risk.code).collect();
    assert_eq!(codes, ["name_risky_brand", "name_risky_tld"]);
    let imported = listed.iter().find(|target| target.host == IP_B).unwrap();
    let codes: Vec<_> = imported.risks.iter().map(|risk| risk.code).collect();
    assert_eq!(codes, ["port_not_443"]);
    Ok(())
}
