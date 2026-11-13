use super::*;

#[sqlx::test]
async fn exact_devices_require_observation_and_cannot_downgrade_to_legacy_trust(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = fixture(pool).await?;
    device_channel::process_message(
        &panel.state,
        server,
        Message::Heartbeat(Heartbeat {
            applied: BTreeMap::from([(MODULE.into(), 1)]),
            uptime_secs: 1,
        }),
    )
    .await?;
    assert_eq!(health(&panel.state.pool, server).await?, (0, false));
    let request = pending(&panel.state.pool, server).await?;
    apsp_traffic::device_config::record_apply_result(&panel.state, server, applied(1)).await?;
    assert_eq!(pending(&panel.state.pool, server).await?, request);
    assert_eq!(health(&panel.state.pool, server).await?, (0, false));
    runtime_control::record_checkpoint_result(&panel.state, server, success(&request)).await?;
    assert_eq!(health(&panel.state.pool, server).await?, (1, true));
    let before_capability_loss =
        runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    introduce(
        &panel.state,
        server,
        false,
        BTreeMap::from([(MODULE.into(), 1)]),
    )
    .await?;
    assert!(runtime_control::checkpoint_required(&panel.state, server).await?);
    assert_eq!(health(&panel.state.pool, server).await?, (1, false));
    runtime_control::record_checkpoint_result(
        &panel.state,
        server,
        success(&before_capability_loss),
    )
    .await?;
    assert_eq!(
        outcome(&panel.state.pool, before_capability_loss.request_id).await?,
        "superseded"
    );
    assert_eq!(health(&panel.state.pool, server).await?, (1, false));
    assert!(
        runtime_control::request_checkpoint(&panel.state, server, MODULE)
            .await
            .is_err()
    );
    Ok(())
}

#[sqlx::test]
async fn legacy_revision_acknowledgement_remains_separate(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let server = panel
        .create_server(&panel.admin_cookie().await?, "旧设备")
        .await?;
    publish(&panel.state.pool, server, 1).await?;
    introduce(
        &panel.state,
        server,
        false,
        BTreeMap::from([(MODULE.into(), 1)]),
    )
    .await?;
    assert_eq!(health(&panel.state.pool, server).await?, (1, true));
    assert!(!runtime_control::checkpoint_required(&panel.state, server).await?);
    assert_eq!(
        count(&panel.state.pool, "runtime_control_requests").await?,
        0
    );
    introduce(
        &panel.state,
        server,
        true,
        BTreeMap::from([(MODULE.into(), 1)]),
    )
    .await?;
    assert_eq!(health(&panel.state.pool, server).await?, (1, false));
    assert_eq!(
        pending(&panel.state.pool, server).await?.expected.revision,
        1
    );
    Ok(())
}

#[sqlx::test]
async fn request_replay_survives_panel_restart_and_receipt_ack_is_durable(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = fixture(pool).await?;
    let request = runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    let restarted =
        apsp_panel::state(panel.state.pool.clone(), (*panel.state.config).clone()).await?;
    let mut receiver = attach(&restarted, server).await;
    runtime_control::replay_pending(&restarted, server).await?;
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimeCheckpointRequest(request.clone())
    );
    device_channel::process_message(
        &restarted,
        server,
        Message::RuntimeCheckpointResult(success(&request)),
    )
    .await?;
    let ack = RuntimeControlAck {
        request_id: request.request_id,
        request_digest: request.digest()?,
    };
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimeCheckpointAck(ack.clone())
    );
    assert_eq!(
        count(&panel.state.pool, "runtime_control_receipts").await?,
        1
    );
    runtime_control::replay_pending(&restarted, server).await?;
    assert!(receiver.try_recv().is_err());
    // Losing the first ACK does not lose the receipt or change its original receive time.
    let received: i64 =
        sqlx::query_scalar("SELECT received_at FROM runtime_control_receipts WHERE request_id=$1")
            .bind(request.request_id)
            .fetch_one(&panel.state.pool)
            .await?;
    device_channel::process_message(
        &restarted,
        server,
        Message::RuntimeCheckpointResult(success(&request)),
    )
    .await?;
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimeCheckpointAck(ack)
    );
    assert_eq!(
        count(&panel.state.pool, "runtime_control_receipts").await?,
        1
    );
    assert_eq!(
        received,
        sqlx::query_scalar::<_, i64>(
            "SELECT received_at FROM runtime_control_receipts WHERE request_id=$1"
        )
        .bind(request.request_id)
        .fetch_one(&panel.state.pool)
        .await?
    );
    let mut conflict = success(&request);
    conflict.observed.as_mut().unwrap().instance_id = "another-instance".into();
    assert!(
        runtime_control::record_checkpoint_result(&restarted, server, conflict)
            .await
            .is_err()
    );
    Ok(())
}

