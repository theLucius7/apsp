#![forbid(unsafe_code)]

use anyhow::Result;
use rand::{Rng, SeedableRng, rngs::StdRng};
use sqlx::{PgPool, migrate::Migrator};
use std::borrow::Cow;
use uuid::Uuid;

const BASE: i64 = 1_790_812_800; // A UTC midnight.

async fn identities(pool: &PgPool) -> Result<(i64, Vec<i64>, Vec<i64>)> {
    let server: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('TEST_ONLY rollup') RETURNING id")
            .fetch_one(pool)
            .await?;
    let mut users = Vec::new();
    let mut nodes = Vec::new();
    for index in 0..3 {
        users.push(
            sqlx::query_scalar(
                "INSERT INTO users(name,subscription_token) VALUES($1,$2) RETURNING id",
            )
            .bind(format!("TEST_ONLY user {index}"))
            .bind(format!("TEST_ONLY-token-{index}"))
            .fetch_one(pool)
            .await?,
        );
        nodes.push(sqlx::query_scalar("INSERT INTO nodes(name,server_id,port,public_host,sni,private_key,public_key,short_id) VALUES($1,$2,$3,'proxy.example.com','www.example.com','TEST_ONLY-private','TEST_ONLY-public','01') RETURNING id")
            .bind(format!("TEST_ONLY node {index}")).bind(server).bind(21000 + index).fetch_one(pool).await?);
    }
    Ok((server, users, nodes))
}

/// Inserts one ledger record as (server, epoch, seq) / (user, node) / (uplink, downlink).
async fn record(
    pool: &PgPool,
    (server, epoch, seq): (i64, Uuid, i64),
    (user, node): (i64, i64),
    end: i64,
    (up, down): (u64, u64),
) -> Result<u64> {
    sqlx::query("INSERT INTO usage_batches(server_id,epoch,seq,payload_hash) VALUES($1,$2,$3,'TEST_ONLY') ON CONFLICT DO NOTHING")
        .bind(server).bind(epoch).bind(seq).execute(pool).await?;
    Ok(sqlx::query("INSERT INTO usage_records(server_id,epoch,seq,stat_name,user_id,node_id,uplink,downlink,period_start,period_end) VALUES($1,$2,$3,$4,$5,$6,$7::text::numeric,$8::text::numeric,$9-30,$9) ON CONFLICT DO NOTHING")
        .bind(server).bind(epoch).bind(seq).bind(format!("u{user}_n{node}")).bind(user).bind(node)
        .bind(up.to_string()).bind(down.to_string()).bind(end).execute(pool).await?.rows_affected())
}

async fn ledger_window(pool: &PgPool, user: i64, after: i64, until: i64) -> Result<String> {
    Ok(sqlx::query_scalar("SELECT COALESCE(SUM(uplink+downlink),0)::text FROM usage_records WHERE user_id=$1 AND period_end>$2 AND period_end<=$3")
        .bind(user).bind(after).bind(until).fetch_one(pool).await?)
}

/// user, day, node, uplink, downlink, records, last period end.
type DailyRow = (i64, i64, i64, String, String, i64, i64);

async fn rollup_snapshot(pool: &PgPool) -> Result<Vec<DailyRow>> {
    Ok(sqlx::query_as("SELECT user_id,day,node_id,uplink::text,downlink::text,records,last_period_end FROM singbox_usage_daily ORDER BY user_id,day,node_id")
        .fetch_all(pool).await?)
}

