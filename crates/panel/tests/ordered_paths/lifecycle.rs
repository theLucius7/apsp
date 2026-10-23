use super::*;

#[sqlx::test(migrations = "./migrations")]
async fn exact_preparation_probes_switch_and_recovery_barrier_preserve_entry_credentials(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let receipt = create(&fixture, "pinned", true).await?;
    let chain = chain(&receipt)?;
    let entry = receipt["entry_node_ids"][0].as_i64().context("entry ID")?;
    let user = authorize(&fixture, chain).await?;
    let credential: Uuid =
        sqlx::query_scalar("SELECT uuid FROM accesses WHERE user_id=$1 AND node_id=$2")
            .bind(id(&user)?)
            .bind(entry)
            .fetch_one(&fixture.panel.state.pool)
            .await?;
    assert!(subscription_nodes(&subscription(&fixture, &user).await?).is_empty());
    fixture.panel.publish_now().await?;
    let raw:String=sqlx::query_scalar("SELECT bundle FROM deployments WHERE server_id=$1 AND module='singbox' ORDER BY rev DESC LIMIT 1").bind(fixture.servers[0]).fetch_one(&fixture.panel.state.pool).await?;
    let bundle: Bundle = serde_json::from_str(&raw)?;
    let native: Value = serde_json::from_str(&bundle.files["config.json"])?;
    assert!(
        !native["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|inbound| inbound["tag"] == format!("node-{entry}"))
    );
    let phases = advance(&fixture, chain, "applied").await?;
    assert!(phases.iter().any(|phase| phase == "preparing_entry"));
    assert!(phases.iter().any(|phase| phase == "switching_entry"));
    let view = resource(&fixture, chain).await?;
    assert_eq!(view["path_state"]["applied_generation"], 1);
    assert_eq!(view["path_state"]["minimum_generation"], 1);
    assert!(view["path_state"]["candidate_generation"].is_null());
    no_secrets(&view);
    let confirmed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM singbox_path_probes WHERE chain_id=$1 AND state='verified'",
    )
    .bind(chain)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert_eq!(confirmed, 2);
    let current: Uuid =
        sqlx::query_scalar("SELECT uuid FROM accesses WHERE user_id=$1 AND node_id=$2")
            .bind(id(&user)?)
            .bind(entry)
            .fetch_one(&fixture.panel.state.pool)
            .await?;
    assert_eq!(credential, current);
    let output = subscription(&fixture, &user).await?;
    let nodes = subscription_nodes(&output);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["uuid"], credential.to_string());
    let retained:Value=sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_array(chain_id,generation,position,node_version_id) ORDER BY chain_id,generation,position) FROM singbox_ordered_chain_hops").fetch_one(&fixture.panel.state.pool).await?;
    let restarted = AppState::new(
        fixture.panel.state.pool.clone(),
        (*fixture.panel.state.config).clone(),
    )
    .await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&restarted).await?;
    let after:Value=sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_array(chain_id,generation,position,node_version_id) ORDER BY chain_id,generation,position) FROM singbox_ordered_chain_hops").fetch_one(&fixture.panel.state.pool).await?;
    assert_eq!(retained, after);
    assert_eq!(phase(&fixture, chain).await?, "applied");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn failed_candidate_cannot_open_an_initial_path_or_publish_user_subscription(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", false).await?)?;
    let user = authorize(&fixture, chain).await?;
    for _ in 0..12 {
        fixture.panel.publish_now().await?;
        confirm_devices(&fixture.panel.state).await?;
        apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state)
            .await?;
        if phase(&fixture, chain).await? == "probing_candidate" {
            break;
        }
    }
    assert_eq!(phase(&fixture, chain).await?, "probing_candidate");
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    finish_controls(&fixture.panel.state, false).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_eq!(phase(&fixture, chain).await?, "failed");
    assert!(subscription_nodes(&subscription(&fixture, &user).await?).is_empty());
    let (route,applied,minimum):(bool,Option<i64>,i64)=sqlx::query_as("SELECT route_enabled,applied_generation,minimum_generation FROM singbox_chains WHERE id=$1").bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert!(!route && applied.is_none());
    assert_eq!(minimum, 0);
    assert_eq!(
        resource(&fixture, chain).await?["path_state"]["probe"]["state"],
        "failed"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn coarse_apply_or_wrong_probe_receipt_cannot_replace_exact_path_confirmation(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", false).await?)?;
    fixture.panel.publish_now().await?;
    let revision: i64 = sqlx::query_scalar(
        "SELECT target_rev FROM server_module_status WHERE server_id=$1 AND module='singbox'",
    )
    .bind(fixture.servers[2])
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    agent_api::process_message(
        &fixture.panel.state,
        fixture.servers[2],
        Message::ApplyResult(ApplyResult {
            module: "singbox".into(),
            rev: revision as u64,
            op_id: Uuid::new_v4(),
            status: ApplyStatus::Applied,
            healthy: true,
            error: None,
        }),
    )
    .await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_eq!(phase(&fixture, chain).await?, "preparing_dependencies");
    for _ in 0..12 {
        fixture.panel.publish_now().await?;
        confirm_devices(&fixture.panel.state).await?;
        apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state)
            .await?;
        if phase(&fixture, chain).await? == "probing_candidate" {
            break;
        }
    }
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let (server,payload):(i64,Value)=sqlx::query_as("SELECT server_id,request_json FROM runtime_control_requests WHERE kind='probe' AND state='pending'").fetch_one(&fixture.panel.state.pool).await?;
    let request: RuntimePathProbeRequest = serde_json::from_value(payload)?;
    let mut wrong = request.expected.clone();
    wrong.activation_id = Uuid::new_v4();
    runtime_control::record_path_probe_result(
        &fixture.panel.state,
        server,
        RuntimePathProbeResult {
            request_id: request.request_id,
            request_digest: request.digest()?,
            observed: Some(wrong),
            probe_id: request.probe_id,
            elapsed_ms: Some(17),
            success: true,
            error: None,
        },
    )
    .await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_ne!(phase(&fixture, chain).await?, "switching_entry");
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT route_enabled FROM singbox_chains WHERE id=$1")
            .bind(chain)
            .fetch_one(&fixture.panel.state.pool)
            .await?
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn late_committed_barrier_retains_semantic_floor_and_never_republishes_lower_generation(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", false).await?)?;
    let user = authorize(&fixture, chain).await?;
    advance(&fixture, chain, "applied").await?;
    api(&fixture.panel,&fixture.cookie,Method::PATCH,&format!("/ordered-subscription-sources/{}",fixture.source),Some(json!({
        "request_id":Uuid::new_v4(),"settings_revision":1,"input":{"kind":"inline","content":content("TEST_ONLY new pinned secret"),"identity_action":"update"}
    })),StatusCode::OK).await?;
    apsp_panel::plugins::singbox::subscription_sources::worker::run_once(&fixture.panel.state)
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
    let view = resource(&fixture, chain).await?;
    api(&fixture.panel,&fixture.cookie,Method::POST,&format!("/ordered-proxy-resources/chain/{chain}/apply-node-versions"),Some(json!({
        "request_id":Uuid::new_v4(),"settings_revision":view["settings_revision"],"generation":1,
        "versions":[{"hop_position":1,"node_version_id":nodes["nodes"][0]["version_id"]}]
    })),StatusCode::OK).await?;
    advance(&fixture, chain, "fixing_barrier").await?;
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let (server,payload):(i64,Value)=sqlx::query_as("SELECT server_id,request_json FROM runtime_control_requests WHERE kind='barrier' AND state='pending'").fetch_one(&fixture.panel.state.pool).await?;
    let mut request: RuntimeRecoveryBarrierRequest = serde_json::from_value(payload)?;
    // TEST_ONLY late receipt: no claim that an expired command was executed by a device.
    request.expires_at = now_timestamp() - 1;
    sqlx::query("UPDATE runtime_control_requests SET created_at=$2-120,expires_at=$2,request_json=$3,request_digest=$4 WHERE request_id=$1")
        .bind(request.request_id).bind(request.expires_at).bind(json!(request)).bind(request.digest()?).execute(&fixture.panel.state.pool).await?;
    runtime_control::record_barrier_result(
        &fixture.panel.state,
        server,
        RuntimeRecoveryBarrierResult {
            request_id: request.request_id,
            request_digest: request.digest()?,
            observed: Some(request.expected),
            minimum_revision: Some(request.minimum_revision),
            pending_intents_clear: true,
            success: true,
            error: None,
        },
    )
    .await?;
    let outcome: String =
        sqlx::query_scalar("SELECT outcome FROM runtime_control_receipts WHERE request_id=$1")
            .bind(request.request_id)
            .fetch_one(&fixture.panel.state.pool)
            .await?;
    assert_eq!(outcome, "late");
    // Cover a panel crash after enqueue but before the local stage/request association.
    sqlx::query("UPDATE singbox_path_stage_deployments SET barrier_request_id=NULL WHERE chain_id=$1 AND generation=2").bind(chain).execute(&fixture.panel.state.pool).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let (minimum, phase, applied): (i64, String, Option<i64>) = sqlx::query_as(
        "SELECT minimum_generation,phase,applied_generation FROM singbox_chains WHERE id=$1",
    )
    .bind(chain)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert_eq!(minimum, 2);
    assert_eq!(phase, "failed");
    assert_eq!(applied, Some(1));
    assert!(subscription_nodes(&subscription(&fixture, &user).await?).is_empty());
    fixture.panel.publish_now().await?;
    let unsafe_routes:i64=sqlx::query_scalar("SELECT count(*) FROM singbox_path_deployment_dependencies d JOIN server_module_status s ON s.server_id=d.server_id AND s.module=d.module AND s.target_rev=d.revision WHERE d.chain_id=$1 AND d.route_active AND d.generation<2")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_eq!(
        unsafe_routes, 0,
        "higher deployment revisions must not reintroduce a lower semantic generation"
    );
    let recovery: Value = resource(&fixture, chain).await?;
    assert_eq!(recovery["path_state"]["minimum_generation"], 2);
    no_secrets(&recovery);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_managed_dependency_revision_requires_a_new_probe_even_when_entry_is_unchanged(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", true).await?)?;
    let user = authorize(&fixture, chain).await?;
    advance(&fixture, chain, "applied").await?;
    let entry_before:Value=sqlx::query_scalar("SELECT checkpoint_json FROM runtime_module_checkpoints WHERE server_id=$1 AND module='singbox'").bind(fixture.servers[0]).fetch_one(&fixture.panel.state.pool).await?;
    let original:Uuid=sqlx::query_scalar("SELECT request_id FROM singbox_path_probes WHERE chain_id=$1 AND generation=1 AND stage='switched'").bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    let middle_before: i64 = sqlx::query_scalar(
        "SELECT target_rev FROM server_module_status WHERE server_id=$1 AND module='singbox'",
    )
    .bind(fixture.servers[1])
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    let direct = fixture
        .panel
        .create_user(&fixture.cookie, "Independent middle-server terminal user")
        .await?;
    fixture
        .panel
        .grant(&fixture.cookie, id(&direct)?, fixture.nodes[1])
        .await?;
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    let entry_after:Value=sqlx::query_scalar("SELECT checkpoint_json FROM runtime_module_checkpoints WHERE server_id=$1 AND module='singbox'").bind(fixture.servers[0]).fetch_one(&fixture.panel.state.pool).await?;
    let middle_after: i64 = sqlx::query_scalar(
        "SELECT target_rev FROM server_module_status WHERE server_id=$1 AND module='singbox'",
    )
    .bind(fixture.servers[1])
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert_eq!(
        entry_before, entry_after,
        "the fixture must hold entry activation and revision unchanged"
    );
    assert!(middle_after > middle_before);
    assert!(
        subscription_nodes(&subscription(&fixture, &user).await?).is_empty(),
        "the old entry probe cannot certify a new dependency vector"
    );
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let payload: Value = sqlx::query_scalar(
        "SELECT request_json FROM runtime_control_requests WHERE kind='probe' AND state='pending'",
    )
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    let request: RuntimePathProbeRequest = serde_json::from_value(payload)?;
    assert_ne!(request.request_id, original);
    assert_eq!(json!(request.expected), entry_after);
    finish_controls(&fixture.panel.state, true).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_eq!(phase(&fixture, chain).await?, "applied");
    assert_eq!(
        subscription_nodes(&subscription(&fixture, &user).await?).len(),
        1
    );
    Ok(())
}

async fn observed_vector(fixture: &Fixture) -> Result<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_array(server_id,checkpoint_json) ORDER BY server_id) FROM runtime_module_checkpoints WHERE module='singbox' AND server_id=ANY($1)")
        .bind(&fixture.servers).fetch_one(&fixture.panel.state.pool).await?)
}

