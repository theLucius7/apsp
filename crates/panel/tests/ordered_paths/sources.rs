use super::*;

async fn replace(fixture: &Fixture, revision: i64, content: &str, action: &str) -> Result<()> {
    api(&fixture.panel,&fixture.cookie,Method::PATCH,&format!("/ordered-subscription-sources/{}",fixture.source),Some(json!({
        "request_id":Uuid::new_v4(),"settings_revision":revision,"input":{"kind":"inline","content":content,"identity_action":action}
    })),StatusCode::OK).await?;
    apsp_panel::plugins::singbox::subscription_sources::worker::run_once(&fixture.panel.state)
        .await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn source_refresh_stages_only_same_identity_followed_node_while_pinned_keeps_its_version(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let followed = chain(&create(&fixture, "follow_node", false).await?)?;
    let pinned = chain(&create(&fixture, "pinned", false).await?)?;
    authorize(&fixture, followed).await?;
    authorize(&fixture, pinned).await?;
    advance(&fixture, followed, "applied").await?;
    advance(&fixture, pinned, "applied").await?;
    let original: Value = sqlx::query_scalar(
        "SELECT snapshot FROM singbox_ordered_chain_versions WHERE chain_id=$1 AND generation=1",
    )
    .bind(followed)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    replace(
        &fixture,
        1,
        &content("TEST_ONLY changed external password"),
        "update",
    )
    .await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let changed = resource(&fixture, followed).await?;
    let unchanged = resource(&fixture, pinned).await?;
    assert_eq!(changed["path_state"]["desired_generation"], 2);
    assert_eq!(changed["path_state"]["candidate_generation"], 2);
    assert_eq!(changed["path_state"]["applied_generation"], 1);
    assert_eq!(changed["path_state"]["recovery_generation"], 1);
    assert_eq!(unchanged["path_state"]["desired_generation"], 1);
    assert!(unchanged["path_state"]["candidate_generation"].is_null());
    assert_eq!(
        unchanged["hops"][0]["node_version_id"],
        fixture.external["version_id"]
    );
    let retained: Value = sqlx::query_scalar(
        "SELECT snapshot FROM singbox_ordered_chain_versions WHERE chain_id=$1 AND generation=1",
    )
    .bind(followed)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert_eq!(original, retained);
    no_secrets(&changed);
    no_secrets(&unchanged);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn failed_or_archived_source_preserves_applied_frozen_identity_and_no_fallback(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "follow_node", false).await?)?;
    let user = authorize(&fixture, chain).await?;
    advance(&fixture, chain, "applied").await?;
    let output = subscription(&fixture, &user).await?;
    let nodes = subscription_nodes(&output);
    assert_eq!(nodes.len(), 1);
    let frozen: Value = sqlx::query_scalar(
        "SELECT snapshot FROM singbox_ordered_chain_versions WHERE chain_id=$1 AND generation=1",
    )
    .bind(chain)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    replace(&fixture, 1, "not supported proxy data", "update").await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let view = resource(&fixture, chain).await?;
    assert_eq!(view["path_state"]["desired_generation"], 1);
    assert_eq!(view["path_state"]["applied_generation"], 1);
    assert!(view["path_state"]["candidate_generation"].is_null());
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::PATCH,
        &format!("/ordered-subscription-sources/{}", fixture.source),
        Some(json!({"request_id":Uuid::new_v4(),"settings_revision":2,"archived":true})),
        StatusCode::OK,
    )
    .await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_eq!(
        resource(&fixture, chain).await?["hops"][0]["source_archived"],
        true
    );
    let retained: Value = sqlx::query_scalar(
        "SELECT snapshot FROM singbox_ordered_chain_versions WHERE chain_id=$1 AND generation=1",
    )
    .bind(chain)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert_eq!(frozen, retained);
    assert_eq!(
        subscription_nodes(&subscription(&fixture, &user).await?).len(),
        1
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn replacing_source_identity_cannot_rebind_an_applied_following_path(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "follow_node", false).await?)?;
    let user = authorize(&fixture, chain).await?;
    advance(&fixture, chain, "applied").await?;
    let before = resource(&fixture, chain).await?;
    replace(
        &fixture,
        1,
        &content("TEST_ONLY different source identity"),
        "replace",
    )
    .await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let view = resource(&fixture, chain).await?;
    assert_eq!(view["path_state"]["desired_generation"], 1);
    assert_eq!(view["path_state"]["applied_generation"], 1);
    assert!(view["path_state"]["candidate_generation"].is_null());
    assert_eq!(
        view["hops"][0]["identity_epoch"],
        before["hops"][0]["identity_epoch"]
    );
    assert_eq!(view["hops"][0]["external_node_id"], fixture.external["id"]);
    assert_eq!(
        view["hops"][0]["node_version_id"],
        fixture.external["version_id"]
    );
    assert_eq!(view["hops"][0]["node_present"], false);
    assert!(view["hops"][0]["update_error"].is_string());
    assert_eq!(
        subscription_nodes(&subscription(&fixture, &user).await?).len(),
        1
    );
    let mut invalid = item(&fixture, "follow_node", false);
    invalid["name"] = json!("Never bind old identity");
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::POST,
        "/chains/ordered-batch",
        Some(json!({"request_id":Uuid::new_v4(),"items":[invalid]})),
        StatusCode::CONFLICT,
    )
    .await?;
    no_secrets(&view);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn explicit_pinned_version_update_is_same_node_cas_and_exact_receipt_replay(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", false).await?)?;
    authorize(&fixture, chain).await?;
    advance(&fixture, chain, "applied").await?;
    replace(
        &fixture,
        1,
        &content("TEST_ONLY changed external password"),
        "update",
    )
    .await?;
    let nodes = api(
        &fixture.panel,
        &fixture.cookie,
        Method::GET,
        &format!("/ordered-subscription-sources/{}/nodes", fixture.source),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(nodes["nodes"][0]["id"], fixture.external["id"]);
    let version = nodes["nodes"][0]["version_id"].clone();
    assert_ne!(version, fixture.external["version_id"]);
    let view = resource(&fixture, chain).await?;
    let body = json!({"request_id":Uuid::new_v4(),"settings_revision":view["settings_revision"],"generation":1,
        "versions":[{"hop_position":1,"node_version_id":version}]});
    let route = format!("/ordered-proxy-resources/chain/{chain}/apply-node-versions");
    let receipt = api(
        &fixture.panel,
        &fixture.cookie,
        Method::POST,
        &route,
        Some(body.clone()),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(receipt["generation"], 2);
    assert_eq!(
        api(
            &fixture.panel,
            &fixture.cookie,
            Method::POST,
            &route,
            Some(body.clone()),
            StatusCode::OK
        )
        .await?,
        receipt
    );
    let mut stale = body;
    stale["request_id"] = json!(Uuid::new_v4());
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::POST,
        &route,
        Some(stale),
        StatusCode::CONFLICT,
    )
    .await?;
    let desired = resource(&fixture, chain).await?;
    assert_eq!(desired["hops"][0]["node_version_id"], version);
    assert_eq!(desired["path_state"]["applied_generation"], 1);
    no_secrets(&desired);
    Ok(())
}
