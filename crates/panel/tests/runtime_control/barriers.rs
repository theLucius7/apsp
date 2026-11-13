use super::*;

#[sqlx::test]
async fn barrier_uses_current_verified_activation_and_persists_its_floor(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = fixture(pool).await?;
    assert!(
        runtime_control::request_barrier(&panel.state, server, MODULE, 1)
            .await
            .is_err()
    );
    let checkpoint = confirmed(&panel.state, server).await?;
    let request = runtime_control::request_barrier(&panel.state, server, MODULE, 1).await?;
    assert_eq!(request.expected, checkpoint);
    let repeated = runtime_control::request_barrier(&panel.state, server, MODULE, 1).await?;
    assert_eq!(request, repeated);
    let restarted =
        apsp_panel::state(panel.state.pool.clone(), (*panel.state.config).clone()).await?;
    let mut receiver = attach(&restarted, server).await;
    runtime_control::replay_pending(&restarted, server).await?;
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimeRecoveryBarrierRequest(request.clone())
    );
    device_channel::process_message(
        &restarted,
        server,
        Message::RuntimeRecoveryBarrierResult(barrier_success(&request)),
    )
    .await?;
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimeRecoveryBarrierAck(RuntimeControlAck {
            request_id: request.request_id,
            request_digest: request.digest()?
        })
    );
    let floor: i64 = sqlx::query_scalar(
        "SELECT minimum_revision FROM runtime_module_checkpoints WHERE server_id=$1 AND module=$2",
    )
    .bind(server)
    .bind(MODULE)
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(floor, 1);
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "verified"
    );
    let repeated =
        runtime_control::record_barrier_result(&restarted, server, barrier_success(&request))
            .await?;
    assert_eq!(repeated.request_id, request.request_id);
    assert_eq!(
        count(&panel.state.pool, "runtime_control_receipts").await?,
        2
    );
    assert!(
        runtime_control::request_barrier(&restarted, server, MODULE, 0)
            .await
            .is_err()
    );
    assert!(
        runtime_control::request_barrier(&restarted, server, MODULE, 2)
            .await
            .is_err()
    );
    Ok(())
}

#[sqlx::test]
async fn barrier_rejects_different_activation_and_insufficient_or_uncleared_floor(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = fixture(pool).await?;
    publish(&panel.state.pool, server, 2).await?;
    confirmed(&panel.state, server).await?;
    let request = runtime_control::request_barrier(&panel.state, server, MODULE, 2).await?;
    let mut wrong = barrier_success(&request);
    wrong.observed.as_mut().unwrap().activation_id = Uuid::from_u128(101);
    runtime_control::record_barrier_result(&panel.state, server, wrong).await?;
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "mismatch"
    );
    assert_eq!(health(&panel.state.pool, server).await?, (2, false));
    assert!(
        runtime_control::request_barrier(&panel.state, server, MODULE, 2)
            .await
            .is_err()
    );
    confirmed(&panel.state, server).await?;
    let request = runtime_control::request_barrier(&panel.state, server, MODULE, 2).await?;
    let mut wrong = barrier_success(&request);
    wrong.minimum_revision = Some(1);
    runtime_control::record_barrier_result(&panel.state, server, wrong).await?;
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "mismatch"
    );
    assert_eq!(health(&panel.state.pool, server).await?, (2, false));
    confirmed(&panel.state, server).await?;
    let request = runtime_control::request_barrier(&panel.state, server, MODULE, 2).await?;
    let mut failure = barrier_success(&request);
    failure.pending_intents_clear = false;
    failure.success = false;
    failure.error = Some("pending intent remains".into());
    runtime_control::record_barrier_result(&panel.state, server, failure).await?;
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "failed"
    );
    assert_eq!(health(&panel.state.pool, server).await?, (2, false));
    let floor: i64 = sqlx::query_scalar(
        "SELECT minimum_revision FROM runtime_module_checkpoints WHERE server_id=$1 AND module=$2",
    )
    .bind(server)
    .bind(MODULE)
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(floor, 0);
    Ok(())
}

#[sqlx::test]
async fn late_or_changed_checkpoint_barrier_is_saved_without_advancing_recovery(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = fixture(pool).await?;
    confirmed(&panel.state, server).await?;
    let mut request = runtime_control::request_barrier(&panel.state, server, MODULE, 1).await?;
    request.expires_at = now_timestamp() - 1;
    sqlx::query("UPDATE runtime_control_requests SET created_at=$2-120,expires_at=$2,request_json=$3,request_digest=$4 WHERE request_id=$1")
        .bind(request.request_id).bind(request.expires_at).bind(json!(request)).bind(request.digest()?).execute(&panel.state.pool).await?;
    let ack =
        runtime_control::record_barrier_result(&panel.state, server, barrier_success(&request))
            .await?;
    assert_eq!(ack.request_id, request.request_id);
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "late"
    );
    let floor: i64 = sqlx::query_scalar(
        "SELECT minimum_revision FROM runtime_module_checkpoints WHERE server_id=$1 AND module=$2",
    )
    .bind(server)
    .bind(MODULE)
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(floor, 0);
    let old = runtime_control::request_barrier(&panel.state, server, MODULE, 1).await?;
    let check = runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    let mut changed = success(&check);
    changed.observed.as_mut().unwrap().activation_id = Uuid::from_u128(102);
    changed.observed.as_mut().unwrap().instance_id = "backend:new-instance".into();
    runtime_control::record_checkpoint_result(&panel.state, server, changed).await?;
    runtime_control::record_barrier_result(&panel.state, server, barrier_success(&old)).await?;
    assert_eq!(
        outcome(&panel.state.pool, old.request_id).await?,
        "superseded"
    );
    assert_eq!(health(&panel.state.pool, server).await?, (1, true));
    let floor: i64 = sqlx::query_scalar(
        "SELECT minimum_revision FROM runtime_module_checkpoints WHERE server_id=$1 AND module=$2",
    )
    .bind(server)
    .bind(MODULE)
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(floor, 0);
    Ok(())
}