#[sqlx::test]
async fn daily_rollup_and_cycle_windows_match_the_ledger(pool: PgPool) -> Result<()> {
    let (server, users, nodes) = identities(&pool).await?;
    let mut random = StdRng::seed_from_u64(77);
    let epoch = Uuid::new_v4();
    // Records cluster around day boundaries as well as inside days, spanning 40 days.
    let mut ends = vec![
        BASE,
        BASE - 1,
        BASE + 1,
        BASE + 86_399,
        BASE + 86_400,
        BASE + 86_401,
    ];
    for _ in 0..400 {
        let day = random.gen_range(-5..35_i64);
        let offset = if random.gen_bool(0.3) {
            [0, 1, 86_399][random.gen_range(0..3)]
        } else {
            random.gen_range(0..86_400)
        };
        ends.push(BASE + day * 86_400 + offset);
    }
    for (seq, end) in ends.iter().enumerate() {
        let user = users[random.gen_range(0..users.len())];
        let node = nodes[random.gen_range(0..nodes.len())];
        let (up, down) = if seq == 0 {
            (u64::MAX, u64::MAX)
        } else {
            (
                random.gen_range(0..1_000_000),
                random.gen_range(0..1_000_000),
            )
        };
        assert_eq!(
            record(
                &pool,
                (server, epoch, seq as i64),
                (user, node),
                *end,
                (up, down)
            )
            .await?,
            1
        );
        // A replayed record is skipped by the ledger and must not be counted twice.
        if seq % 7 == 0 {
            assert_eq!(
                record(
                    &pool,
                    (server, epoch, seq as i64),
                    (user, node),
                    *end,
                    (up, down)
                )
                .await?,
                0
            );
        }
    }
    // Per-identity rollup totals equal the ledger.
    let mismatched: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT user_id,node_id,period_end/86400*86400 AS day,SUM(uplink) AS up,SUM(downlink) AS down,COUNT(*) AS n,MAX(period_end) AS last FROM usage_records GROUP BY 1,2,3
         ) l FULL JOIN singbox_usage_daily d ON d.user_id=l.user_id AND d.node_id=l.node_id AND d.day=l.day
         WHERE d.uplink IS DISTINCT FROM l.up OR d.downlink IS DISTINCT FROM l.down
            OR d.records IS DISTINCT FROM l.n OR d.last_period_end IS DISTINCT FROM l.last",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(mismatched, 0);
    // Arbitrary cycle windows, including day-aligned, empty, inverted and negative bounds.
    let mut bounds = vec![
        (BASE - 1, BASE),
        (BASE - 1, BASE + 86_399),
        (BASE, BASE + 86_400),
        (BASE - 1, BASE + 86_400),
        (BASE + 1, BASE + 86_400 - 1),
        (BASE + 86_400, BASE + 86_400),
        (BASE + 86_400, BASE),
        (-1, BASE + 3 * 86_400),
        (BASE - 6 * 86_400, BASE + 40 * 86_400),
    ];
    for _ in 0..200 {
        let after = BASE + random.gen_range(-6 * 86_400..36 * 86_400);
        let until = after + random.gen_range(0..40 * 86_400);
        bounds.push((after, until));
    }
    for (after, until) in bounds {
        for user in &users {
            let window: String = sqlx::query_scalar("SELECT singbox_usage_window($1,$2,$3)::text")
                .bind(user)
                .bind(after)
                .bind(until)
                .fetch_one(&pool)
                .await?;
            assert_eq!(
                window,
                ledger_window(&pool, *user, after, until).await?,
                "user {user} window ({after}, {until}]"
            );
        }
    }
    // The repair function reproduces exactly what the trigger maintained.
    let maintained = rollup_snapshot(&pool).await?;
    let rebuilt: i64 = sqlx::query_scalar("SELECT singbox_usage_daily_rebuild()")
        .fetch_one(&pool)
        .await?;
    assert_eq!(rebuilt as usize, maintained.len());
    assert_eq!(rollup_snapshot(&pool).await?, maintained);
    Ok(())
}