async fn change_middle_configuration(fixture: &Fixture, name: &str) -> Result<()> {
    let user = fixture.panel.create_user(&fixture.cookie, name).await?;
    fixture
        .panel
        .grant(&fixture.cookie, id(&user)?, fixture.nodes[1])
        .await?;
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await
}

#[sqlx::test(migrations = "./migrations")]
async fn orphan_probe_request_cannot_be_reassociated_with_a_later_complete_vector(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", true).await?)?;
    let user = authorize(&fixture, chain).await?;
    advance(&fixture, chain, "applied").await?;
    let (original,probe_id,original_vector):(Uuid,Uuid,Value)=sqlx::query_as("SELECT request_id,probe_id,dependency_vector FROM singbox_path_probes WHERE chain_id=$1 AND generation=1 AND stage='switched'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    change_middle_configuration(&fixture, "TEST_ONLY first middle change").await?;
    let first_vector = observed_vector(&fixture).await?;
    assert_ne!(first_vector, original_vector);
    // TEST_ONLY crash window from the former convenience call: the generic queue
    // commits and can notify the device while the plugin still owns R0/V0.
    let orphan = runtime_control::request_path_probe(
        &fixture.panel.state,
        fixture.servers[0],
        "singbox",
        probe_id,
    )
    .await?;
    assert_ne!(orphan.request_id, original);
    change_middle_configuration(&fixture, "TEST_ONLY second middle change").await?;
    let later_vector = observed_vector(&fixture).await?;
    assert_ne!(later_vector, first_vector);
    assert_eq!(
        later_vector[0][1],
        json!(orphan.expected),
        "entry activation stays fixed across both managed changes"
    );
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let waiting:(Uuid,Value)=sqlx::query_as("SELECT request_id,dependency_vector FROM singbox_path_probes WHERE chain_id=$1 AND generation=1 AND stage='switched'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_eq!(
        waiting,
        (original, original_vector.clone()),
        "another opaque pending request must not be adopted"
    );
    assert!(subscription_nodes(&subscription(&fixture, &user).await?).is_empty());
    let ack = runtime_control::record_path_probe_result(
        &fixture.panel.state,
        fixture.servers[0],
        RuntimePathProbeResult {
            request_id: orphan.request_id,
            request_digest: orphan.digest()?,
            observed: Some(orphan.expected.clone()),
            probe_id,
            elapsed_ms: Some(17),
            success: true,
            error: None,
        },
    )
    .await?;
    assert_eq!(ack.request_id, orphan.request_id);
    let old_outcome: String =
        sqlx::query_scalar("SELECT outcome FROM runtime_control_receipts WHERE request_id=$1")
            .bind(orphan.request_id)
            .fetch_one(&fixture.panel.state.pool)
            .await?;
    assert_eq!(
        old_outcome, "verified",
        "generic entry-only verification can legitimately accept the delayed old result"
    );
    assert!(subscription_nodes(&subscription(&fixture, &user).await?).is_empty());
    let restarted = AppState::new(
        fixture.panel.state.pool.clone(),
        (*fixture.panel.state.config).clone(),
    )
    .await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&restarted).await?;
    let (fresh,fresh_vector,state):(Uuid,Value,String)=sqlx::query_as("SELECT request_id,dependency_vector,state FROM singbox_path_probes WHERE chain_id=$1 AND generation=1 AND stage='switched'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_ne!(fresh, original);
    assert_ne!(fresh, orphan.request_id);
    assert_eq!(fresh_vector, later_vector);
    assert_eq!(state, "pending");
    assert!(
        subscription_nodes(&subscription(&fixture, &user).await?).is_empty(),
        "an old verified receipt cannot certify V2"
    );
    finish_controls(&restarted, true).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&restarted).await?;
    assert_eq!(
        subscription_nodes(&subscription(&fixture, &user).await?).len(),
        1
    );
    let current:(Uuid,Value,String)=sqlx::query_as("SELECT request_id,dependency_vector,state FROM singbox_path_probes WHERE chain_id=$1 AND generation=1 AND stage='switched'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_eq!(current, (fresh, later_vector, "verified".to_owned()));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn orphan_barrier_and_later_vector_cannot_retire_old_identities_before_new_proof(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", true).await?)?;
    let user = authorize(&fixture, chain).await?;
    advance(&fixture, chain, "applied").await?;
    api(&fixture.panel,&fixture.cookie,Method::PATCH,&format!("/ordered-subscription-sources/{}",fixture.source),Some(json!({
        "request_id":Uuid::new_v4(),"settings_revision":1,"input":{"kind":"inline","content":content("TEST_ONLY changed source secret"),"identity_action":"update"}
    })),StatusCode::OK).await?;
    apsp_panel::plugins::singbox::subscription_sources::worker::run_once(&fixture.panel.state)
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
    let view = resource(&fixture, chain).await?;
    api(&fixture.panel,&fixture.cookie,Method::POST,&format!("/ordered-proxy-resources/chain/{chain}/apply-node-versions"),Some(json!({
        "request_id":Uuid::new_v4(),"settings_revision":view["settings_revision"],"generation":1,
        "versions":[{"hop_position":2,"node_version_id":nodes["nodes"][0]["version_id"]}]
    })),StatusCode::OK).await?;
    advance(&fixture, chain, "fixing_barrier").await?;
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    let proved_vector = observed_vector(&fixture).await?;
    let (switched,probe_vector):(Uuid,Value)=sqlx::query_as("SELECT request_id,dependency_vector FROM singbox_path_probes WHERE chain_id=$1 AND generation=2 AND stage='switched'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_eq!(probe_vector, proved_vector);
    let entry: RuntimeCheckpoint = serde_json::from_value(proved_vector[0][1].clone())?;
    // TEST_ONLY orphan of the first barrier: no plugin association exists yet.
    let orphan = runtime_control::request_barrier(
        &fixture.panel.state,
        fixture.servers[0],
        "singbox",
        entry.binding.revision,
    )
    .await?;
    let saved:Option<Uuid>=sqlx::query_scalar("SELECT barrier_request_id FROM singbox_path_stage_deployments WHERE chain_id=$1 AND generation=2 AND stage='switching_entry' AND role='entry'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert!(saved.is_none());
    change_middle_configuration(&fixture, "TEST_ONLY middle changed after switched proof").await?;
    let later_vector = observed_vector(&fixture).await?;
    assert_ne!(later_vector, proved_vector);
    assert_eq!(later_vector[0][1], json!(orphan.expected));
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_eq!(
        phase(&fixture, chain).await?,
        "probing_switched",
        "first barrier must recheck the entire switched proof even when association is absent"
    );
    let retained:i64=sqlx::query_scalar("SELECT count(*) FROM singbox_path_deployment_dependencies d JOIN server_module_status m ON m.server_id=d.server_id AND m.module=d.module AND m.target_rev=d.revision WHERE d.chain_id=$1 AND d.generation=1 AND d.role='managed'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_eq!(
        retained, 2,
        "old managed identities remain in the published vector"
    );
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    let current_vector = observed_vector(&fixture).await?;
    assert_eq!(
        current_vector, later_vector,
        "republishing the probing phase must preserve the complete later vector"
    );
    assert_eq!(current_vector[0][1], json!(orphan.expected));
    runtime_control::record_barrier_result(
        &fixture.panel.state,
        fixture.servers[0],
        RuntimeRecoveryBarrierResult {
            request_id: orphan.request_id,
            request_digest: orphan.digest()?,
            observed: Some(orphan.expected.clone()),
            minimum_revision: Some(orphan.minimum_revision),
            pending_intents_clear: true,
            success: true,
            error: None,
        },
    )
    .await?;
    let outcome: String =
        sqlx::query_scalar("SELECT outcome FROM runtime_control_receipts WHERE request_id=$1")
            .bind(orphan.request_id)
            .fetch_one(&fixture.panel.state.pool)
            .await?;
    assert_eq!(outcome, "verified");
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let (minimum, applied): (i64, Option<i64>) = sqlx::query_as(
        "SELECT minimum_generation,applied_generation FROM singbox_chains WHERE id=$1",
    )
    .bind(chain)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert_eq!(
        minimum, 2,
        "the committed old device floor is retained as fact"
    );
    assert_eq!(applied, Some(1));
    assert_eq!(phase(&fixture, chain).await?, "probing_switched");
    let (new_probe,new_vector):(Uuid,Value)=sqlx::query_as("SELECT request_id,dependency_vector FROM singbox_path_probes WHERE chain_id=$1 AND generation=2 AND stage='switched'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_ne!(new_probe, switched);
    assert_eq!(new_vector, later_vector);
    assert!(subscription_nodes(&subscription(&fixture, &user).await?).is_empty());
    finish_controls(&fixture.panel.state, true).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_eq!(phase(&fixture, chain).await?, "fixing_barrier");
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    let barrier_vector = observed_vector(&fixture).await?;
    assert_eq!(
        barrier_vector, later_vector,
        "republishing the barrier phase must preserve the newly proved complete vector"
    );
    assert_eq!(barrier_vector[0][1], json!(orphan.expected));
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let (fresh,fresh_vector):(Uuid,Value)=sqlx::query_as("SELECT barrier_request_id,barrier_vector FROM singbox_path_stage_deployments WHERE chain_id=$1 AND generation=2 AND stage='switching_entry' AND role='entry'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_ne!(fresh, orphan.request_id);
    assert_eq!(fresh_vector, later_vector);
    assert_eq!(phase(&fixture, chain).await?, "fixing_barrier");
    finish_controls(&fixture.panel.state, true).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_eq!(
        phase(&fixture, chain).await?,
        "retiring_old",
        "only the new proof and its atomically associated barrier permit cleanup"
    );
    advance(&fixture, chain, "applied").await?;
    assert_eq!(
        subscription_nodes(&subscription(&fixture, &user).await?).len(),
        1
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn an_unknown_orphan_barrier_floor_blocks_rollback_after_a_new_vector_probe_fails(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", true).await?)?;
    let user = authorize(&fixture, chain).await?;
    advance(&fixture, chain, "applied").await?;
    api(&fixture.panel,&fixture.cookie,Method::PATCH,&format!("/ordered-subscription-sources/{}",fixture.source),Some(json!({
        "request_id":Uuid::new_v4(),"settings_revision":1,"input":{"kind":"inline","content":content("TEST_ONLY unknown barrier source change"),"identity_action":"update"}
    })),StatusCode::OK).await?;
    apsp_panel::plugins::singbox::subscription_sources::worker::run_once(&fixture.panel.state)
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
    let view = resource(&fixture, chain).await?;
    api(&fixture.panel,&fixture.cookie,Method::POST,&format!("/ordered-proxy-resources/chain/{chain}/apply-node-versions"),Some(json!({
        "request_id":Uuid::new_v4(),"settings_revision":view["settings_revision"],"generation":1,
        "versions":[{"hop_position":2,"node_version_id":nodes["nodes"][0]["version_id"]}]
    })),StatusCode::OK).await?;
    advance(&fixture, chain, "fixing_barrier").await?;
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    let vector = observed_vector(&fixture).await?;
    let entry: RuntimeCheckpoint = serde_json::from_value(vector[0][1].clone())?;
    // TEST_ONLY the command may already have committed its device floor, but its
    // delayed durable receipt gives the panel no authority to claim that fact yet.
    let orphan = runtime_control::request_barrier(
        &fixture.panel.state,
        fixture.servers[0],
        "singbox",
        entry.binding.revision,
    )
    .await?;
    change_middle_configuration(&fixture, "TEST_ONLY unconfirmed floor middle change").await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_eq!(phase(&fixture, chain).await?, "probing_switched");
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let request:Value=sqlx::query_scalar("SELECT q.request_json FROM singbox_path_probes p JOIN runtime_control_requests q ON q.request_id=p.request_id WHERE p.chain_id=$1 AND p.generation=2 AND p.stage='switched' AND p.state='pending'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    let request: RuntimePathProbeRequest = serde_json::from_value(request)?;
    runtime_control::record_path_probe_result(
        &fixture.panel.state,
        fixture.servers[0],
        RuntimePathProbeResult {
            request_id: request.request_id,
            request_digest: request.digest()?,
            observed: Some(request.expected.clone()),
            probe_id: request.probe_id,
            elapsed_ms: None,
            success: false,
            error: Some("TEST_ONLY subsequent vector could not be confirmed".into()),
        },
    )
    .await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let guarded:(String,i64,Option<i64>,Option<i64>,Option<i64>)=sqlx::query_as("SELECT phase,minimum_generation,applied_generation,candidate_generation,recovery_generation FROM singbox_chains WHERE id=$1")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_eq!(
        guarded,
        ("fixing_barrier".to_owned(), 1, Some(1), Some(2), Some(1)),
        "unknown floor permits neither rollback nor an invented committed-generation fact"
    );
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    let active:Vec<i64>=sqlx::query_scalar("SELECT d.generation FROM singbox_path_deployment_dependencies d JOIN server_module_status m ON m.server_id=d.server_id AND m.module=d.module AND m.target_rev=d.revision WHERE d.chain_id=$1 AND d.role='entry' AND d.route_active")
        .bind(chain).fetch_all(&fixture.panel.state.pool).await?;
    assert_eq!(
        active,
        vec![2],
        "the possibly committed candidate remains the only routed generation"
    );
    let retained:i64=sqlx::query_scalar("SELECT count(*) FROM singbox_path_deployment_dependencies d JOIN server_module_status m ON m.server_id=d.server_id AND m.module=d.module AND m.target_rev=d.revision WHERE d.chain_id=$1 AND d.generation=1 AND d.role='managed'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_eq!(
        retained, 2,
        "both prior managed identities remain until the ambiguity is resolved"
    );
    let received: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_control_receipts WHERE request_id=$1)",
    )
    .bind(orphan.request_id)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert!(!received);
    assert!(subscription_nodes(&subscription(&fixture, &user).await?).is_empty());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_superseded_probe_keeps_its_history_and_requires_a_new_request_without_failing_the_path(
    pool: PgPool,
) -> Result<()> {
    let fixture = fixture(pool).await?;
    let chain = chain(&create(&fixture, "pinned", true).await?)?;
    let user = authorize(&fixture, chain).await?;
    advance(&fixture, chain, "applied").await?;
    change_middle_configuration(&fixture, "TEST_ONLY current vector refresh").await?;
    let current_vector = observed_vector(&fixture).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let raw:Value=sqlx::query_scalar("SELECT q.request_json FROM singbox_path_probes p JOIN runtime_control_requests q ON q.request_id=p.request_id WHERE p.chain_id=$1 AND p.generation=1 AND p.stage='switched' AND p.state='pending'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    let request: RuntimePathProbeRequest = serde_json::from_value(raw)?;
    // A real metadata mutation dirties all referenced servers, without changing
    // the frozen endpoint, native bytes or live activation. This also occurs when
    // another path advances on the same entry server while a proof is in flight.
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::PATCH,
        &format!("/nodes/{}", fixture.nodes[1]),
        Some(json!({"name":"TEST_ONLY display changed during confirmation"})),
        StatusCode::OK,
    )
    .await?;
    runtime_control::record_path_probe_result(
        &fixture.panel.state,
        fixture.servers[0],
        RuntimePathProbeResult {
            request_id: request.request_id,
            request_digest: request.digest()?,
            observed: Some(request.expected.clone()),
            probe_id: request.probe_id,
            elapsed_ms: Some(17),
            success: true,
            error: None,
        },
    )
    .await?;
    let historical: (String, Value, i64) = sqlx::query_as(
        "SELECT outcome,result_json,received_at FROM runtime_control_receipts WHERE request_id=$1",
    )
    .bind(request.request_id)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert_eq!(historical.0, "superseded");
    assert_eq!(
        historical.1["success"], true,
        "the device did not report a native network failure"
    );
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    assert_eq!(
        observed_vector(&fixture).await?,
        current_vector,
        "metadata-only dirtiness did not change the exact activation vector"
    );
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_eq!(
        phase(&fixture, chain).await?,
        "applied",
        "superseded confirmation must not masquerade as a path failure"
    );
    let cleared:(Option<Uuid>,Option<Value>,Option<i64>,String)=sqlx::query_as("SELECT request_id,dependency_vector,revision,state FROM singbox_path_probes WHERE chain_id=$1 AND generation=1 AND stage='switched'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_eq!(cleared, (None, None, None, "pending".to_owned()));
    assert!(
        subscription_nodes(&subscription(&fixture, &user).await?).is_empty(),
        "a superseded receipt cannot preserve current qualification"
    );
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    let (fresh,fresh_vector):(Uuid,Value)=sqlx::query_as("SELECT request_id,dependency_vector FROM singbox_path_probes WHERE chain_id=$1 AND generation=1 AND stage='switched'")
        .bind(chain).fetch_one(&fixture.panel.state.pool).await?;
    assert_ne!(fresh, request.request_id);
    assert_eq!(fresh_vector, current_vector);
    assert!(subscription_nodes(&subscription(&fixture, &user).await?).is_empty());
    finish_controls(&fixture.panel.state, true).await?;
    apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    assert_eq!(phase(&fixture, chain).await?, "applied");
    assert_eq!(
        subscription_nodes(&subscription(&fixture, &user).await?).len(),
        1
    );
    let retained: (String, Value, i64) = sqlx::query_as(
        "SELECT outcome,result_json,received_at FROM runtime_control_receipts WHERE request_id=$1",
    )
    .bind(request.request_id)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    assert_eq!(
        retained, historical,
        "new qualification never rewrites the old request receipt or receive time"
    );
    Ok(())
}
