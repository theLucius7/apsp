use super::*;

const PROBE: Uuid = Uuid::from_u128(700);
const ANOTHER: Uuid = Uuid::from_u128(701);

async fn probe_fixture(pool: PgPool) -> Result<(TestPanel, i64)> {
    let (panel, server) = fixture(pool).await?;
    agent_api::process_message(
        &panel.state,
        server,
        Message::Hello(Hello {
            agent_version: "TEST_ONLY".into(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![
                RUNTIME_CHECKPOINT_CAPABILITY.into(),
                RUNTIME_RECOVERY_BARRIER_CAPABILITY.into(),
                RUNTIME_PATH_PROBE_CAPABILITY.into(),
            ],
            applied: BTreeMap::new(),
        }),
    )
    .await?;
    let bundle = Bundle { files: BTreeMap::from([
        ("config.json".into(), "TEST_ONLY native configuration".into()),
        ("runtime-probes.json".into(), json!({"schema":1,"runtime_version":"1.14.2-apsp.1",
            "required_build_tags":["with_clash_api","with_v2ray_api"],
            "bindings":[{"id":PROBE,"selector":"path_1_2_0","target":"https://panel.example/health"},
                {"id":ANOTHER,"selector":"path_1_3_0","target":"https://panel.example/health"}]}).to_string()),
    ]) };
    sqlx::query("UPDATE deployments SET bundle=$3 WHERE server_id=$1 AND module=$2 AND rev=1")
        .bind(server)
        .bind(MODULE)
        .bind(serde_json::to_string(&bundle)?)
        .execute(&panel.state.pool)
        .await?;
    confirmed(&panel.state, server).await?;
    Ok((panel, server))
}

fn probe_success(request: &RuntimePathProbeRequest) -> RuntimePathProbeResult {
    RuntimePathProbeResult {
        request_id: request.request_id,
        request_digest: request.digest().unwrap(),
        observed: Some(request.expected.clone()),
        probe_id: request.probe_id,
        elapsed_ms: Some(17),
        success: true,
        error: None,
    }
}

#[sqlx::test]
async fn signed_probe_replays_original_deadline_and_acknowledges_only_committed_immutable_receipts(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = probe_fixture(pool).await?;
    let request = runtime_control::request_path_probe(&panel.state, server, MODULE, PROBE).await?;
    assert_eq!(
        runtime_control::request_path_probe(&panel.state, server, MODULE, PROBE).await?,
        request
    );
    assert!(
        runtime_control::request_path_probe(&panel.state, server, MODULE, ANOTHER)
            .await
            .is_err()
    );
    let restarted = AppState::new(panel.state.pool.clone(), (*panel.state.config).clone()).await?;
    let mut receiver = attach(&restarted, server).await;
    runtime_control::replay_pending(&restarted, server).await?;
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimePathProbeRequest(request.clone())
    );
    let result = probe_success(&request);
    agent_api::process_message(
        &restarted,
        server,
        Message::RuntimePathProbeResult(result.clone()),
    )
    .await?;
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimePathProbeAck(RuntimeControlAck {
            request_id: request.request_id,
            request_digest: request.digest()?,
        })
    );
    assert_eq!(
        runtime_control::path_probe_fact(&restarted, server, request.request_id).await?,
        Some(("verified".into(), result.clone()))
    );
    runtime_control::record_path_probe_result(&restarted, server, result.clone()).await?;
    let mut conflicting = result;
    conflicting.elapsed_ms = Some(18);
    assert!(
        runtime_control::record_path_probe_result(&restarted, server, conflicting)
            .await
            .is_err()
    );
    let floor: i64 = sqlx::query_scalar(
        "SELECT minimum_revision FROM runtime_module_checkpoints WHERE server_id=$1 AND module=$2",
    )
    .bind(server)
    .bind(MODULE)
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(floor, 0);
    assert_eq!(health(&panel.state.pool, server).await?, (1, true));
    Ok(())
}