#[sqlx::test]
async fn entitlement_usage_keeps_the_original_cycle_boundaries(pool: PgPool) -> Result<()> {
    let (server, users, nodes) = identities(&pool).await?;
    let user = users[0];
    let at = apsp_protocol::now_timestamp();
    // A +05:45 zone puts both cycle boundaries inside UTC days.
    let zone = "Asia/Kathmandu";
    let group: i64 = sqlx::query_scalar("INSERT INTO singbox_package_groups(name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,duration_days) VALUES('TEST_ONLY package',1110,1,0,0,$1,3650) RETURNING id")
        .bind(zone).fetch_one(&pool).await?;
    let assignment: i64 = sqlx::query_scalar("INSERT INTO singbox_package_assignments(user_id,request_id,package_group_id,package_name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,starts_at,expires_at) VALUES($1,$2,$3,'TEST_ONLY package',1110,1,0,0,$4,$5,$6) RETURNING id")
        .bind(user).bind(Uuid::new_v4()).bind(group).bind(zone).bind(at - 400 * 86_400).bind(at + 400 * 86_400).fetch_one(&pool).await?;
    sqlx::query("INSERT INTO singbox_user_packages(user_id,assignment_id) VALUES($1,$2)")
        .bind(user)
        .bind(assignment)
        .execute(&pool)
        .await?;
    let (start, next): (i64, i64) =
        sqlx::query_as("SELECT cycle_start,next_reset FROM singbox_cycle_bounds($1,1,0,0,$2)")
            .bind(at)
            .bind(zone)
            .fetch_one(&pool)
            .await?;
    assert_ne!(start % 86_400, 0);
    let epoch = Uuid::new_v4();
    for (seq, (end, amount)) in [
        (start, 1_u64),
        (start + 1, 10),
        (start + 2 * 86_400, 100),
        (next, 1_000),
        (next + 1, 10_000),
    ]
    .into_iter()
    .enumerate()
    {
        record(
            &pool,
            (server, epoch, seq as i64),
            (user, nodes[0]),
            end,
            (amount, 0),
        )
        .await?;
    }
    let (used, status): (String, String) =
        sqlx::query_as("SELECT used_bytes,status FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(at)
            .bind(user)
            .fetch_one(&pool)
            .await?;
    assert_eq!(used, "1110");
    assert_eq!(used, ledger_window(&pool, user, start, next).await?);
    assert_eq!(status, "exhausted");
    // Users without a package keep the unmetered zero result.
    let (used, status): (String, String) =
        sqlx::query_as("SELECT used_bytes,status FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(at)
            .bind(users[1])
            .fetch_one(&pool)
            .await?;
    assert_eq!((used.as_str(), status.as_str()), ("0", "unmetered"));
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn rollup_upgrade_preserves_existing_quota_credits_and_late_charges(
    pool: PgPool,
) -> Result<()> {
    let migrations = sqlx::migrate!();
    let prior = Migrator {
        migrations: Cow::Owned(
            migrations
                .iter()
                .filter(|m| m.version <= 58)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    prior.run(&pool).await?;
    let (server, users, nodes) = identities(&pool).await?;
    let user = users[0];
    let at = BASE + 15 * 86_400;
    let group: i64 = sqlx::query_scalar("INSERT INTO singbox_package_groups(name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,duration_days) VALUES('TEST_ONLY credited upgrade',1000,1,0,0,'UTC',3650) RETURNING id")
        .fetch_one(&pool).await?;
    let assignment: i64 = sqlx::query_scalar("INSERT INTO singbox_package_assignments(user_id,request_id,package_group_id,package_name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,starts_at,expires_at) VALUES($1,$2,$3,'TEST_ONLY credited upgrade',1000,1,0,0,'UTC',$4,$5) RETURNING id")
        .bind(user).bind(Uuid::new_v4()).bind(group).bind(at - 400 * 86_400).bind(at + 400 * 86_400).fetch_one(&pool).await?;
    sqlx::query("INSERT INTO singbox_user_packages(user_id,assignment_id) VALUES($1,$2)")
        .bind(user)
        .bind(assignment)
        .execute(&pool)
        .await?;
    let (start, next): (i64, i64) =
        sqlx::query_as("SELECT cycle_start,next_reset FROM singbox_cycle_bounds($1,1,0,0,'UTC')")
            .bind(at)
            .fetch_one(&pool)
            .await?;
    let epoch = Uuid::new_v4();
    record(
        &pool,
        (server, epoch, 1),
        (user, nodes[0]),
        start + 60,
        (800, 500),
    )
    .await?;
    sqlx::query("INSERT INTO singbox_quota_credits(id,user_id,cycle_start,next_reset,bytes,created_at,administrator_id) VALUES($1,$2,$3,$4,400,$5,1)")
        .bind(Uuid::new_v4()).bind(user).bind(start).bind(next).bind(at).execute(&pool).await?;
    let prior_entitlement: (String, String, bool) = sqlx::query_as(
        "SELECT used_bytes,status,allowed FROM singbox_entitlements($1) WHERE user_id=$2",
    )
    .bind(at)
    .bind(user)
    .fetch_one(&pool)
    .await?;
    assert_eq!(prior_entitlement, ("900".into(), "active".into(), true));
    migrations.run(&pool).await?;
    let upgraded: (String, String, bool) = sqlx::query_as(
        "SELECT used_bytes,status,allowed FROM singbox_entitlements($1) WHERE user_id=$2",
    )
    .bind(at)
    .bind(user)
    .fetch_one(&pool)
    .await?;
    assert_eq!(upgraded, prior_entitlement);
    assert_eq!(
        record(
            &pool,
            (server, epoch, 2),
            (user, nodes[0]),
            start + 120,
            (160, 40)
        )
        .await?,
        1
    );
    assert_eq!(
        record(
            &pool,
            (server, epoch, 2),
            (user, nodes[0]),
            start + 120,
            (160, 40)
        )
        .await?,
        0
    );
    let late: (String, String, bool) = sqlx::query_as(
        "SELECT used_bytes,status,allowed FROM singbox_entitlements($1) WHERE user_id=$2",
    )
    .bind(at)
    .bind(user)
    .fetch_one(&pool)
    .await?;
    assert_eq!(late, ("1100".into(), "exhausted".into(), false));
    assert_eq!(ledger_window(&pool, user, start, next).await?, "1500");
    sqlx::query("INSERT INTO singbox_quota_credits(id,user_id,cycle_start,next_reset,bytes,created_at,administrator_id) VALUES($1,$2,$3,$4,2000,$5,1)")
        .bind(Uuid::new_v4()).bind(user).bind(start).bind(next).bind(at).execute(&pool).await?;
    let reset: (String, String, bool) = sqlx::query_as(
        "SELECT used_bytes,status,allowed FROM singbox_entitlements($1) WHERE user_id=$2",
    )
    .bind(at)
    .bind(user)
    .fetch_one(&pool)
    .await?;
    assert_eq!(reset, ("0".into(), "active".into(), true));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM usage_records WHERE user_id=$1")
            .bind(user)
            .fetch_one(&pool)
            .await?,
        2
    );
    Ok(())
}
