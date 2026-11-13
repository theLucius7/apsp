use super::*;

async fn counts(fixture: &Fixture) -> Result<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('nodes',(SELECT count(*) FROM nodes),'chains',(SELECT count(*) FROM singbox_chains),'versions',(SELECT count(*) FROM singbox_ordered_chain_versions),'receipts',(SELECT count(*) FROM singbox_ordered_chain_creation_requests),'dirty',(SELECT jsonb_agg(jsonb_build_array(id,dirty_at) ORDER BY id) FROM servers))")
        .fetch_one(&fixture.panel.state.pool).await?)
}

#[sqlx::test(migrations = "./migrations")]
async fn ordered_node_views_match_candidate_and_damaged_entry_configuration_guards(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let receipt = create(&fixture, "pinned", true).await?;
    let chain = chain(&receipt)?;
    let entry = receipt["entry_node_ids"][0].as_i64().context("entry")?;
    let expected = json!([{"id":chain,"name":"A-M-X-B"}]);
    for node in [entry, fixture.nodes[1], fixture.nodes[2]] {
        let view = api(
            &fixture.panel,
            &fixture.cookie,
            Method::GET,
            &format!("/nodes/{node}"),
            None,
            StatusCode::OK,
        )
        .await?;
        assert_eq!(view["configuration_locked"], true);
        assert_eq!(view["referenced_chains"], expected);
        no_secrets(&view);
        api(
            &fixture.panel,
            &fixture.cookie,
            Method::PATCH,
            &format!("/nodes/{node}"),
            Some(json!({"settings":{"listen":"127.0.0.1"}})),
            StatusCode::CONFLICT,
        )
        .await?;
    }
    // TEST_ONLY old settings omit defaults. Saving the exact public model must
    // not change the immutable candidate or bypass its edit guard.
    sqlx::query("UPDATE nodes SET settings='{}' WHERE id=$1")
        .bind(entry)
        .execute(&fixture.panel.state.pool)
        .await?;
    let view = api(
        &fixture.panel,
        &fixture.cookie,
        Method::GET,
        &format!("/nodes/{entry}"),
        None,
        StatusCode::OK,
    )
    .await?;
    sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
        .bind(fixture.servers[0])
        .execute(&fixture.panel.state.pool)
        .await?;
    let frozen: Value = sqlx::query_scalar(
        "SELECT snapshot FROM singbox_ordered_chain_versions WHERE chain_id=$1 AND generation=1",
    )
    .bind(chain)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    let saved = api(
        &fixture.panel,
        &fixture.cookie,
        Method::PATCH,
        &format!("/nodes/{entry}"),
        Some(json!({"settings":view["settings"]})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(saved["configuration_locked"], true);
    assert_eq!(saved["referenced_chains"], expected);
    let dirty: Option<i64> = sqlx::query_scalar("SELECT dirty_at FROM servers WHERE id=$1")
        .bind(fixture.servers[0])
        .fetch_one(&fixture.panel.state.pool)
        .await?;
    assert_eq!(dirty, None);
    // TEST_ONLY both pointers are damaged; an unresolved, soft-deleted owner
    // must still lock its entry without exposing a private frozen snapshot.
    sqlx::query("UPDATE singbox_chains SET desired_generation=99,candidate_generation=99,deleted_at=1,phase='retiring' WHERE id=$1")
        .bind(chain).execute(&fixture.panel.state.pool).await?;
    let view = api(
        &fixture.panel,
        &fixture.cookie,
        Method::GET,
        &format!("/nodes/{entry}"),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(view["configuration_locked"], true);
    assert_eq!(view["referenced_chains"], expected);
    no_secrets(&view);
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::PATCH,
        &format!("/nodes/{entry}"),
        Some(json!({"settings":{"listen":"127.0.0.1"}})),
        StatusCode::CONFLICT,
    )
    .await?;
    let renamed = api(
        &fixture.panel,
        &fixture.cookie,
        Method::PATCH,
        &format!("/nodes/{entry}"),
        Some(json!({"name":"Allowed rename","enabled":false})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(renamed["configuration_locked"], true);
    assert_eq!(renamed["referenced_chains"], expected);
    let retained: Value = sqlx::query_scalar(
        "SELECT snapshot FROM singbox_ordered_chain_versions WHERE chain_id=$1 AND generation=1",
    )
    .bind(chain)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert_eq!(retained, frozen);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn full_four_hop_creation_freezes_ordered_identities_without_opening_user_route(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let receipt = create(&fixture, "pinned", true).await?;
    let chain = chain(&receipt)?;
    let view = resource(&fixture, chain).await?;
    assert_eq!(view["path_kind"], "ordered");
    assert_eq!(view["hops"].as_array().unwrap().len(), 3);
    assert_eq!(view["hops"][0]["node_id"], fixture.nodes[1]);
    assert_eq!(view["hops"][1]["external_node_id"], fixture.external["id"]);
    assert_eq!(
        view["hops"][1]["node_version_id"],
        fixture.external["version_id"]
    );
    assert_eq!(view["hops"][2]["node_id"], fixture.nodes[2]);
    assert_eq!(view["path_state"]["phase"], "preparing_dependencies");
    assert!(view["path_state"]["applied_generation"].is_null());
    assert_eq!(view["path_state"]["candidate_generation"], 1);
    assert_eq!(view["exit"]["id"], fixture.nodes[2]);
    no_secrets(&view);
    let route: bool = sqlx::query_scalar("SELECT route_enabled FROM singbox_chains WHERE id=$1")
        .bind(chain)
        .fetch_one(&fixture.panel.state.pool)
        .await?;
    assert!(!route);
    let frozen: Value = sqlx::query_scalar(
        "SELECT snapshot FROM singbox_ordered_chain_versions WHERE chain_id=$1 AND generation=1",
    )
    .bind(chain)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert_eq!(
        frozen["hops"][1]["outbound"]["password"],
        "TEST_ONLY external password"
    );
    assert!(
        sqlx::query("UPDATE singbox_ordered_chain_versions SET snapshot='{}' WHERE chain_id=$1")
            .bind(chain)
            .execute(&fixture.panel.state.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM singbox_ordered_chain_hops WHERE chain_id=$1")
            .bind(chain)
            .execute(&fixture.panel.state.pool)
            .await
            .is_err()
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn atomic_batch_rolls_back_invalid_last_item_and_replay_survives_source_archive(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let mut invalid = item(&fixture, "pinned", false);
    invalid["hops"][0]["node_version_id"] = json!(Uuid::new_v4());
    let before = counts(&fixture).await?;
    let response = fixture
        .panel
        .admin(
            Method::POST,
            "/api/traffic/chains/ordered-batch",
            &fixture.cookie,
            Some(
                json!({"request_id":Uuid::new_v4(),"items":[item(&fixture,"pinned",true),invalid]}),
            ),
        )
        .await?;
    assert!(response.status().is_client_error());
    assert_eq!(counts(&fixture).await?, before);
    let body = json!({"request_id":Uuid::new_v4(),"items":[item(&fixture,"pinned",false)]});
    let receipt = api(
        &fixture.panel,
        &fixture.cookie,
        Method::POST,
        "/chains/ordered-batch",
        Some(body.clone()),
        StatusCode::CREATED,
    )
    .await?;
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::PATCH,
        &format!("/ordered-subscription-sources/{}", fixture.source),
        Some(json!({"request_id":Uuid::new_v4(),"settings_revision":1,"archived":true})),
        StatusCode::OK,
    )
    .await?;
    let before = counts(&fixture).await?;
    assert_eq!(
        api(
            &fixture.panel,
            &fixture.cookie,
            Method::POST,
            "/chains/ordered-batch",
            Some(body.clone()),
            StatusCode::OK
        )
        .await?,
        receipt
    );
    let mut changed = body;
    changed["items"][0]["name"] = json!("Different request");
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::POST,
        "/chains/ordered-batch",
        Some(changed),
        StatusCode::CONFLICT,
    )
    .await?;
    assert_eq!(counts(&fixture).await?, before);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn loop_duplicate_endpoint_and_unsupported_agent_never_create_partial_resources(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    for hops in [
        json!([]),
        json!([{"kind":"managed","node_id":fixture.nodes[0]}]),
        json!([{"kind":"managed","node_id":fixture.nodes[1]},{"kind":"managed","node_id":fixture.nodes[1]}]),
    ] {
        let mut body = item(&fixture, "pinned", true);
        body["hops"] = hops;
        let before = counts(&fixture).await?;
        let response = fixture
            .panel
            .admin(
                Method::POST,
                "/api/traffic/chains/ordered-batch",
                &fixture.cookie,
                Some(json!({"request_id":Uuid::new_v4(),"items":[body]})),
            )
            .await?;
        assert!(response.status().is_client_error());
        assert_eq!(counts(&fixture).await?, before);
    }
    sqlx::query("UPDATE servers SET capabilities='[]' WHERE id=$1")
        .bind(fixture.servers[2])
        .execute(&fixture.panel.state.pool)
        .await?;
    let before = counts(&fixture).await?;
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::POST,
        "/chains/ordered-batch",
        Some(json!({"request_id":Uuid::new_v4(),"items":[item(&fixture,"pinned",false)]})),
        StatusCode::CONFLICT,
    )
    .await?;
    assert_eq!(counts(&fixture).await?, before);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn candidate_source_and_managed_references_guard_all_delete_and_edit_routes(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", true).await?)?;
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::DELETE,
        &format!("/ordered-subscription-sources/{}", fixture.source),
        Some(json!({"settings_revision":1})),
        StatusCode::CONFLICT,
    )
    .await?;
    for node in [fixture.nodes[1], fixture.nodes[2]] {
        api(
            &fixture.panel,
            &fixture.cookie,
            Method::DELETE,
            &format!("/nodes/{node}"),
            None,
            StatusCode::CONFLICT,
        )
        .await?;
        api(
            &fixture.panel,
            &fixture.cookie,
            Method::DELETE,
            &format!("/ordered-proxy-resources/direct/{node}"),
            None,
            StatusCode::CONFLICT,
        )
        .await?;
        api(
            &fixture.panel,
            &fixture.cookie,
            Method::PATCH,
            &format!("/nodes/{node}"),
            Some(json!({"public_host":"changed.example.net"})),
            StatusCode::CONFLICT,
        )
        .await?;
    }
    let source = api(
        &fixture.panel,
        &fixture.cookie,
        Method::GET,
        &format!("/ordered-subscription-sources/{}", fixture.source),
        None,
        StatusCode::OK,
    )
    .await?;
    assert!(source["dependencies"].as_array().context("source references")?.iter().any(|dependency|dependency["chain_id"]==chain && dependency["state"]=="candidate"));
    no_secrets(&source);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn resource_mutations_use_revision_cas_and_exact_immutable_receipts(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", false).await?)?;
    let route = format!("/ordered-proxy-resources/chain/{chain}");
    let body = json!({"request_id":Uuid::new_v4(),"settings_revision":1,"name":"Renamed resource"});
    let receipt = api(
        &fixture.panel,
        &fixture.cookie,
        Method::PATCH,
        &route,
        Some(body.clone()),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(receipt["settings_revision"], 2);
    assert_eq!(
        api(
            &fixture.panel,
            &fixture.cookie,
            Method::PATCH,
            &route,
            Some(body.clone()),
            StatusCode::OK
        )
        .await?,
        receipt
    );
    let mut stale = body.clone();
    stale["request_id"] = json!(Uuid::new_v4());
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::PATCH,
        &route,
        Some(stale),
        StatusCode::CONFLICT,
    )
    .await?;
    let mut different = body;
    different["name"] = json!("Changed same ID");
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::PATCH,
        &route,
        Some(different),
        StatusCode::CONFLICT,
    )
    .await?;
    let view = resource(&fixture, chain).await?;
    assert_eq!(view["name"], "Renamed resource");
    assert_eq!(view["settings_revision"], 2);
    assert_eq!(view["path_state"]["desired_generation"], 1);
    no_secrets(&view);
    Ok(())
}