#[sqlx::test]
async fn wrong_probe_activation_digest_or_device_never_promotes_candidate_evidence(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = probe_fixture(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let other = panel.create_server(&cookie, "其他设备").await?;
    let request = runtime_control::request_path_probe(&panel.state, server, MODULE, PROBE).await?;
    assert!(
        runtime_control::record_path_probe_result(&panel.state, other, probe_success(&request))
            .await
            .is_err()
    );
    assert!(
        runtime_control::path_probe_fact(&panel.state, other, request.request_id)
            .await?
            .is_none()
    );
    let mut wrong = probe_success(&request);
    wrong.request_digest = "b".repeat(64);
    assert!(
        runtime_control::record_path_probe_result(&panel.state, server, wrong)
            .await
            .is_err()
    );
    let mut wrong = probe_success(&request);
    wrong.probe_id = ANOTHER;
    runtime_control::record_path_probe_result(&panel.state, server, wrong).await?;
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "mismatch"
    );
    let request = runtime_control::request_path_probe(&panel.state, server, MODULE, PROBE).await?;
    let mut wrong = probe_success(&request);
    wrong.observed.as_mut().unwrap().activation_id = Uuid::new_v4();
    runtime_control::record_path_probe_result(&panel.state, server, wrong).await?;
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "mismatch"
    );
    assert_eq!(health(&panel.state.pool, server).await?, (1, true));
    Ok(())
}

#[sqlx::test]
async fn late_probe_or_runtime_change_is_retained_without_false_verification(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = probe_fixture(pool).await?;
    let mut request =
        runtime_control::request_path_probe(&panel.state, server, MODULE, PROBE).await?;
    request.expires_at = now_timestamp() - 1;
    sqlx::query("UPDATE runtime_control_requests SET created_at=$2-120,expires_at=$2,request_json=$3,request_digest=$4 WHERE request_id=$1")
        .bind(request.request_id).bind(request.expires_at).bind(json!(request)).bind(request.digest()?).execute(&panel.state.pool).await?;
    runtime_control::record_path_probe_result(&panel.state, server, probe_success(&request))
        .await?;
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "late"
    );
    let request = runtime_control::request_path_probe(&panel.state, server, MODULE, PROBE).await?;
    let checkpoint = runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    let mut changed = success(&checkpoint);
    changed.observed.as_mut().unwrap().activation_id = Uuid::new_v4();
    runtime_control::record_checkpoint_result(&panel.state, server, changed).await?;
    runtime_control::record_path_probe_result(&panel.state, server, probe_success(&request))
        .await?;
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "mismatch"
    );
    let request = runtime_control::request_path_probe(&panel.state, server, MODULE, PROBE).await?;
    sqlx::query("UPDATE servers SET dirty_at=$2 WHERE id=$1")
        .bind(server)
        .bind(now_timestamp())
        .execute(&panel.state.pool)
        .await?;
    runtime_control::record_path_probe_result(&panel.state, server, probe_success(&request))
        .await?;
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "superseded"
    );
    Ok(())
}

#[sqlx::test]
async fn missing_capability_unknown_binding_and_unconfirmed_runtime_cannot_issue_probe(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = probe_fixture(pool).await?;
    assert!(
        runtime_control::request_path_probe(&panel.state, server, MODULE, Uuid::new_v4())
            .await
            .is_err()
    );
    assert!(
        runtime_control::request_path_probe(&panel.state, server, MODULE, Uuid::nil())
            .await
            .is_err()
    );
    introduce(&panel.state, server, true, BTreeMap::new()).await?;
    assert!(
        runtime_control::request_path_probe(&panel.state, server, MODULE, PROBE)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM runtime_control_requests WHERE kind='probe'"
        )
        .fetch_one(&panel.state.pool)
        .await?,
        0
    );
    Ok(())
}
