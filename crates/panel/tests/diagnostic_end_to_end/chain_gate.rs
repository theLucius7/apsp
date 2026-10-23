use super::*;

#[sqlx::test(migrations = "./migrations")]
async fn saved_preparing_full_is_refused_after_upgrade_without_starting_a_service(
    pool: PgPool,
) -> Result<()> {
    let harness = Harness::start(pool).await?;
    write_artifact(&harness)?;
    let server = harness
        .api(
            Method::POST,
            "/api/servers",
            json!({"name":"已领取任务门禁夹具"}),
        )
        .await?;
    let server_id = server["id"].as_i64().context("server id")?;
    let enrollment = harness
        .api(
            Method::POST,
            &format!("/api/servers/{server_id}/enrollment"),
            json!({}),
        )
        .await?;
    let config = harness.agent_config();
    identity::enroll(
        &config,
        enrollment["token"].as_str().context("enrollment token")?,
    )
    .await?;
    // A resumed Preparing task verifies the signed host architecture before adapter preparation.
    let artifact = apsp_panel::artifacts::descriptor(
        &harness.state,
        "nodequality",
        VERSION,
        apsp_protocol::release::native_arch()?,
    )
    .await?;
    let now = apsp_protocol::now_timestamp();
    let job = apsp_protocol::DiagnosticJob {
        id: uuid::Uuid::new_v4(),
        plugin: "nodequality".into(),
        version: VERSION.into(),
        artifact,
        timeout_secs: 1800,
        expires_at: Some(now + 2100),
        resource_budget: None,
        options: BTreeMap::new(),
    };
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,job,status,created_at,updated_at,expires_at) VALUES($1,$2,$3,'queued',$4,$4,$5)")
        .bind(job.id).bind(server_id).bind(serde_json::to_value(&job)?).bind(now)
        .bind(now + 2100).execute(&harness.state.pool).await?;
    // A legacy Agent had already received this HTTP response before rollout.
    // The upgraded Agent must reject its durable Preparing state as well.
    State::open(&config.state_db)?.set_json("diagnostics:active", &json!({"Preparing": job}))?;
    // Receiving that signed job required this previously published capability.
    // A fresh enrollment otherwise races Hello persistence with the resumed
    // artifact GET and can fail at HTTP 409 before reaching the full-mode gate.
    sqlx::query("UPDATE servers SET capabilities=$2 WHERE id=$1")
        .bind(server_id)
        .bind(json!([
            apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY
        ]))
        .execute(&harness.state.pool)
        .await?;
    let services = Arc::new(IndependentServices::default());
    let agent = tokio::spawn(transport::run_with_diagnostics(
        config.clone(),
        vec![],
        vec![Arc::new(NodeQualityAdapter::new())],
        Arc::new(FakeResourceOps::new(Arc::new(SystemOps))),
        services.clone(),
        "full-gate-upgrade-fixture",
    ));
    let acknowledgement = eventually(
        "preparing task refused and acknowledged after upgrade",
        20,
        || async {
            anyhow::ensure!(
                !agent.is_finished(),
                "Agent exited during gate acknowledgement"
            );
            let row: (String, Option<String>, bool) = sqlx::query_as(
                "SELECT status,error,agent_completed FROM diagnostic_jobs WHERE id=$1",
            )
            .bind(job.id)
            .fetch_one(&harness.state.pool)
            .await?;
            Ok(row.0 == "failed"
                && row.2
                && row.1.is_some_and(|error| error.contains("离线受控工具链")))
        },
    )
    .await;
    agent.abort();
    let _ = agent.await;
    if let Err(error) = acknowledgement {
        let state_db = config.state_db.clone();
        let evidence = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let row: (String, Option<String>, bool, serde_json::Value) = sqlx::query_as(
                "SELECT j.status,j.error,j.agent_completed,s.capabilities FROM diagnostic_jobs j JOIN servers s ON s.id=j.server_id WHERE j.id=$1",
            )
            .bind(job.id)
            .fetch_one(&harness.state.pool)
            .await?;
            let local = tokio::task::spawn_blocking(move || -> Result<serde_json::Value> {
                let state = State::open(&state_db)?;
                Ok(json!({
                    "active": state.get_json::<serde_json::Value>("diagnostics:active")?,
                    "outbox": state.get_json::<serde_json::Value>("diagnostics:outbox")?,
                    "done": state.get_json::<bool>(&format!("diagnostics:done:{}", job.id))?,
                }))
            })
            .await
            .context("read gate fixture state")??;
            Ok::<_, anyhow::Error>(json!({
                "panel": {"status": row.0, "error": row.1, "agent_completed": row.2, "capabilities": row.3},
                "agent": local,
            }))
        })
        .await;
        let context = match evidence {
            Ok(Ok(evidence)) => format!("gate fixture observation: {evidence}"),
            Ok(Err(error)) => format!("gate fixture observation failed: {error:#}"),
            Err(_) => "gate fixture observation exceeded its two-second budget".into(),
        };
        return Err(error.context(context));
    }
    assert_eq!(services.starts.load(Ordering::SeqCst), 0);
    assert_eq!(services.stops.load(Ordering::SeqCst), 0);
    assert!(
        !config
            .runtime_root
            .join("diagnostics")
            .join(job.id.to_string())
            .exists()
    );
    assert!(
        State::open(&config.state_db)?
            .get_json::<serde_json::Value>("diagnostics:active")?
            .is_none_or(|checkpoint| checkpoint.is_null())
    );
    Ok(())
}