#[sqlx::test]
async fn confirmed_floor_never_decreases_across_a_new_activation(pool: PgPool) -> Result<()> {
    let (panel, server) = fixture(pool).await?;
    publish(&panel.state.pool, server, 2).await?;
    confirmed(&panel.state, server).await?;
    let barrier = runtime_control::request_barrier(&panel.state, server, MODULE, 2).await?;
    runtime_control::record_barrier_result(&panel.state, server, barrier_success(&barrier)).await?;
    let check = runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    let mut changed = success(&check);
    changed.observed.as_mut().unwrap().activation_id = Uuid::from_u128(103);
    runtime_control::record_checkpoint_result(&panel.state, server, changed).await?;
    let request = runtime_control::request_barrier(&panel.state, server, MODULE, 1).await?;
    assert_eq!(request.minimum_revision, 2);
    runtime_control::record_barrier_result(&panel.state, server, barrier_success(&request)).await?;
    let floor: i64 = sqlx::query_scalar(
        "SELECT minimum_revision FROM runtime_module_checkpoints WHERE server_id=$1 AND module=$2",
    )
    .bind(server)
    .bind(MODULE)
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(floor, 2);
    Ok(())
}

#[sqlx::test]
async fn unreceived_expired_barrier_replays_its_original_payload_until_a_durable_terminal_receipt(
    pool: PgPool,
) -> Result<()> {
    let (panel, server) = fixture(pool).await?;
    confirmed(&panel.state, server).await?;
    let mut request = runtime_control::request_barrier(&panel.state, server, MODULE, 1).await?;
    // TEST_ONLY simulate downtime past the original command's fixed deadline.
    request.expires_at = now_timestamp() - 1;
    let digest = request.digest()?;
    let payload = json!(request);
    sqlx::query("UPDATE runtime_control_requests SET created_at=$2-120,expires_at=$2,request_json=$3,request_digest=$4 WHERE request_id=$1")
        .bind(request.request_id).bind(request.expires_at).bind(&payload).bind(&digest).execute(&panel.state.pool).await?;
    let mut expired_checkpoint =
        runtime_control::request_checkpoint(&panel.state, server, MODULE).await?;
    expire(&panel.state.pool, &mut expired_checkpoint).await?;
    let restarted =
        apsp_panel::state(panel.state.pool.clone(), (*panel.state.config).clone()).await?;
    let mut receiver = attach(&restarted, server).await;
    runtime_control::replay_pending(&restarted, server).await?;
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimeRecoveryBarrierRequest(request.clone())
    );
    assert!(
        receiver.try_recv().is_err(),
        "expired checkpoints are not recovered as new work"
    );
    runtime_control::replay_pending(&restarted, server).await?;
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimeRecoveryBarrierRequest(request.clone())
    );
    let persisted:(Value,String,i64,String)=sqlx::query_as("SELECT request_json,request_digest,expires_at,state FROM runtime_control_requests WHERE request_id=$1")
        .bind(request.request_id).fetch_one(&panel.state.pool).await?;
    assert_eq!(
        persisted,
        (
            payload,
            digest.clone(),
            request.expires_at,
            "expired".to_owned()
        ),
        "replay never renews the deadline or changes the original request identity"
    );
    device_channel::process_message(
        &restarted,
        server,
        Message::RuntimeRecoveryBarrierResult(RuntimeRecoveryBarrierResult {
            request_id: request.request_id,
            request_digest: digest.clone(),
            observed: None,
            minimum_revision: None,
            pending_intents_clear: false,
            success: false,
            error: Some("TEST_ONLY original deadline expired before device execution".into()),
        }),
    )
    .await?;
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimeRecoveryBarrierAck(RuntimeControlAck {
            request_id: request.request_id,
            request_digest: digest,
        })
    );
    assert_eq!(
        outcome(&panel.state.pool, request.request_id).await?,
        "late"
    );
    runtime_control::replay_pending(&restarted, server).await?;
    assert!(
        receiver.try_recv().is_err(),
        "a durable receipt excludes further expired-command recovery"
    );
    confirmed(&panel.state, server).await?;
    let mut corrupted = runtime_control::request_barrier(&restarted, server, MODULE, 1).await?;
    assert_eq!(
        receive(&mut receiver).await?,
        Message::RuntimeRecoveryBarrierRequest(corrupted.clone())
    );
    corrupted.expires_at = now_timestamp() - 1;
    sqlx::query("UPDATE runtime_control_requests SET created_at=$2-120,expires_at=$2,request_json=$3,request_digest=$4 WHERE request_id=$1")
        .bind(corrupted.request_id).bind(corrupted.expires_at).bind(json!(corrupted)).bind("0".repeat(64)).execute(&panel.state.pool).await?;
    assert!(
        runtime_control::replay_pending(&restarted, server)
            .await
            .is_err(),
        "the persisted original digest must be checked before replay"
    );
    assert!(receiver.try_recv().is_err());
    Ok(())
}
