use super::super::super::{account_on, lock, model::Policy, resource};
use super::super::jobs;
use super::super::{Job, scheduler};
use super::*;
use crate::cloud_api::test_support::{Mock, Reply};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const GIB: u64 = 1024 * 1024 * 1024;
const KEY: &str = "TEST_ONLY_KEY";
fn config() -> Policy {
    serde_json::from_value(json!({"enabled":true,"stop_mode":"StopCharging","threshold_action":"off",
        "limit_gb":100,"threshold_percent":95,"schedule_enabled":false,"start_time":"08:00","stop_time":"23:00",
        "utc_offset_minutes":480,"keepalive":false,"cdt_guard":{"threshold_bytes":150*GIB,"jump_bytes":20*GIB,"auto_start":true}})).unwrap()
}
async fn seed(pool: &PgPool) -> (Account, Resource) {
    let aid = Uuid::new_v4();
    let rid = Uuid::new_v4();
    sqlx::query("INSERT INTO alicloud_accounts(id,name,access_key_id,access_key_secret) VALUES($1,'测试云账号',$2,'TEST_ONLY_SECRET')")
        .bind(aid).bind(KEY).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id,power_policy) VALUES($1,$2,'测试实例','ecs','cn-hongkong','i-guardtest',$3)")
        .bind(rid).bind(aid).bind(Json(config())).execute(pool).await.unwrap();
    let mut tx = lock(pool, aid).await.unwrap();
    let a = account_on(&mut tx, aid).await.unwrap();
    tx.commit().await.unwrap();
    (a, resource(pool, rid).await.unwrap())
}
fn row(region: &str, amount: u64) -> Value {
    json!({"BusinessRegionId":region,"ISPType":"BGP","Traffic":amount.to_string(),
        "ProductTrafficDetails":[{"Product":"SharedBandwidth","Traffic":(amount/2).to_string()},
        {"Product":"IPv6","Traffic":(amount-amount/2).to_string()}]})
}
fn traffic(amount: u64) -> Reply {
    Reply::ok(
        "ListCdtInternetTraffic",
        json!({"RequestId":"meter","TrafficDetails":[row("cn-hongkong", amount)]}),
    )
}
fn power(status: &str) -> Reply {
    Reply::ok(
        "DescribeInstances",
        json!({"RequestId":"power","TotalCount":1,"Instances":{"Instance":[{
        "InstanceId":"i-guardtest","RegionId":"cn-hongkong","Status":status,"StoppedMode":"StopCharging",
        "InstanceChargeType":"PostPaid","InstanceNetworkType":"vpc","SpotStrategy":"NoSpot",
        "Ipv6Address":["2001:db8::1"],"PublicIpAddress":{"IpAddress":[]},"OperationLocks":{"LockReason":[]}}]}}),
    )
}
fn observation(a: &Account, r: &Resource, usage: u64, jump: u64, now: i64) -> GuardState {
    let mut value = observe(None, usage, a, r, &key_digest(KEY), now).unwrap();
    value.max_jump_bytes = jump;
    value
}
async fn save_observation(pool: &PgPool, r: Uuid, value: &GuardState) {
    sqlx::query("UPDATE alicloud_resources SET cdt_guard_state=$2 WHERE id=$1")
        .bind(r)
        .bind(Json(value))
        .execute(pool)
        .await
        .unwrap();
}
async fn queued(pool: &PgPool, rid: Uuid) -> Vec<Job> {
    sqlx::query_as("SELECT * FROM alicloud_power_jobs WHERE resource_id=$1 AND status='queued'")
        .bind(rid)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[test]
fn bgp_pool_includes_other_regions_and_ipv6_but_not_mainland() {
    let value = json!({"TrafficDetails":[row("cn-hongkong", 45*GIB), row("ap-southeast-1", 30*GIB), row("cn-hangzhou", 10*GIB)]});
    assert_eq!(
        meter::traffic(&value, "cn-hongkong").unwrap(),
        Some(75 * GIB)
    );
    assert_eq!(
        meter::traffic(&value, "cn-shanghai").unwrap(),
        Some(10 * GIB)
    );
    let mut duplicate = value.clone();
    duplicate["TrafficDetails"][1] = duplicate["TrafficDetails"][0].clone();
    assert!(meter::traffic(&duplicate, "cn-hongkong").is_err());
    for path in [
        "ISPType",
        "BusinessRegionId",
        "ProductTrafficDetails",
        "Traffic",
    ] {
        let mut invalid = value.clone();
        invalid["TrafficDetails"][0]
            .as_object_mut()
            .unwrap()
            .remove(path);
        assert!(meter::traffic(&invalid, "cn-hongkong").is_err(), "{path}");
    }
    for (key, value) in [
        ("Traffic", json!("-1")),
        ("ISPType", json!("BGP_PRO")),
        ("Traffic", json!(10)),
    ] {
        let mut invalid = json!({"TrafficDetails":[row("cn-hongkong", 100)]});
        invalid["TrafficDetails"][0][key] = value;
        assert!(meter::traffic(&invalid, "cn-hongkong").is_err());
    }
    for (key, extra) in [("NextToken", json!("next")), ("TotalCount", json!(4))] {
        let mut partial = value.clone();
        partial[key] = extra;
        assert!(meter::traffic(&partial, "cn-hongkong").is_err());
    }
}

#[test]
fn script_mode_is_opt_in_and_excludes_conflicting_automation() {
    let mut p = config();
    p.validate().unwrap();
    p.keepalive = true;
    p.schedule_enabled = true;
    assert!(p.validate().is_ok());
    p.keepalive = false;
    p.threshold_action = "stop".into();
    assert!(p.validate().is_err());
    p.threshold_action = "off".into();
    p.cdt_guard.as_mut().unwrap().jump_bytes = 0;
    assert!(p.validate().is_err());
    let mut old = serde_json::to_value(config()).unwrap();
    old.as_object_mut().unwrap().remove("cdt_guard");
    assert!(
        serde_json::from_value::<Policy>(old)
            .unwrap()
            .cdt_guard
            .is_none()
    );
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn combined_guard_cron_and_keepalive_preserve_stop_priority_and_start_gates(pool: PgPool) {
    use super::super::super::model::ScheduleMode;
    use super::super::scheduler;
    let (account, mut resource) = seed(&pool).await;
    let base = 1_793_923_200; // Friday 08:00 at UTC+08.
    let state: State = serde_json::from_value(json!({"cloud_id":"i-guardtest","region":"cn-hongkong","status":"Stopped","stopped_mode":"StopCharging","charge_type":"PostPaid","network_type":"vpc","spot_strategy":"SpotAsPriceGo","interruption_behavior":"Stop","public_ips":[],"locked":false})).unwrap();
    for (minute, over, stale, hold, running, same_time, expected) in [
        (0, false, false, false, false, false, Some("start")),
        (0, true, false, false, true, false, Some("stop")),
        (0, false, true, false, false, false, None),
        (0, false, false, true, false, false, None),
        (120, false, false, false, false, false, None),
        (60, false, true, false, true, false, Some("stop")),
        (0, false, false, false, true, true, Some("stop")),
        (30, false, false, false, false, false, Some("start")),
    ] {
        let now = base + minute * 60;
        resource.power_policy.0 = config();
        resource.power_policy.schedule_enabled = true;
        resource.power_policy.schedule_mode = ScheduleMode::Cron;
        resource.power_policy.start_cron = "0 8 * * 1-5".into();
        resource.power_policy.stop_cron = if same_time {
            "0 8 * * 1-5"
        } else {
            "0 9 * * 1-5"
        }
        .into();
        resource.power_policy.keepalive = true;
        resource.power_policy.cdt_guard.as_mut().unwrap().auto_start = false;
        resource.manual_hold = hold;
        resource.next_power_at = 0;
        resource.cdt_guard_state = Some(Json(observation(
            &account,
            &resource,
            if over { 200 * GIB } else { 10 * GIB },
            0,
            if stale { now - 121 } else { now },
        )));
        resource.cdt_guard_error = None;
        let mut state = state.clone();
        state.status = if running { "Running" } else { "Stopped" }.into();
        let mut tx = pool.begin().await.unwrap();
        scheduler::evaluate(&mut tx, &account, &mut resource, &state, now)
            .await
            .unwrap();
        let jobs: Vec<Job> =
            sqlx::query_as("SELECT * FROM alicloud_power_jobs WHERE resource_id=$1")
                .bind(resource.id)
                .fetch_all(&mut *tx)
                .await
                .unwrap();
        assert_eq!(
            jobs.first().map(|job| job.action.as_str()),
            expected,
            "minute={minute} over={over} stale={stale} hold={hold} same={same_time}"
        );
        if let Some(job) = jobs.first() {
            assert!(jobs::allowed(job, &account, &resource, now));
            if job.action == "start" {
                resource.manual_hold = true;
                assert!(!jobs::allowed(job, &account, &resource, now));
                resource.manual_hold = false;
                resource.cdt_guard_error = Some("request_timeout".into());
                assert!(!jobs::allowed(job, &account, &resource, now));
            }
        }
        tx.rollback().await.unwrap();
    }
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn guard_backoff_skips_meter_but_still_evaluates_scheduled_stop(pool: PgPool) {
    let (account, mut resource) = seed(&pool).await;
    let now = apsp_protocol::now_timestamp();
    let local_minute = (now + 28800).rem_euclid(86400) / 60;
    resource.power_policy.schedule_enabled = true;
    resource.power_policy.stop_time = format!("{:02}:{:02}", local_minute / 60, local_minute % 60);
    let start = (local_minute + 720) % 1440;
    resource.power_policy.start_time = format!("{:02}:{:02}", start / 60, start % 60);
    resource.cdt_guard_next_at = now + 3600;
    resource.cdt_guard_error = Some("rate_limited".into());
    let state: State = serde_json::from_value(json!({"cloud_id":"i-guardtest","region":"cn-hongkong","status":"Running","stopped_mode":null,"charge_type":"PostPaid","network_type":"vpc","spot_strategy":"NoSpot","interruption_behavior":null,"public_ips":[],"locked":false})).unwrap();
    let mock = Mock::start(vec![]).await;
    let mut tx = pool.begin().await.unwrap();
    poll(
        &mut tx,
        &account,
        &mut resource,
        &state,
        &Cloud::local(&mock.endpoint),
        now,
    )
    .await
    .unwrap();
    let job: Job = sqlx::query_as("SELECT * FROM alicloud_power_jobs WHERE resource_id=$1")
        .bind(resource.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(job.action, "stop");
    assert!(jobs::allowed(&job, &account, &resource, now));
    assert!(mock.requests().is_empty());
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn monthly_high_water_jump_reset_and_freshness_are_preserved(pool: PgPool) {
    let (a, mut r) = seed(&pool).await;
    let now = 1_790_812_800; // 2026-10-01 00:00 UTC.
    let old = observation(&a, &r, 90 * GIB, 45 * GIB, now);
    let next = observe(Some(&old), 110 * GIB, &a, &r, &key_digest(KEY), now + 60).unwrap();
    assert_eq!(next.max_jump_bytes, 45 * GIB);
    let lower = observe(Some(&next), 80 * GIB, &a, &r, &key_digest(KEY), now + 120).unwrap();
    assert_eq!(lower.max_usage_bytes, 110 * GIB);
    r.cdt_guard_state = Some(Json(lower));
    assert_eq!(exceeded(&a, &r, now + 120), Some(true));
    assert_eq!(exceeded(&a, &r, now + 241), None);
    let mut revised = a.clone();
    revised.revision += 1;
    assert_eq!(exceeded(&revised, &r, now + 120), None);
    assert!(observe(Some(&old), 100 * GIB, &a, &r, "other-key", now + 120).is_err());
    let mut previous_month = old.clone();
    previous_month.billing_cycle = "2026-09".into();
    assert_eq!(
        observe(
            Some(&previous_month),
            100 * GIB,
            &a,
            &r,
            &key_digest(KEY),
            now
        )
        .unwrap_err()
        .code,
        "cdt_reset_pending"
    );
    let reset = observe(Some(&previous_month), GIB, &a, &r, &key_digest(KEY), now).unwrap();
    assert_eq!((reset.max_usage_bytes, reset.max_jump_bytes), (GIB, 0));
    let mut future = old.clone();
    future.billing_cycle = "2027-01".into();
    assert!(observe(Some(&future), 0, &a, &r, &key_digest(KEY), now).is_err());
    assert_ne!(billing::month(now - 28801), billing::month(now - 28800));
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn buffered_threshold_stops_once_using_existing_durable_jobs_without_bills(pool: PgPool) {
    let (a, r) = seed(&pool).await;
    save_observation(
        &pool,
        r.id,
        &observation(&a, &r, 90 * GIB, 45 * GIB, apsp_protocol::now_timestamp()),
    )
    .await;
    let mock = Mock::start(vec![
        power("Running"),
        traffic(110 * GIB),
        power("Running"),
        Reply::ok("StopInstance", json!({"RequestId":"stop"})),
        power("Stopped"),
    ])
    .await;
    let cloud = Cloud::local(&mock.endpoint);
    scheduler::poll(&pool, r.id, &cloud).await.unwrap();
    let jobs = queued(&pool, r.id).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].source, "cdt_guard");
    let saved = resource(&pool, r.id).await.unwrap();
    assert_eq!(saved.cdt_guard_state.unwrap().max_usage_bytes, 110 * GIB);
    jobs::process(&pool, jobs[0].id, &cloud).await.unwrap();
    jobs::process(&pool, jobs[0].id, &cloud).await.unwrap();
    let requests = mock.requests();
    assert!(!requests[1].params.contains_key("BusinessRegionId"));
    assert_eq!(requests[3].params["ForceStop"], "false");
    assert_eq!(requests[3].params["StoppedMode"], "StopCharging");
    assert_eq!(requests[3].params["InstanceId"], "i-guardtest");
    assert!(!requests[3].params.contains_key("BatchOptimization"));
    assert!(!requests[3].params.contains_key("InstanceId.1"));
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.action == "StopInstance")
            .count(),
        1
    );
    mock.exhausted();
}

async fn single_start(pool: &PgPool, code: &str, expected: &str) {
    let (_, r) = seed(pool).await;
    let mut replies = vec![
        power("Stopped"),
        traffic(10 * GIB),
        power("Stopped"),
        if code == "200" {
            Reply::ok("StartInstance", json!({"RequestId":"start"}))
        } else {
            Reply {
                action: "StartInstance".into(),
                status: 400,
                value: json!({"RequestId":"start", "Code":code}),
            }
        },
    ];
    if code == "200" {
        replies.push(power("Running"));
    }
    let mock = Mock::start(replies).await;
    let cloud = Cloud::local(&mock.endpoint);
    scheduler::poll(pool, r.id, &cloud).await.unwrap();
    let id = queued(pool, r.id).await[0].id;
    jobs::process(pool, id, &cloud).await.unwrap();
    jobs::process(pool, id, &cloud).await.unwrap();
    assert_eq!(jobs::load(pool, id).await.unwrap().status, expected);
    assert_eq!(mock.requests()[3].params["InstanceId"], "i-guardtest");
    assert_eq!(mock.requests()[3].params["InitLocalDisk"], "false");
    mock.exhausted();
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn single_start_success_uses_minimum_permission_api_and_reads_back(pool: PgPool) {
    single_start(&pool, "200", "succeeded").await;
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn single_start_rejection_is_terminal(pool: PgPool) {
    single_start(&pool, "OperationDenied.NoStock", "failed").await;
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn single_start_unknown_result_never_replays_write(pool: PgPool) {
    single_start(&pool, "UnknownResult", "uncertain").await;
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn ordinary_instance_can_restart_below_limit_but_manual_hold_and_pause_win(pool: PgPool) {
    let (_, r) = seed(&pool).await;
    let mock = Mock::start(vec![power("Stopped"), traffic(10 * GIB)]).await;
    let cloud = Cloud::local(&mock.endpoint);
    scheduler::poll(&pool, r.id, &cloud).await.unwrap();
    let q = queued(&pool, r.id).await;
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].action, "start");
    assert!(!q[0].before_state.spot());
    sqlx::query("UPDATE alicloud_resources SET manual_hold=true,next_power_at=0 WHERE id=$1")
        .bind(r.id)
        .execute(&pool)
        .await
        .unwrap();
    jobs::process(&pool, q[0].id, &cloud).await.unwrap();
    assert_eq!(
        jobs::load(&pool, q[0].id).await.unwrap().status,
        "cancelled"
    );
    mock.exhausted();
    let mock = Mock::start(vec![power("Stopped"), traffic(11 * GIB)]).await;
    scheduler::poll(&pool, r.id, &Cloud::local(&mock.endpoint))
        .await
        .unwrap();
    assert!(queued(&pool, r.id).await.is_empty());
    mock.exhausted();
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn invalid_meter_cancels_start_and_retains_history(pool: PgPool) {
    let (a, r) = seed(&pool).await;
    let mock = Mock::start(vec![power("Stopped"), traffic(10 * GIB)]).await;
    scheduler::poll(&pool, r.id, &Cloud::local(&mock.endpoint))
        .await
        .unwrap();
    let id = queued(&pool, r.id).await[0].id;
    let mut partial = traffic(1);
    partial.value["NextToken"] = "more".into();
    let mock = Mock::start(vec![power("Stopped"), partial]).await;
    sqlx::query("UPDATE alicloud_resources SET next_power_at=0 WHERE id=$1")
        .bind(r.id)
        .execute(&pool)
        .await
        .unwrap();
    scheduler::poll(&pool, r.id, &Cloud::local(&mock.endpoint))
        .await
        .unwrap();
    assert_eq!(jobs::load(&pool, id).await.unwrap().status, "cancelled");
    assert!(queued(&pool, r.id).await.is_empty());
    let r = resource(&pool, r.id).await.unwrap();
    assert_eq!(
        r.cdt_guard_state.as_ref().unwrap().max_usage_bytes,
        10 * GIB
    );
    assert_eq!(exceeded(&a, &r, apsp_protocol::now_timestamp()), None);
    assert!(
        r.power_policy
            .blocks_start(&a, &r, apsp_protocol::now_timestamp())
    );
    mock.exhausted();
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn zero_uses_no_extra_permission_and_cannot_clear_exhausted_history(pool: PgPool) {
    let (a, r) = seed(&pool).await;
    save_observation(
        &pool,
        r.id,
        &observation(&a, &r, 198 * GIB, 71 * GIB, apsp_protocol::now_timestamp()),
    )
    .await;
    let mock = Mock::start(vec![power("Stopped"), traffic(0)]).await;
    scheduler::poll(&pool, r.id, &Cloud::local(&mock.endpoint))
        .await
        .unwrap();
    assert!(queued(&pool, r.id).await.is_empty());
    let r = resource(&pool, r.id).await.unwrap();
    assert_eq!(
        r.cdt_guard_state.as_ref().unwrap().max_usage_bytes,
        198 * GIB
    );
    assert_eq!(exceeded(&a, &r, apsp_protocol::now_timestamp()), Some(true));
    assert!(r.cdt_guard_error.is_none());
    mock.exhausted();
    let mock = Mock::start(vec![traffic(0)]).await;
    assert_eq!(
        read(
            &a,
            &Resource {
                cdt_guard_state: None,
                ..r
            },
            &Cloud::local(&mock.endpoint),
            apsp_protocol::now_timestamp()
        )
        .await
        .unwrap_err()
        .code,
        "cdt_not_enabled"
    );
    mock.exhausted();
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn import_is_monotonic_paused_and_never_fresh_authorization(pool: PgPool) {
    let (a, r) = seed(&pool).await;
    let now = apsp_protocol::now_timestamp();
    let make_input = |revision, usage: u64| seed::Input {
        revision,
        state: seed::LegacyState {
            version: 1,
            identity: seed::Identity {
                instance_id: r.cloud_id.clone(),
                region_id: r.region.clone(),
                quota_pool: "non-mainland".into(),
            },
            billing_cycle: billing::month(now),
            max_usage_bytes: (usage * GIB).to_string(),
            max_jump_bytes: (20 * GIB).to_string(),
            observed_at: now,
        },
    };
    assert!(
        seed::import_on(&pool, r.id, make_input(1, 100))
            .await
            .is_err()
    );
    sqlx::query("UPDATE alicloud_resources SET power_policy=jsonb_set(power_policy,'{enabled}','false') WHERE id=$1").bind(r.id).execute(&pool).await.unwrap();
    seed::import_on(&pool, r.id, make_input(1, 100))
        .await
        .unwrap();
    seed::import_on(&pool, r.id, make_input(2, 80))
        .await
        .unwrap();
    let mut r = resource(&pool, r.id).await.unwrap();
    assert_eq!(
        r.cdt_guard_state.as_ref().unwrap().max_usage_bytes,
        100 * GIB
    );
    r.power_policy.enabled = true;
    assert!(r.power_policy.blocks_start(&a, &r, now));
    assert!(queued(&pool, r.id).await.is_empty());
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn confirmed_cdt_accepts_zero_month_reset_without_additional_permissions(pool: PgPool) {
    let (a, r) = seed(&pool).await;
    let now = apsp_protocol::now_timestamp();
    let mut previous = observation(&a, &r, 200 * GIB, 20 * GIB, now - 60);
    previous.billing_cycle = "2020-01".into();
    // Older persisted state lacks the new field but has a positive live reading.
    let mut legacy = serde_json::to_value(&previous).unwrap();
    legacy.as_object_mut().unwrap().remove("cdt_observed");
    previous = serde_json::from_value(legacy).unwrap();
    save_observation(&pool, r.id, &previous).await;
    let mock = Mock::start(vec![power("Stopped"), traffic(0)]).await;
    scheduler::poll(&pool, r.id, &Cloud::local(&mock.endpoint))
        .await
        .unwrap();
    let saved = resource(&pool, r.id).await.unwrap();
    let reset = saved.cdt_guard_state.as_ref().unwrap();
    assert_eq!(reset.max_usage_bytes, 0);
    assert!(reset.cdt_observed);
    assert_eq!(queued(&pool, r.id).await[0].action, "start");
    let repeated = observe(Some(reset), 0, &a, &saved, &key_digest(KEY), now + 1).unwrap();
    assert!(repeated.cdt_observed);
    // A restored controller retains proof without needing a product-list query.
    let restored: GuardState =
        serde_json::from_str(&serde_json::to_string(&repeated).unwrap()).unwrap();
    assert!(observe(Some(&restored), 0, &a, &saved, &key_digest(KEY), now + 2).is_ok());
    assert!(observe(Some(&restored), 0, &a, &saved, "changed-key", now + 2).is_err());
    let mut imported = previous;
    imported.queried_at = 0;
    assert_eq!(
        observe(Some(&imported), 0, &a, &saved, &key_digest(KEY), now + 2)
            .unwrap_err()
            .code,
        "cdt_not_enabled"
    );
    mock.exhausted();
}

#[test]
fn empty_or_missing_pool_is_not_a_zero_counter() {
    // A complete empty listing is reported as "no reading", never as zero.
    assert_eq!(
        meter::traffic(&json!({"TrafficDetails":[]}), "cn-hongkong").unwrap(),
        None
    );
    assert_eq!(
        meter::traffic(&json!({"TrafficDetails":[],"TotalCount":0}), "cn-hongkong").unwrap(),
        None
    );
    for incomplete in [
        json!({"TrafficDetails":[],"TotalCount":2}),
        json!({"TrafficDetails":[],"NextToken":"more"}),
        json!({}),
    ] {
        assert!(meter::traffic(&incomplete, "cn-hongkong").is_err());
    }
    assert!(
        meter::traffic(
            &json!({"TrafficDetails":[row("cn-hangzhou", 0)]}),
            "cn-hongkong"
        )
        .is_err()
    );
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn empty_listing_in_a_new_cycle_restarts_a_guard_stopped_instance(pool: PgPool) {
    // 2026-11-01: the guard stopped the instance at its threshold in October;
    // November's listing stays empty while it is off, so it must still start.
    let (a, r) = seed(&pool).await;
    let now = apsp_protocol::now_timestamp();
    let mut previous = observation(&a, &r, 151 * GIB, 0, now - 60);
    previous.billing_cycle = "2020-01".into();
    save_observation(&pool, r.id, &previous).await;
    let empty = Reply::ok(
        "ListCdtInternetTraffic",
        json!({"RequestId":"meter","TrafficDetails":[],"TotalCount":0}),
    );
    let mock = Mock::start(vec![power("Stopped"), empty]).await;
    scheduler::poll(&pool, r.id, &Cloud::local(&mock.endpoint))
        .await
        .unwrap();
    let saved = resource(&pool, r.id).await.unwrap();
    assert!(saved.cdt_guard_error.is_none());
    let reset = saved.cdt_guard_state.as_ref().unwrap();
    assert_eq!(reset.max_usage_bytes, 0);
    assert_eq!(reset.billing_cycle, billing::month(now));
    assert_eq!(queued(&pool, r.id).await[0].action, "start");
    // Still empty a minute later in the same cycle: accepted while nothing was metered.
    assert!(observe_empty(Some(reset), &a, &saved, &key_digest(KEY), now + 60).is_ok());
    mock.exhausted();
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn empty_listing_never_replaces_evidence(pool: PgPool) {
    let (a, r) = seed(&pool).await;
    let now = apsp_protocol::now_timestamp();
    let digest = key_digest(KEY);
    let reject = |result: Result<GuardState, Failure>| result.unwrap_err().code;
    // No history at all.
    assert_eq!(
        reject(observe_empty(None, &a, &r, &digest, now)),
        "invalid_response"
    );
    // Usage already seen this cycle: an empty listing is a broken reading.
    let current = observation(&a, &r, 5 * GIB, 0, now - 60);
    assert_eq!(
        reject(observe_empty(Some(&current), &a, &r, &digest, now)),
        "invalid_response"
    );
    // Imported history without a live reading proves nothing.
    let mut imported = current.clone();
    imported.billing_cycle = "2020-01".into();
    imported.queried_at = 0;
    assert_eq!(
        reject(observe_empty(Some(&imported), &a, &r, &digest, now)),
        "invalid_response"
    );
    // A confirmed previous cycle still has to match the account key and instance.
    let mut previous = current;
    previous.billing_cycle = "2020-01".into();
    assert!(observe_empty(Some(&previous), &a, &r, &digest, now).is_ok());
    assert_eq!(
        reject(observe_empty(Some(&previous), &a, &r, "changed-key", now)),
        "cdt_state_mismatch"
    );
    // A running instance with an empty listing in a confirmed new cycle is not stopped.
    save_observation(&pool, r.id, &previous).await;
    let empty = Reply::ok(
        "ListCdtInternetTraffic",
        json!({"RequestId":"meter","TrafficDetails":[]}),
    );
    let mock = Mock::start(vec![power("Running"), empty]).await;
    scheduler::poll(&pool, r.id, &Cloud::local(&mock.endpoint))
        .await
        .unwrap();
    assert!(queued(&pool, r.id).await.is_empty());
    mock.exhausted();
}
