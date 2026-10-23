use super::*;

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
    assert!(
        panel
            .client
            .get(sub)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?
            .is_empty()
    );
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
