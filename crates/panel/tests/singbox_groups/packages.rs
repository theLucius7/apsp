use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

#[sqlx::test(migrations = "./migrations")]
async fn calendar_month_end_leap_year_timezone_and_exact_boundary(pool: PgPool) -> Result<()> {
    for (at, day, hour, minute, zone, start, next) in [
        (
            "2026-02-28 00:00:00+00",
            31,
            0,
            0,
            "UTC",
            "2026-02-28 00:00:00+00",
            "2026-03-31 00:00:00+00",
        ),
        (
            "2024-02-29 00:00:00+00",
            31,
            0,
            0,
            "UTC",
            "2024-02-29 00:00:00+00",
            "2024-03-31 00:00:00+00",
        ),
        (
            "2026-03-30 23:59:59+00",
            31,
            0,
            0,
            "UTC",
            "2026-02-28 00:00:00+00",
            "2026-03-31 00:00:00+00",
        ),
        (
            "2026-12-31 23:59:59+00",
            1,
            0,
            0,
            "UTC",
            "2026-12-01 00:00:00+00",
            "2027-01-01 00:00:00+00",
        ),
        (
            "2026-09-30 16:00:00+00",
            1,
            0,
            0,
            "Asia/Taipei",
            "2026-09-30 16:00:00+00",
            "2026-10-31 16:00:00+00",
        ),
        (
            "2026-10-05 04:29:59+00",
            5,
            12,
            30,
            "Asia/Taipei",
            "2026-09-05 04:30:00+00",
            "2026-10-05 04:30:00+00",
        ),
        (
            "2026-03-08 07:30:00+00",
            8,
            2,
            30,
            "America/New_York",
            "2026-03-08 07:30:00+00",
            "2026-04-08 06:30:00+00",
        ),
    ] {
        let (actual_start, actual_next): (i64, i64) = sqlx::query_as(
            "SELECT cycle_start,next_reset FROM singbox_cycle_bounds($1,$2,$3,$4,$5)",
        )
        .bind(stamp(&pool, at).await?)
        .bind(day)
        .bind(hour)
        .bind(minute)
        .bind(zone)
        .fetch_one(&pool)
        .await?;
        assert_eq!(actual_start, stamp(&pool, start).await?, "{at} {zone}");
        assert_eq!(actual_next, stamp(&pool, next).await?, "{at} {zone}");
    }
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn assignment_snapshots_idempotency_and_full_precision_quota(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = id(&panel.create_user(&cookie, "Member").await?)?;
    let plan = create_plan(&panel, &cookie, Some("18446744073709551615")).await?;
    let request = Uuid::new_v4();
    let first = assign(&panel, &cookie, user, plan, request).await?;
    assert_eq!(
        first["expires_at"].as_i64().unwrap() - first["starts_at"].as_i64().unwrap(),
        365 * 86400
    );
    assert_eq!(assign(&panel, &cookie, user, plan, request).await?, first);
    call(
        &panel,
        &cookie,
        Method::PUT,
        &format!("/package-groups/{plan}"),
        Some(plan_body(Some("10"))),
    )
    .await?;
    let before = entitlement(&panel, &cookie, user).await?;
    assert_eq!(before["monthly_bytes"], "18446744073709551615");
    let second = assign(&panel, &cookie, user, plan, Uuid::new_v4()).await?;
    assert_ne!(second["id"], first["id"]);
    assert_eq!(
        entitlement(&panel, &cookie, user).await?["monthly_bytes"],
        "10"
    );
    // An old retry must not overwrite the newer current assignment.
    assign(&panel, &cookie, user, plan, request).await?;
    assert_eq!(
        entitlement(&panel, &cookie, user).await?["monthly_bytes"],
        "10"
    );
    let another = create_plan(&panel, &cookie, None).await?;
    let conflict = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/users/{user}/package"),
            &cookie,
            Some(json!({"package_group_id":another,"request_id":request})),
        )
        .await?;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/package-groups/{plan}"),
        None,
    )
    .await?;
    assert_eq!(
        entitlement(&panel, &cookie, user).await?["monthly_bytes"],
        "10"
    );
    let retry = assign(&panel, &cookie, user, plan, request).await?;
    assert_eq!(retry, first);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn quota_blocks_subscription_and_runtime_without_erasing_history(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Server").await?;
    let node = id(&panel.create_node(&cookie, server, "Node").await?)?;
    let user = panel.create_user(&cookie, "Member").await?;
    let uid = id(&user)?;
    panel.grant(&cookie, uid, node).await?;
    let plan = create_plan(&panel, &cookie, Some("100")).await?;
    assign(&panel, &cookie, uid, plan, Uuid::new_v4()).await?;
    entitlements::refresh(&pool, now_timestamp()).await?;
    panel.publish_now().await?;
    applied(&pool).await?;
    let sub = user["subscription_url"].as_str().unwrap();
    assert!(
        !panel
            .client
            .get(sub)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?
            .is_empty()
    );
    let batch = usage(uid, node, now_timestamp(), 40, 60);
    apsp_panel::usage::ingest(&panel.state, server, batch.clone()).await?;
    apsp_panel::usage::ingest(&panel.state, server, batch).await?;
    assert_eq!(
        entitlement(&panel, &cookie, uid).await?["used_bytes"],
        "100"
    );
    assert_eq!(
        entitlement(&panel, &cookie, uid).await?["status"],
        "exhausted"
    );
    // An exhausted entitlement is refused with the same reason in every format; the default
    // links format used to answer 200 with an empty body, which clients take for "no nodes".
    let mut reasons = Vec::new();
    for url in [sub.to_owned(), format!("{sub}?format=singbox")] {
        let response = panel.client.get(&url).send().await?;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{url}");
        assert!(response.headers().get("subscription-userinfo").is_none());
        reasons.push(response.json::<Value>().await?["error"].clone());
    }
    assert!(
        reasons[0].as_str().unwrap().contains("流量已用尽"),
        "{reasons:?}"
    );
    assert_eq!(reasons[0], reasons[1]);
    entitlements::refresh(&pool, now_timestamp()).await?;
    let dirty: Option<i64> = sqlx::query_scalar("SELECT dirty_at FROM servers WHERE id=$1")
        .bind(server)
        .fetch_one(&pool)
        .await?;
    assert!(dirty.is_some());
    entitlements::refresh(&pool, now_timestamp()).await?;
    let still_dirty: Option<i64> = sqlx::query_scalar("SELECT dirty_at FROM servers WHERE id=$1")
        .bind(server)
        .fetch_one(&pool)
        .await?;
    assert_eq!(dirty, still_dirty);
    panel.publish_now().await?;
    assert!(
        latest_config(&pool, server).await?["inbounds"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Reassignment cannot reset usage, and a fresh runtime epoch is not a billing cycle.
    assign(&panel, &cookie, uid, plan, Uuid::new_v4()).await?;
    apsp_panel::usage::ingest(
        &panel.state,
        server,
        usage(uid, node, now_timestamp(), 1, 1),
    )
    .await?;
    assert_eq!(
        entitlement(&panel, &cookie, uid).await?["used_bytes"],
        "102"
    );
    let e = entitlement(&panel, &cookie, uid).await?;
    let next = e["next_reset"].as_i64().unwrap();
    assert_eq!(eligible(&pool, uid, next).await?, vec![node]);
    entitlements::refresh(&pool, next).await?;
    // A late replay from the old month does not consume the new month's quota.
    apsp_panel::usage::ingest(&panel.state, server, usage(uid, node, next, 1000, 1000)).await?;
    let new_usage: String =
        sqlx::query_scalar("SELECT used_bytes FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(next)
            .bind(uid)
            .fetch_one(&pool)
            .await?;
    assert_eq!(new_usage, "0");
    let expiry = e["expires_at"].as_i64().unwrap();
    assert!(eligible(&pool, uid, expiry).await?.is_empty());
    let records: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_records")
        .fetch_one(&pool)
        .await?;
    assert_eq!(records, 3);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn quota_is_shared_across_nodes_and_terminal_samples_remain_chargeable(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let a = panel.create_server(&cookie, "A").await?;
    let b = panel.create_server(&cookie, "B").await?;
    let na = id(&panel.create_node(&cookie, a, "A").await?)?;
    let nb = id(&panel.create_node(&cookie, b, "B").await?)?;
    let user = id(&panel.create_user(&cookie, "Member").await?)?;
    panel.grant(&cookie, user, na).await?;
    panel.grant(&cookie, user, nb).await?;
    let plan = create_plan(&panel, &cookie, Some("200")).await?;
    assign(&panel, &cookie, user, plan, Uuid::new_v4()).await?;
    panel.publish_now().await?;
    let (x, y) = tokio::join!(
        apsp_panel::usage::ingest(&panel.state, a, usage(user, na, now_timestamp(), 50, 50)),
        apsp_panel::usage::ingest(&panel.state, b, usage(user, nb, now_timestamp(), 49, 50))
    );
    x?;
    y?;
    assert_eq!(
        entitlement(&panel, &cookie, user).await?["used_bytes"],
        "199"
    );
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/users/{user}/accesses/{na}"),
        None,
    )
    .await?;
    apsp_panel::usage::ingest(&panel.state, a, usage(user, na, now_timestamp(), 0, 1)).await?;
    assert_eq!(
        entitlement(&panel, &cookie, user).await?["status"],
        "exhausted"
    );
    assert!(eligible(&pool, user, now_timestamp()).await?.is_empty());
    Ok(())
}

fn header(response: &reqwest::Response, name: &str) -> Result<String> {
    Ok(response
        .headers()
        .get(name)
        .with_context(|| format!("missing {name} header"))?
        .to_str()?
        .to_owned())
}

#[sqlx::test(migrations = "./migrations")]
async fn restricted_users_get_the_same_refusal_in_every_format_and_an_audit_result(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Restricted").await?;
    let node = id(&panel.create_node(&cookie, server, "Node").await?)?;
    let user = panel.create_user(&cookie, "Member").await?;
    let uid = id(&user)?;
    panel.grant(&cookie, uid, node).await?;
    let plan = create_plan(&panel, &cookie, Some("1000")).await?;
    assign(&panel, &cookie, uid, plan, Uuid::new_v4()).await?;
    panel.publish_now().await?;
    applied(&pool).await?;
    let sub = user["subscription_url"].as_str().unwrap();
    let urls = [sub.to_owned(), format!("{sub}?format=singbox")];
    // An allowed user is served in both formats.
    for url in &urls {
        let response = panel.client.get(url).send().await?;
        assert_eq!(response.status(), StatusCode::OK, "{url}");
        assert!(!response.text().await?.is_empty(), "{url}");
    }
    sqlx::query(
        "UPDATE singbox_package_assignments SET starts_at=$2-172800,expires_at=$2-86400 WHERE user_id=$1",
    )
    .bind(uid)
    .bind(now_timestamp())
    .execute(&pool)
    .await?;
    let mut reasons = Vec::new();
    for url in &urls {
        let response = panel.client.get(url).send().await?;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{url}");
        assert!(response.headers().get("subscription-userinfo").is_none());
        reasons.push(response.json::<Value>().await?["error"].clone());
    }
    assert!(reasons[0].as_str().unwrap().contains("到期"), "{reasons:?}");
    assert_eq!(reasons[0], reasons[1]);
    let results: Vec<String> = sqlx::query_scalar(
        "SELECT detail->>'result' FROM singbox_operation_events WHERE action='subscription_fetch' AND user_id=$1 ORDER BY id",
    )
    .bind(uid)
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        results,
        ["generated", "generated", "restricted", "restricted"]
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn subscription_headers_report_cycle_usage_quota_expiry_and_profile(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Headers").await?;
    let node = id(&panel.create_node(&cookie, server, "Node").await?)?;
    let limited = panel.create_user(&cookie, "限量成员").await?;
    let free = panel.create_user(&cookie, "Unmetered").await?;
    let (limited_id, free_id) = (id(&limited)?, id(&free)?);
    panel.grant(&cookie, limited_id, node).await?;
    panel.grant(&cookie, free_id, node).await?;
    let plan = create_plan(&panel, &cookie, Some("1000")).await?;
    assign(&panel, &cookie, limited_id, plan, Uuid::new_v4()).await?;
    panel.publish_now().await?;
    applied(&pool).await?;
    let cycle = entitlement(&panel, &cookie, limited_id).await?;
    let (start, next, expires) = (
        cycle["cycle_start"].as_i64().unwrap(),
        cycle["next_reset"].as_i64().unwrap(),
        cycle["expires_at"].as_i64().unwrap(),
    );
    // One batch on another day of the cycle (read from the daily rollup) and one now (read
    // from the ledger); the free user has no cycle.
    let batches = [
        (limited_id, start + 86400 + 100, 10, 20),
        (limited_id, now_timestamp(), 30, 40),
        (free_id, now_timestamp(), 5, 7),
    ];
    for (user, end, up, down) in batches {
        apsp_panel::usage::ingest(&panel.state, server, usage(user, node, end, up, down)).await?;
    }
    let title = format!("base64:{}", STANDARD.encode("限量成员"));
    for format in ["links", "singbox"] {
        let url = format!(
            "{}?format={format}",
            limited["subscription_url"].as_str().unwrap()
        );
        let response = panel.client.get(url).send().await?.error_for_status()?;
        assert_eq!(
            header(&response, "subscription-userinfo")?,
            format!("upload=40; download=60; total=1000; expire={expires}"),
            "{format}"
        );
        assert_eq!(header(&response, "profile-update-interval")?, "24");
        assert_eq!(header(&response, "profile-title")?, title);
    }
    // A manual quota reset credits part of the ledger: clients see the usage that counts, so
    // upload and download shrink proportionally and still add up to it.
    sqlx::query("INSERT INTO singbox_quota_credits(id,user_id,cycle_start,next_reset,bytes,created_at,administrator_id) VALUES($1,$2,$3,$4,50,$5,1)")
        .bind(Uuid::new_v4())
        .bind(limited_id)
        .bind(start)
        .bind(next)
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    assert_eq!(
        entitlement(&panel, &cookie, limited_id).await?["used_bytes"],
        "50"
    );
    let response = panel
        .client
        .get(format!(
            "{}?format=singbox",
            limited["subscription_url"].as_str().unwrap()
        ))
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(
        header(&response, "subscription-userinfo")?,
        format!("upload=20; download=30; total=1000; expire={expires}")
    );
    // Without a package there is no quota or expiry; usage is cumulative.
    let response = panel
        .client
        .get(free["subscription_url"].as_str().unwrap())
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(
        header(&response, "subscription-userinfo")?,
        "upload=5; download=7; total=0"
    );
    assert_eq!(
        header(&response, "profile-title")?,
        format!("base64:{}", STANDARD.encode("Unmetered"))
    );
    Ok(())
}

/// Marks the admin session as recently re-authenticated, which applying an operation requires.
async fn prove(pool: &PgPool, cookie: &str) -> Result<()> {
    let token = cookie
        .strip_prefix("apsp_session=")
        .context("session cookie")?;
    let now = now_timestamp();
    sqlx::query("INSERT INTO administrator_reauth(session_hash,verified_at,expires_at) VALUES($1,$2,$3) ON CONFLICT(session_hash) DO UPDATE SET verified_at=$2,expires_at=$3")
        .bind(apsp_panel::auth::hash_token(token))
        .bind(now)
        .bind(now + 300)
        .execute(pool)
        .await?;
    Ok(())
}

async fn review(panel: &TestPanel, cookie: &str, request: Value) -> Result<Value> {
    call(
        panel,
        cookie,
        Method::POST,
        "/operations/preview",
        Some(request),
    )
    .await
}

async fn confirm(panel: &TestPanel, cookie: &str, reviewed: &Value) -> Result<(StatusCode, Value)> {
    let response = panel
        .admin(
            Method::POST,
            &format!(
                "{ROOT}/operations/{}/apply",
                reviewed["id"].as_str().context("preview id")?
            ),
            cookie,
            Some(json!({"confirm":true})),
        )
        .await?;
    let status = response.status();
    Ok((status, response.json().await?))
}

#[sqlx::test(migrations = "./migrations")]
async fn operations_apply_after_traffic_arrives_between_preview_and_apply(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    prove(&pool, &cookie).await?;
    let server = panel.create_server(&cookie, "Operations").await?;
    let node = id(&panel.create_node(&cookie, server, "Node").await?)?;
    let uid = id(&panel.create_user(&cookie, "Member").await?)?;
    panel.grant(&cookie, uid, node).await?;
    let plan = create_plan(&panel, &cookie, Some("1000")).await?;
    assign(&panel, &cookie, uid, plan, Uuid::new_v4()).await?;
    panel.publish_now().await?;
    applied(&pool).await?;
    apsp_panel::usage::ingest(
        &panel.state,
        server,
        usage(uid, node, now_timestamp(), 100, 200),
    )
    .await?;

    // Extending: traffic arrives after the preview and the apply still succeeds.
    let before = entitlement(&panel, &cookie, uid).await?;
    let extend = review(
        &panel,
        &cookie,
        json!({"operation":"extend_validity","user_id":uid,"days":30}),
    )
    .await?;
    apsp_panel::usage::ingest(
        &panel.state,
        server,
        usage(uid, node, now_timestamp(), 1, 2),
    )
    .await?;
    let (status, receipt) = confirm(&panel, &cookie, &extend).await?;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(
        receipt["expires_at"].as_i64().unwrap(),
        before["expires_at"].as_i64().unwrap() + 30 * 86400
    );

    // Resetting the quota credits what was used by apply time, not what the preview showed.
    let reset = review(
        &panel,
        &cookie,
        json!({"operation":"reset_quota","user_id":uid}),
    )
    .await?;
    assert_eq!(reset["summary"]["credit_bytes"], "303");
    apsp_panel::usage::ingest(
        &panel.state,
        server,
        usage(uid, node, now_timestamp(), 3, 4),
    )
    .await?;
    let (status, receipt) = confirm(&panel, &cookie, &reset).await?;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["credit_bytes"], "310");
    assert_eq!(entitlement(&panel, &cookie, uid).await?["used_bytes"], "0");

    // A real change still invalidates a preview: of two reviews of the same state, only the
    // first one applies.
    let first = review(
        &panel,
        &cookie,
        json!({"operation":"extend_validity","user_id":uid,"days":10}),
    )
    .await?;
    let second = review(
        &panel,
        &cookie,
        json!({"operation":"extend_validity","user_id":uid,"days":20}),
    )
    .await?;
    let (status, receipt) = confirm(&panel, &cookie, &first).await?;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    let (status, _) = confirm(&panel, &cookie, &second).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn extending_an_expired_user_renews_from_the_moment_it_is_applied(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    prove(&pool, &cookie).await?;
    let uid = id(&panel.create_user(&cookie, "Lapsed").await?)?;
    let plan = create_plan(&panel, &cookie, Some("1000")).await?;
    assign(&panel, &cookie, uid, plan, Uuid::new_v4()).await?;
    sqlx::query(
        "UPDATE singbox_package_assignments SET starts_at=$2-172800,expires_at=$2-86400 WHERE user_id=$1",
    )
    .bind(uid)
    .bind(now_timestamp())
    .execute(&pool)
    .await?;
    assert_eq!(
        entitlement(&panel, &cookie, uid).await?["status"],
        "expired"
    );
    let now = now_timestamp();
    let extend = review(
        &panel,
        &cookie,
        json!({"operation":"extend_validity","user_id":uid,"days":30}),
    )
    .await?;
    assert_eq!(extend["summary"]["extends_from_now"], true);
    assert!(extend["summary"]["new_expires_at"].as_i64().unwrap() >= now + 30 * 86400);
    let (status, receipt) = confirm(&panel, &cookie, &extend).await?;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    let renewed = entitlement(&panel, &cookie, uid).await?;
    assert_eq!(renewed["status"], "active");
    let expires = renewed["expires_at"].as_i64().unwrap();
    assert!(expires >= now + 30 * 86400 && expires <= now_timestamp() + 30 * 86400);
    Ok(())
}
