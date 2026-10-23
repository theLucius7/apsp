use super::*;

pub(super) async fn ordered_negative_inputs(panel: &TestPanel) -> Result<()> {
    sqlx::query("UPDATE servers SET capabilities=$1")
        .bind(json!([
            apsp_protocol::RUNTIME_CHECKPOINT_CAPABILITY,
            apsp_protocol::RUNTIME_RECOVERY_BARRIER_CAPABILITY,
            apsp_protocol::RUNTIME_PATH_PROBE_CAPABILITY
        ]))
        .execute(&panel.state.pool)
        .await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn chain_is_private_billed_once_and_requires_both_applied_endpoints(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let a = panel.create_server(&cookie, "Entry server").await?;
    let b = panel.create_server(&cookie, "Exit server").await?;
    let entry = id(&panel.create_node(&cookie, a, "Entry").await?)?;
    let exit = id(&panel.create_node(&cookie, b, "Exit").await?)?;
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/nodes/{exit}"),
        Some(json!({"settings":{"public_port":8443,"reality":{"fingerprint":"firefox","flow":"none"},"transport":{"type":"grpc","service_name":"private-relay"}}})),
    )
    .await?;
    let chain = panel
        .import_legacy_chain(&cookie, "Two hops", entry, exit)
        .await?;
    let chain_id = id(&chain)?;
    let relay: Uuid = sqlx::query_scalar("SELECT relay_uuid FROM singbox_chains WHERE id=$1")
        .bind(chain_id)
        .fetch_one(&pool)
        .await?;
    assert!(!chain.to_string().contains(&relay.to_string()));
    let user = panel.create_user(&cookie, "Member").await?;
    let uid = id(&user)?;
    let group = create_policy(&panel, &cookie, &[], &[chain_id]).await?;
    set_policies(&panel, &cookie, uid, &[group]).await?;
    assert_eq!(eligible(&pool, uid, now_timestamp()).await?, vec![entry]);
    let direct = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/users/{uid}/accesses"),
            &cookie,
            Some(json!({"node_id":entry})),
        )
        .await?;
    assert_eq!(direct.status(), StatusCode::CONFLICT);
    entitlements::refresh(&pool, now_timestamp()).await?;
    panel.publish_now().await?;
    let entry_config = latest_config(&pool, a).await?;
    let exit_config = latest_config(&pool, b).await?;
    assert_eq!(
        entry_config["route"]["rules"][0]["outbound"],
        format!("chain-{chain_id}")
    );
    assert_eq!(entry_config["outbounds"][1]["uuid"], relay.to_string());
    assert_eq!(entry_config["outbounds"][1]["server_port"], 8443);
    assert_eq!(
        entry_config["outbounds"][1]["tls"]["utls"]["fingerprint"],
        "firefox"
    );
    assert!(entry_config["outbounds"][1].get("flow").is_none());
    assert_eq!(
        entry_config["outbounds"][1]["transport"],
        json!({"type":"grpc","service_name":"private-relay"})
    );
    assert_eq!(
        exit_config["inbounds"][0]["transport"],
        entry_config["outbounds"][1]["transport"]
    );
    assert!(exit_config["inbounds"][0]["users"][0].get("flow").is_none());
    let snapshot: Value = sqlx::query_scalar(
        "SELECT path_json FROM singbox_chain_versions WHERE chain_id=$1 AND generation=1",
    )
    .bind(chain_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        snapshot["hops"][0]["endpoint"]["settings"]["transport"],
        entry_config["outbounds"][1]["transport"]
    );
    assert_eq!(
        exit_config["inbounds"][0]["users"][0]["uuid"],
        relay.to_string()
    );
    assert_eq!(
        entry_config["experimental"]["v2ray_api"]["stats"]["users"],
        json!([format!("u{uid}_n{entry}")])
    );
    assert_eq!(
        exit_config["experimental"]["v2ray_api"]["stats"]["users"],
        json!([])
    );
    let sub = format!(
        "{}?format=singbox",
        user["subscription_url"].as_str().unwrap()
    );
    assert_eq!(
        panel.client.get(&sub).send().await?.status(),
        StatusCode::CONFLICT
    );
    sqlx::query(
        "UPDATE server_module_status SET applied_rev=target_rev,healthy=TRUE WHERE server_id=$1",
    )
    .bind(a)
    .execute(&pool)
    .await?;
    assert_eq!(
        panel.client.get(&sub).send().await?.status(),
        StatusCode::CONFLICT
    );
    applied(&pool).await?;
    let client: Value = panel
        .client
        .get(&sub)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        client["outbounds"][0]["outbounds"],
        json!([format!("node-{entry}")])
    );
    assert!(!client.to_string().contains(&relay.to_string()));
    let list = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/users/{uid}/accesses"),
        None,
    )
    .await?;
    assert_eq!(list.as_array().unwrap().len(), 1);
    // Pausing either endpoint removes the chain, while preserving its grants.
    for node in [entry, exit] {
        call(
            &panel,
            &cookie,
            Method::PATCH,
            &format!("/nodes/{node}"),
            Some(json!({"enabled":false})),
        )
        .await?;
        assert!(eligible(&pool, uid, now_timestamp()).await?.is_empty());
        assert_eq!(
            panel.client.get(&sub).send().await?.status(),
            StatusCode::CONFLICT
        );
        panel.publish_now().await?;
        assert!(
            latest_config(&pool, a).await?["inbounds"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            call(&panel, &cookie, Method::GET, "/chains", None).await?[0]["available"],
            false
        );
        call(
            &panel,
            &cookie,
            Method::PATCH,
            &format!("/nodes/{node}"),
            Some(json!({"enabled":true})),
        )
        .await?;
        assert_eq!(eligible(&pool, uid, now_timestamp()).await?, vec![entry]);
        panel.publish_now().await?;
        applied(&pool).await?;
        assert_eq!(
            panel.client.get(&sub).send().await?.status(),
            StatusCode::OK
        );
    }
    // A referenced exit cannot be deleted through the legacy node endpoint.
    let conflict = panel
        .admin(
            Method::DELETE,
            &format!("{ROOT}/nodes/{exit}"),
            &cookie,
            None,
        )
        .await?;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    // Disabling an exit still revokes the entry without a direct fallback.
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/nodes/{exit}"),
        Some(json!({"enabled":false})),
    )
    .await?;
    assert!(eligible(&pool, uid, now_timestamp()).await?.is_empty());
    assert_eq!(
        panel.client.get(&sub).send().await?.status(),
        StatusCode::CONFLICT
    );
    panel.publish_now().await?;
    assert!(
        latest_config(&pool, a).await?["inbounds"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        call(&panel, &cookie, Method::GET, "/chains", None).await?[0]["available"],
        false
    );
    call(
        &panel,
        &cookie,
        Method::PUT,
        &format!("/policy-groups/{group}"),
        Some(json!({"name":"Empty","node_ids":[],"chain_ids":[]})),
    )
    .await?;
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/chains/{chain_id}"),
        None,
    )
    .await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn chain_rejects_existing_direct_grants_cycles_nesting_and_same_server(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_public_url(pool, Some("https://panel.example")).await?;
    let cookie = panel.admin_cookie().await?;
    let a = panel.create_server(&cookie, "A").await?;
    let b = panel.create_server(&cookie, "B").await?;
    let c = panel.create_server(&cookie, "C").await?;
    let na = id(&panel.create_node(&cookie, a, "A").await?)?;
    let na2 = id(&panel.create_node(&cookie, a, "A2").await?)?;
    let nb = id(&panel.create_node(&cookie, b, "B").await?)?;
    let nc = id(&panel.create_node(&cookie, c, "C").await?)?;
    let user = id(&panel.create_user(&cookie, "Direct").await?)?;
    panel.grant(&cookie, user, na2).await?;
    ordered_negative_inputs(&panel).await?;
    // Two-hop creation is closed (ADR 0079 phase 3, S1d): every request is refused.
    for (entry, exit, status) in [
        (na, na2, StatusCode::CONFLICT),
        (na2, nb, StatusCode::CONFLICT),
        (na, na, StatusCode::CONFLICT),
    ] {
        assert_eq!(
            panel
                .admin(
                    Method::POST,
                    &format!("{ROOT}/chains"),
                    &cookie,
                    Some(json!({"name":"Invalid","entry_node_id":entry,"exit_node_id":exit}))
                )
                .await?
                .status(),
            status
        );
    }
    let chain = id(&panel.import_legacy_chain(&cookie, "AB", na, nb).await?)?;
    for (entry, exit) in [(nb, na), (nb, nc), (nc, na), (na, nc)] {
        assert_eq!(
            panel
                .admin(
                    Method::POST,
                    &format!("{ROOT}/chains"),
                    &cookie,
                    Some(json!({"name":"Invalid","entry_node_id":entry,"exit_node_id":exit}))
                )
                .await?
                .status(),
            StatusCode::CONFLICT
        );
    }
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("{ROOT}/policy-groups"),
                &cookie,
                Some(json!({"name":"Bypass","node_ids":[na],"chain_ids":[]}))
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    create_policy(&panel, &cookie, &[], &[chain]).await?;
    assert_eq!(
        panel
            .admin(
                Method::DELETE,
                &format!("{ROOT}/chains/{chain}"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn retiring_chain_exit_schedules_entry_revocation_even_without_user_requests(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let a = panel.create_server(&cookie, "A").await?;
    let b = panel.create_server(&cookie, "B").await?;
    let entry = id(&panel.create_node(&cookie, a, "Entry").await?)?;
    let exit = id(&panel.create_node(&cookie, b, "Exit").await?)?;
    let chain = id(&panel
        .import_legacy_chain(&cookie, "AB", entry, exit)
        .await?)?;
    let user = id(&panel.create_user(&cookie, "Member").await?)?;
    let group = create_policy(&panel, &cookie, &[], &[chain]).await?;
    set_policies(&panel, &cookie, user, &[group]).await?;
    entitlements::refresh(&pool, now_timestamp()).await?;
    panel.publish_now().await?;
    // Simulate the core's soft-delete boundary; no plugin API or traffic occurs.
    sqlx::query("UPDATE servers SET deleted_at=$2 WHERE id=$1")
        .bind(b)
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    entitlements::refresh(&pool, now_timestamp()).await?;
    let dirty: Option<i64> = sqlx::query_scalar("SELECT dirty_at FROM servers WHERE id=$1")
        .bind(a)
        .fetch_one(&pool)
        .await?;
    assert!(dirty.is_some());
    panel.publish_now().await?;
    assert!(
        latest_config(&pool, a).await?["inbounds"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn chains_reject_modern_protocol_at_either_endpoint(pool: PgPool) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example")).await?;
    let cookie = panel.admin_cookie().await?;
    let a = panel.create_server(&cookie, "A").await?;
    let b = panel.create_server(&cookie, "B").await?;
    let entry = id(&panel.create_node(&cookie, a, "Reality entry").await?)?;
    let exit = id(&panel.create_node(&cookie, b, "Reality exit").await?)?;
    let mut modern = Vec::new();
    for server in [a, b] {
        modern.push(id(&call(
            &panel,
            &cookie,
            Method::POST,
            "/nodes",
            Some(json!({
                "name":"Modern", "server_id":server, "public_host":"proxy.example.com", "sni":"",
                "protocol_config":{"type":"shadowsocks2022", "method":"2022-blake3-aes-256-gcm"}
            })),
        )
        .await?)?);
    }
    ordered_negative_inputs(&panel).await?;
    for (entry_node, exit_node) in [(modern[0], exit), (entry, modern[1])] {
        let response = panel.admin(
            Method::POST,
            &format!("{ROOT}/chains"),
            &cookie,
            Some(json!({"name":"Invalid protocol", "entry_node_id":entry_node, "exit_node_id":exit_node})),
        ).await?;
        // Two-hop creation is closed (ADR 0079 phase 3, S1d).
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM singbox_chains")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 0);
    panel
        .import_legacy_chain(&cookie, "Reality", entry, exit)
        .await?;
    Ok(())
}