#[sqlx::test]
async fn late_and_superseded_success_are_historical_facts_without_health_promotion(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = fixture(pool).await?;
    let mut late = runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    expire(&panel.state.pool, &mut late).await?;
    let ack =
        runtime_control::record_checkpoint_result(&panel.state, server, success(&late)).await?;
    assert_eq!(ack.request_id, late.request_id);
    assert_eq!(outcome(&panel.state.pool, late.request_id).await?, "late");
    assert_eq!(health(&panel.state.pool, server).await?, (0, false));
    assert_eq!(
        count(&panel.state.pool, "runtime_module_checkpoints").await?,
        0
    );
    runtime_control::replay_pending(&panel.state, server).await?;
    assert_eq!(
        runtime_control::record_checkpoint_result(&panel.state, server, success(&late)).await?,
        ack
    );
    assert_eq!(
        count(&panel.state.pool, "runtime_control_receipts").await?,
        1
    );
    let old = runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    publish(&panel.state.pool, server, 2).await?;
    let current = runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    runtime_control::record_checkpoint_result(&panel.state, server, success(&current)).await?;
    runtime_control::record_checkpoint_result(&panel.state, server, success(&old)).await?;
    assert_eq!(
        outcome(&panel.state.pool, old.request_id).await?,
        "superseded"
    );
    assert_eq!(health(&panel.state.pool, server).await?, (2, true));
    apsp_traffic::device_config::record_apply_result(&panel.state, server, applied(1)).await?;
    assert_eq!(health(&panel.state.pool, server).await?, (2, true));
    assert_eq!(count(&panel.state.pool, "runtime_apply_facts").await?, 1);
    Ok(())
}

#[sqlx::test]
async fn wrong_digest_owner_binding_or_dirty_desire_cannot_mark_healthy(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = fixture(pool).await?;
    let request = runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    let mut wrong = success(&request);
    wrong.request_digest = "f".repeat(64);
    assert!(
        runtime_control::record_checkpoint_result(&panel.state, server, wrong)
            .await
            .is_err()
    );
    let other = panel
        .create_server(&panel.admin_cookie().await?, "另一设备")
        .await?;
    assert!(
        runtime_control::record_checkpoint_result(&panel.state, other, success(&request))
            .await
            .is_err()
    );
    assert_eq!(
        count(&panel.state.pool, "runtime_control_receipts").await?,
        0
    );
    let mut wrong = success(&request);
    wrong.observed.as_mut().unwrap().binding =
        RuntimeBinding::new(Uuid::new_v4(), MODULE.into(), 1, "a".repeat(64));
    runtime_control::record_checkpoint_result(&panel.state, server, wrong).await?;
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "mismatch"
    );
    assert_eq!(health(&panel.state.pool, server).await?, (0, false));
    let request = runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    sqlx::query("UPDATE servers SET dirty_at=$2 WHERE id=$1")
        .bind(server)
        .bind(now_timestamp())
        .execute(&panel.state.pool)
        .await?;
    runtime_control::record_checkpoint_result(&panel.state, server, success(&request)).await?;
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "superseded"
    );
    assert_eq!(health(&panel.state.pool, server).await?, (0, false));
    Ok(())
}

#[sqlx::test]
async fn successful_poll_is_bounded_but_failed_observation_can_be_reauthenticated(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = fixture(pool).await?;
    let apply = applied(1);
    apsp_traffic::device_config::record_apply_result(&panel.state, server, apply.clone()).await?;
    let request = pending(&panel.state.pool, server).await?;
    runtime_control::record_checkpoint_result(&panel.state, server, success(&request)).await?;
    for _ in 0..100 {
        apsp_traffic::device_config::record_apply_result(&panel.state, server, apply.clone())
            .await?;
        device_channel::process_message(
            &panel.state,
            server,
            Message::Heartbeat(Heartbeat {
                applied: BTreeMap::from([(MODULE.into(), 1)]),
                uptime_secs: 1,
            }),
        )
        .await?;
    }
    assert_eq!(
        count(&panel.state.pool, "runtime_control_requests").await?,
        1
    );
    assert_eq!(count(&panel.state.pool, "runtime_apply_facts").await?, 1);
    assert_eq!(health(&panel.state.pool, server).await?, (1, true));
    let check = runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    runtime_control::record_checkpoint_result(
        &panel.state,
        server,
        RuntimeCheckpointResult {
            request_id: check.request_id,
            request_digest: check.digest()?,
            observed: None,
            success: false,
            error: Some("runtime instance is no longer healthy".into()),
        },
    )
    .await?;
    assert_eq!(health(&panel.state.pool, server).await?, (1, false));
    apsp_traffic::device_config::record_apply_result(&panel.state, server, apply.clone()).await?;
    let retry = pending(&panel.state.pool, server).await?;
    assert_ne!(retry.request_id, check.request_id);
    runtime_control::record_checkpoint_result(&panel.state, server, success(&retry)).await?;
    assert_eq!(health(&panel.state.pool, server).await?, (1, true));
    let mut conflicting = apply;
    conflicting.status = ApplyStatus::Failed;
    conflicting.healthy = false;
    conflicting.error = Some("different fact".into());
    assert!(
        apsp_traffic::device_config::record_apply_result(&panel.state, server, conflicting)
            .await
            .is_err()
    );
    Ok(())
}
