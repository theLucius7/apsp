use super::*;
use crate::config::Config;
use apsp_protocol::{
    RuntimeCheckpoint, RuntimeCheckpointRequest, RuntimeCheckpointResult, RuntimeOperation,
    RuntimeServiceState, RuntimeSnapshot,
    release::{ReleaseProof, TrustedKeys},
};
use apsp_traffic::runtime_workflows::automation::FrozenAgentBundle;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{path::PathBuf, sync::Arc};

const MODULE: &str = "singbox";
const AGENT_040_SHA256: &str = "f11a6c3d0ee3048ff574d5874e274a7e7f5f09e15cf314d06452a8cc0fa94fe1";
const AGENT_041_SHA256: &str = "8947c400d8864de47b413b0bee014406c05e8d35d72fc5ea3bced27cb7c60690";
const AGENT_BINARY_BYTES: u64 = 710;

struct TestDirectory(PathBuf);

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn singbox_040_sha256() -> String {
    "1".repeat(64)
}

fn singbox_041_sha256() -> String {
    "3".repeat(64)
}

fn frozen_bundle_040() -> FrozenAgentBundle {
    FrozenAgentBundle {
        agent_version: "0.4.0".into(),
        release_tag: "agent-v0.4.0".into(),
        target: "linux-musl-amd64".into(),
        source_repo: apsp_protocol::release::RELEASE_SOURCE_REPO.into(),
        agent_sha256: AGENT_040_SHA256.into(),
        agent_bytes: AGENT_BINARY_BYTES,
        component: "sing-box".into(),
        component_version: apsp_traffic::RUNTIME_VERSION.into(),
        component_sha256: singbox_040_sha256(),
    }
}

fn frozen_bundle_041() -> FrozenAgentBundle {
    FrozenAgentBundle {
        agent_version: "0.4.1".into(),
        release_tag: "agent-v0.4.1".into(),
        target: "linux-musl-amd64".into(),
        source_repo: apsp_protocol::release::RELEASE_SOURCE_REPO.into(),
        agent_sha256: AGENT_041_SHA256.into(),
        agent_bytes: AGENT_BINARY_BYTES,
        component: "sing-box".into(),
        component_version: apsp_traffic::RUNTIME_VERSION.into(),
        component_sha256: singbox_041_sha256(),
    }
}

fn build_test_agent_binary(version: &str, singbox_sha: &str) -> Vec<u8> {
    let compact = format!(
        r#"{{"agent_version":"{version}","components":{{"ipquality":{{"format":"tar.gz","sha256":"{}","version":"87397e2c3196ec796f5477c83343c2354df601ea-node-r1"}},"sing-box":{{"format":"tar.gz","sha256":"{singbox_sha}","version":"1.14.4-apsp.2"}},"tcpquality":{{"format":"agent_subcommand","subcommand":"tcp-probe","version":"0.3.0-{}-r1"}}}},"schema":1,"target_arch":"x86_64","target_env":"musl","target_os":"linux"}}"#,
        "2".repeat(64),
        "a".repeat(40),
    );
    let json_sha256 = format!("{:x}", Sha256::digest(compact.as_bytes()));
    let mut binary = Vec::new();
    binary.extend_from_slice(b"\x7fELF-TEST_ONLY-agent-");
    binary.extend_from_slice(version.as_bytes());
    binary.push(b'-');
    binary.extend_from_slice(b"\0APSP_EMBEDDED_");
    binary.extend_from_slice(b"COMPONENTS_V1:");
    binary.extend_from_slice(format!("{:08x}:{json_sha256}\n", compact.len()).as_bytes());
    binary.extend_from_slice(compact.as_bytes());
    binary.extend_from_slice(b"\n:END_APSP_EMBEDDED_");
    binary.extend_from_slice(b"COMPONENTS_V1\0");
    binary
}

fn release_proof_040() -> ReleaseProof {
    ReleaseProof {
        metadata_json: "{\"schema\":1,\"source_repo\":\"theLucius7/apsp\",\"tag\":\"agent-v0.4.0\",\"protocol_min\":1,\"protocol_max\":1,\"artifacts\":[{\"name\":\"agent\",\"version\":\"0.4.0\",\"arch\":\"linux-musl-amd64\",\"format\":\"raw\",\"binary_name\":\"apsp-agent\",\"archive_size\":710,\"binary_sha256\":\"f11a6c3d0ee3048ff574d5874e274a7e7f5f09e15cf314d06452a8cc0fa94fe1\",\"binary_size\":710,\"asset_name\":\"agent-0.4.0-linux-musl-amd64\"}]}\n".into(),
        checksums: "f11a6c3d0ee3048ff574d5874e274a7e7f5f09e15cf314d06452a8cc0fa94fe1  agent/0.4.0/linux-musl-amd64\n306c6ca7407560340797866e077e053627ad409277d1b9da58106fce4cf717cb  install.sh\nde818d6ef8a9025a482b373bd099ec2dc7530effc469f4487a11660043695c69  release.json\n".into(),
        signature: "untrusted comment: signature from minisign secret key\nRUS3NbDikg3VqRDkj3VomFhxASyy++P9AB+7rZhk9k7soF2w1+pOXrIwIL60FMI2+JNkm5pFiI1Vz5qrcgcj+vT0dWSlwI7KXgM=\ntrusted comment: Apsp TEST ONLY fixture; never trust for official releases\nwlagpEnFgXVTLxklP5g1CC1LR1nHpteUfGO7SeIBAhHV+/dJrXGA2DGTo1NsZefOZLCFCfMZA2XAnAdkNWjoDA==\n".into(),
    }
}

fn release_proof_041() -> ReleaseProof {
    ReleaseProof {
        metadata_json: "{\"schema\":1,\"source_repo\":\"theLucius7/apsp\",\"tag\":\"agent-v0.4.1\",\"protocol_min\":1,\"protocol_max\":1,\"artifacts\":[{\"name\":\"agent\",\"version\":\"0.4.1\",\"arch\":\"linux-musl-amd64\",\"format\":\"raw\",\"binary_name\":\"apsp-agent\",\"archive_size\":710,\"binary_sha256\":\"8947c400d8864de47b413b0bee014406c05e8d35d72fc5ea3bced27cb7c60690\",\"binary_size\":710,\"asset_name\":\"agent-0.4.1-linux-musl-amd64\"}]}\n".into(),
        checksums: "8947c400d8864de47b413b0bee014406c05e8d35d72fc5ea3bced27cb7c60690  agent/0.4.1/linux-musl-amd64\n306c6ca7407560340797866e077e053627ad409277d1b9da58106fce4cf717cb  install.sh\n0502a88c2c4d3943bdb9ed7bd97a40f59a16c81e03e85a44d3fba5af79d59eab  release.json\n".into(),
        signature: "untrusted comment: signature from minisign secret key\nRUS3NbDikg3VqcxIaenRtEiscxCs9ck654jPxLh7zb6XMiHj5WKPs4hpt6CCGImGhiUkyApMHrsoV9UzmO6yYhlMRo2d4ca0HQA=\ntrusted comment: Apsp TEST ONLY fixture; never trust for official releases\n1/1jPj5Hv+T/4KFKJtcVVXlIyoKyszmlOwPRpGCjnM0U629gRUi/bq5fCzFa/nJCjkz6TxS2wOz7mUmIFZu3Bg==\n".into(),
    }
}

async fn signed_state(pool: &PgPool) -> anyhow::Result<(AppState, TestDirectory)> {
    let directory = TestDirectory(std::env::temp_dir().join(format!(
        "apsp-deployment-automation-test-{}",
        Uuid::new_v4()
    )));
    std::fs::create_dir(&directory.0)?;
    let mut state = apsp_governance::state(
        pool.clone(),
        Config {
            database_url: String::new(),
            listen: "127.0.0.1:0".parse()?,
            public_url: "http://127.0.0.1".into(),
            data_dir: directory.0.clone(),
            admin_password: None,
        },
    )
    .await?;
    state.release_keys = Some(Arc::new(TrustedKeys::from_json(include_str!(
        "../../../../protocol/tests/fixtures/public-keys.json"
    ))?));
    let targets = vec!["linux-musl-amd64".to_owned()];
    for (tag, version, singbox_sha, proof) in [
        (
            "agent-v0.4.0",
            "0.4.0",
            singbox_040_sha256(),
            release_proof_040(),
        ),
        (
            "agent-v0.4.1",
            "0.4.1",
            singbox_041_sha256(),
            release_proof_041(),
        ),
    ] {
        let binary = build_test_agent_binary(version, &singbox_sha);
        let expected_asset = format!("agent-{version}-linux-musl-amd64");
        crate::releases::import_bundle_for_targets(
            &state,
            tag,
            proof,
            &targets,
            move |asset, _| {
                let binary = binary.clone();
                let expected_asset = expected_asset.clone();
                async move {
                    match asset.as_str() {
                        "install.sh" => Ok(b"#!/bin/sh\nexit 0\n".to_vec()),
                        name if name == expected_asset => Ok(binary),
                        other => anyhow::bail!("unexpected test asset {other}"),
                    }
                }
            },
        )
        .await?;
    }
    Ok((state, directory))
}

fn static_info_fixture(bundle: &FrozenAgentBundle) -> Value {
    json!({
        "os": "linux",
        "arch": "x86_64",
        "libc": "musl",
        "agent_version": bundle.agent_version,
        "runtime_version": apsp_traffic::RUNTIME_VERSION,
        "components": {
            "ipquality": "87397e2c3196ec796f5477c83343c2354df601ea-node-r1",
            "sing-box": bundle.component_version,
            "tcpquality": format!("0.3.0-{}-r1", "a".repeat(40)),
        },
        "component_digests": {
            "agent": bundle.agent_sha256,
            "ipquality": "2".repeat(64),
            "sing-box": bundle.component_sha256,
        },
    })
}

fn legacy_candidate_fixture(binding: &RuntimeBinding, now: i64) -> Value {
    json!({
        "kind": "singbox_retry_deployment",
        "module": MODULE,
        "runtime_version": apsp_traffic::RUNTIME_VERSION,
        "binding": binding,
        "expected_failure_revision": binding.revision,
        "target_status_updated_at": now,
        "changes_business_configuration": false,
        "retry_unknown_result": false,
        "artifact": {
            "name": "sing-box",
            "version": apsp_traffic::RUNTIME_VERSION,
            "arch": "linux-musl-amd64",
            "sha256": "9".repeat(64),
        },
    })
}

async fn record_verified_checkpoint(
    pool: &PgPool,
    server: i64,
    read: &RuntimeCheckpointRequest,
    binding: RuntimeBinding,
    now: i64,
) -> anyhow::Result<()> {
    let observed = RuntimeCheckpoint {
        binding: binding.clone(),
        activation_id: Uuid::new_v4(),
        instance_id: "TEST_ONLY-verified-instance".into(),
        healthy: true,
    };
    let result = RuntimeCheckpointResult {
        request_id: read.request_id,
        request_digest: read.digest()?,
        observed: Some(observed.clone()),
        success: true,
        error: None,
    };
    sqlx::query("INSERT INTO runtime_control_receipts(request_id,result_json,result_sha256,received_at,outcome) VALUES($1,$2,$3,$4,'verified')")
        .bind(read.request_id)
        .bind(serde_json::to_value(result)?)
        .bind("d".repeat(64))
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE runtime_control_requests SET state='received' WHERE request_id=$1")
        .bind(read.request_id)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO runtime_module_checkpoints(server_id,module,checkpoint_json,checkpoint_request_id,verified_at) VALUES($1,'singbox',$2,$3,$4) ON CONFLICT(server_id,module) DO UPDATE SET checkpoint_json=EXCLUDED.checkpoint_json,checkpoint_request_id=EXCLUDED.checkpoint_request_id,verified_at=EXCLUDED.verified_at")
        .bind(server)
        .bind(serde_json::to_value(observed)?)
        .bind(read.request_id)
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE server_module_status SET applied_rev=$2,healthy=true,last_error=NULL WHERE server_id=$1 AND module='singbox'")
        .bind(server)
        .bind(binding.revision as i64)
        .execute(pool)
        .await?;
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn interrupted_mutation_is_reconciled_only_with_its_new_verified_read_and_keeps_original_result(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (state, _directory) = signed_state(&pool).await?;
    let (server, original, binding) = fixture(
        &pool,
        true,
        false,
        Some(RuntimeOperationError::Interrupted),
        false,
    )
    .await?;
    let before_ops: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_operations")
        .fetch_one(&pool)
        .await?;
    let before_result: Option<Value> =
        sqlx::query_scalar("SELECT result FROM runtime_operations WHERE id=$1")
            .bind(original)
            .fetch_one(&pool)
            .await?;
    let before_candidate: Value = sqlx::query_scalar(
        "SELECT details->'candidate' FROM operations_history WHERE action='runtime_deployment_bound' AND details->>'request_id'=$1::text",
    )
    .bind(original)
    .fetch_one(&pool)
    .await?;

    let mut untrusted_state = state.clone();
    untrusted_state.release_keys = None;
    let mut tx = pool.begin().await?;
    assert!(matches!(
        request_automation_deployment_checkpoint_tx(&untrusted_state, &mut tx, original).await,
        Err(ApiError::Conflict(_))
    ));
    tx.rollback().await?;

    let mut tx = pool.begin().await?;
    let read = request_automation_deployment_checkpoint_tx(&state, &mut tx, original).await?;
    assert_eq!(read.expected, binding);
    assert!(matches!(
        reconcile_automation_deployment_tx(
            &state,
            &mut tx,
            original,
            read.request_id,
            "TEST_ONLY checked exact instance"
        )
        .await,
        Err(ApiError::Conflict(_))
    ));
    tx.commit().await?;

    record_verified_checkpoint(&pool, server, &read, binding.clone(), now_timestamp()).await?;

    let mut tx = pool.begin().await?;
    assert!(matches!(
        reconcile_automation_deployment_tx(
            &state,
            &mut tx,
            original,
            Uuid::new_v4(),
            "TEST_ONLY unrelated read must fail"
        )
        .await,
        Err(ApiError::Conflict(_))
    ));
    let reconciliation = reconcile_automation_deployment_tx(
        &state,
        &mut tx,
        original,
        read.request_id,
        "TEST_ONLY exact checkpoint checked; stop all later steps",
    )
    .await?;
    assert_eq!(reconciliation["original_result"]["error"], "interrupted");
    assert_eq!(reconciliation["original_mutation_replayed"], false);
    assert_eq!(reconciliation["legacy_candidate"], false);
    assert_eq!(
        reconciliation["agent_bundle"]["agent_sha256"],
        AGENT_040_SHA256
    );
    let receipt = automation_deployment_receipt(&mut tx, original).await?;
    assert_eq!(receipt["state"], "failed");
    assert_eq!(receipt["original_state"], "uncertain");
    assert_eq!(receipt["result"]["error"], "interrupted");
    assert!(receipt["reconciled_at"].as_i64().is_some());
    tx.commit().await?;

    let after_ops: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_operations")
        .fetch_one(&pool)
        .await?;
    let after_result: Option<Value> =
        sqlx::query_scalar("SELECT result FROM runtime_operations WHERE id=$1")
            .bind(original)
            .fetch_one(&pool)
            .await?;
    let after_candidate: Value = sqlx::query_scalar(
        "SELECT details->'candidate' FROM operations_history WHERE action='runtime_deployment_bound' AND details->>'request_id'=$1::text",
    )
    .bind(original)
    .fetch_one(&pool)
    .await?;
    assert_eq!(after_ops, before_ops);
    assert_eq!(after_result, before_result);
    assert_eq!(after_candidate, before_candidate);
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn legacy_dispatched_unknowns_reconcile_readonly_without_replay_or_candidate_mutation(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (state, _directory) = signed_state(&pool).await?;

    // Trigger 1a: legacy dispatched request expired without device result.
    let (_, expired_legacy, expired_binding) = fixture(&pool, true, true, None, false).await?;
    let legacy_expired_json = legacy_candidate_fixture(&expired_binding, now_timestamp());
    sqlx::query(
        "UPDATE operations_history SET details=jsonb_build_object('request_id',$1::text,'candidate',$2) WHERE action='runtime_deployment_bound' AND details->>'request_id'=$1::text",
    )
    .bind(expired_legacy)
    .bind(&legacy_expired_json)
    .execute(&pool)
    .await?;
    let mut tx = pool.begin().await?;
    assert_eq!(
        automation_deployment_receipt(&mut tx, expired_legacy).await?["state"],
        "uncertain"
    );
    let expired_read =
        request_automation_deployment_checkpoint_tx(&state, &mut tx, expired_legacy).await?;
    assert_eq!(expired_read.expected, expired_binding);
    tx.commit().await?;

    // Trigger 1b & Trigger 2: legacy dispatched Interrupted AND legacy dispatched healthy apply
    // awaiting final verified checkpoint across panel upgrade.
    for (error, success) in [
        (Some(RuntimeOperationError::Interrupted), false),
        (None, true),
    ] {
        let (server, original, binding) = fixture(&pool, true, false, error, success).await?;
        let legacy_json = legacy_candidate_fixture(&binding, now_timestamp());
        sqlx::query(
            "UPDATE operations_history SET details=jsonb_build_object('request_id',$1::text,'candidate',$2) WHERE action='runtime_deployment_bound' AND details->>'request_id'=$1::text",
        )
        .bind(original)
        .bind(&legacy_json)
        .execute(&pool)
        .await?;

        let before_ops: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_operations")
            .fetch_one(&pool)
            .await?;
        let before_result_raw: Option<String> =
            sqlx::query_scalar("SELECT result::text FROM runtime_operations WHERE id=$1")
                .bind(original)
                .fetch_one(&pool)
                .await?;
        let before_candidate_raw: String = sqlx::query_scalar(
            "SELECT (details->'candidate')::text FROM operations_history WHERE action='runtime_deployment_bound' AND details->>'request_id'=$1::text",
        )
        .bind(original)
        .fetch_one(&pool)
        .await?;

        let mut tx = pool.begin().await?;
        assert_eq!(
            automation_deployment_receipt(&mut tx, original).await?["state"],
            "uncertain"
        );
        let read_first =
            request_automation_deployment_checkpoint_tx(&state, &mut tx, original).await?;
        tx.commit().await?;

        record_verified_checkpoint(&pool, server, &read_first, binding.clone(), now_timestamp())
            .await?;

        // Reject if signed Agent/component identity changed between checkpoint creation and reconcile.
        sqlx::query(
            "UPDATE servers SET static_info=$2,static_info_received_at=$3,last_seen=$3 WHERE id=$1",
        )
        .bind(server)
        .bind(static_info_fixture(&frozen_bundle_041()))
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
        let mut tx = pool.begin().await?;
        assert!(matches!(
            reconcile_automation_deployment_tx(
                &state,
                &mut tx,
                original,
                read_first.request_id,
                "TEST_ONLY agent changed during inspection must fail"
            )
            .await,
            Err(ApiError::Conflict(_))
        ));
        tx.rollback().await?;

        // Reject if target binding drifted to another revision.
        let mut tx = pool.begin().await?;
        sqlx::query(
            "UPDATE server_module_status SET target_rev=2 WHERE server_id=$1 AND module='singbox'",
        )
        .bind(server)
        .execute(&mut *tx)
        .await?;
        assert!(matches!(
            request_automation_deployment_checkpoint_tx(&state, &mut tx, original).await,
            Err(ApiError::Conflict(_))
        ));
        tx.rollback().await?;

        // Request a fresh readonly checkpoint under the current signed 0.4.1 Agent and verify it.
        let mut tx = pool.begin().await?;
        let read_second =
            request_automation_deployment_checkpoint_tx(&state, &mut tx, original).await?;
        tx.commit().await?;

        // Stale checkpoint (>300s old) must fail.
        record_verified_checkpoint(
            &pool,
            server,
            &read_second,
            binding.clone(),
            now_timestamp() - 301,
        )
        .await?;
        let mut tx = pool.begin().await?;
        assert!(matches!(
            reconcile_automation_deployment_tx(
                &state,
                &mut tx,
                original,
                read_second.request_id,
                "TEST_ONLY stale checkpoint must fail"
            )
            .await,
            Err(ApiError::Conflict(_))
        ));
        tx.rollback().await?;

        // Fresh verified checkpoint matching frozen 0.4.1 bundle reconciles without mutating history.
        record_verified_checkpoint(
            &pool,
            server,
            &read_second,
            binding.clone(),
            now_timestamp(),
        )
        .await?;
        let mut tx = pool.begin().await?;
        let reconciliation = reconcile_automation_deployment_tx(
            &state,
            &mut tx,
            original,
            read_second.request_id,
            "TEST_ONLY legacy dispatched unknown verified readonly",
        )
        .await?;
        assert_eq!(reconciliation["legacy_candidate"], true);
        assert_eq!(reconciliation["original_mutation_replayed"], false);
        assert_eq!(reconciliation["original_result_rewritten"], false);
        assert_eq!(
            reconciliation["agent_bundle"]["agent_sha256"],
            AGENT_041_SHA256
        );
        let receipt = automation_deployment_receipt(&mut tx, original).await?;
        assert_eq!(receipt["state"], "failed");
        assert_eq!(receipt["original_state"], "uncertain");
        tx.commit().await?;

        let after_ops: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_operations")
            .fetch_one(&pool)
            .await?;
        let after_result_raw: Option<String> =
            sqlx::query_scalar("SELECT result::text FROM runtime_operations WHERE id=$1")
                .bind(original)
                .fetch_one(&pool)
                .await?;
        let after_candidate_raw: String = sqlx::query_scalar(
            "SELECT (details->'candidate')::text FROM operations_history WHERE action='runtime_deployment_bound' AND details->>'request_id'=$1::text",
        )
        .bind(original)
        .fetch_one(&pool)
        .await?;
        assert_eq!(after_ops, before_ops);
        assert_eq!(after_result_raw, before_result_raw);
        assert_eq!(after_candidate_raw, before_candidate_raw);
    }
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn readonly_reconciliation_stops_later_steps_releases_only_eligible_lock_and_keeps_completed_legacy_history_readonly(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (state, _directory) = signed_state(&pool).await?;
    let (server_one, op_one, binding_one) = fixture(&pool, true, false, None, true).await?;
    let (server_two, op_two, binding_two) = fixture(
        &pool,
        true,
        false,
        Some(RuntimeOperationError::Interrupted),
        false,
    )
    .await?;
    let now = now_timestamp();
    let job: Uuid =
        sqlx::query_scalar("SELECT automation_job_id FROM runtime_operations WHERE id=$1")
            .bind(op_one)
            .fetch_one(&pool)
            .await?;
    let legacy_one = legacy_candidate_fixture(&binding_one, now);
    let legacy_two = legacy_candidate_fixture(&binding_two, now);
    sqlx::query("UPDATE runtime_operations SET automation_job_id=$2 WHERE id=$1")
        .bind(op_two)
        .bind(job)
        .execute(&pool)
        .await?;
    sqlx::query(
        "UPDATE operations_history SET details=jsonb_build_object('request_id',$1::text,'candidate',$2) WHERE action='runtime_deployment_bound' AND details->>'request_id'=$1::text",
    )
    .bind(op_one)
    .bind(&legacy_one)
    .execute(&pool)
    .await?;
    sqlx::query(
        "UPDATE operations_history SET job_id=$1,details=jsonb_build_object('request_id',$2::text,'candidate',$3) WHERE action='runtime_deployment_bound' AND details->>'request_id'=$2::text",
    )
    .bind(job)
    .bind(op_two)
    .bind(&legacy_two)
    .execute(&pool)
    .await?;
    let legacy_spec = json!({
        "plan": {
            "name": "TEST_ONLY legacy multi-server job",
            "steps": [
                {
                    "kind": "singbox_retry_deployment",
                    "service": null,
                    "runtime_version": apsp_traffic::RUNTIME_VERSION,
                    "timeout_secs": 300
                },
                {
                    "kind": "service_status",
                    "service": "nginx",
                    "timeout_secs": 120
                }
            ],
            "batch_size": 1,
            "concurrency": 1,
            "pause_between_batches": false,
            "max_duration_secs": 1800
        },
        "typed_steps": [{
            "position": 0,
            "kind": "singbox_retry_deployment",
            "version": apsp_traffic::RUNTIME_VERSION,
            "targets": {
                server_one.to_string(): legacy_one,
                server_two.to_string(): legacy_two,
            }
        }]
    });
    sqlx::query("UPDATE operations_jobs SET spec=$2,targets=$3,status='uncertain' WHERE id=$1")
        .bind(job)
        .bind(&legacy_spec)
        .bind(vec![server_one, server_two])
        .execute(&pool)
        .await?;
    for (server, op, batch) in [(server_one, op_one, 0_i32), (server_two, op_two, 1_i32)] {
        sqlx::query(
            "INSERT INTO operations_server_locks(server_id,job_id,acquired_at) VALUES($1,$2,$3)",
        )
        .bind(server)
        .bind(job)
        .bind(now)
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO operations_target_steps(job_id,server_id,position,batch,state,runtime_operation_id,started_at) VALUES($1,$2,0,$3,'uncertain',$4,$5)")
            .bind(job)
            .bind(server)
            .bind(batch)
            .bind(op)
            .bind(now - 5)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO operations_target_steps(job_id,server_id,position,batch,state) VALUES($1,$2,1,$3,'pending')")
            .bind(job)
            .bind(server)
            .bind(batch)
            .execute(&pool)
            .await?;
    }

    let mut tx = pool.begin().await?;
    let requested = super::super::runtime_reconciliation::checkpoint_target_tx(
        &state, &mut tx, job, server_one, 1,
    )
    .await?;
    tx.commit().await?;
    let checkpoint_id: Uuid = requested["checkpoint_request_id"]
        .as_str()
        .expect("checkpoint id")
        .parse()?;
    let checkpoint: RuntimeCheckpointRequest = serde_json::from_value(
        sqlx::query_scalar::<_, Value>(
            "SELECT request_json FROM runtime_control_requests WHERE request_id=$1",
        )
        .bind(checkpoint_id)
        .fetch_one(&pool)
        .await?,
    )?;
    record_verified_checkpoint(&pool, server_one, &checkpoint, binding_one, now_timestamp())
        .await?;

    let mut tx = pool.begin().await?;
    let reconciled = super::super::runtime_reconciliation::reconcile_target_tx(
        &state,
        &mut tx,
        job,
        server_one,
        1,
        checkpoint_id,
        "TEST_ONLY reconcile server one and keep server two lock",
    )
    .await?;
    assert_eq!(reconciled["state"], "reconciled");
    assert_eq!(reconciled["next_steps"], "stopped");
    assert_eq!(reconciled["automatic_replay"], false);
    tx.commit().await?;

    let skipped_steps: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM operations_target_steps WHERE job_id=$1 AND position=1 AND state='skipped'",
    )
    .bind(job)
    .fetch_one(&pool)
    .await?;
    assert_eq!(skipped_steps, 2);
    let remaining_locks: Vec<i64> = sqlx::query_scalar(
        "SELECT server_id FROM operations_server_locks WHERE job_id=$1 ORDER BY server_id",
    )
    .bind(job)
    .fetch_all(&pool)
    .await?;
    assert_eq!(remaining_locks, vec![server_two]);

    // Completed historical legacy jobs remain read-only and cannot be reopened.
    let (_, completed_op, _) = fixture(&pool, true, false, None, true).await?;
    let completed_job: Uuid =
        sqlx::query_scalar("SELECT automation_job_id FROM runtime_operations WHERE id=$1")
            .bind(completed_op)
            .fetch_one(&pool)
            .await?;
    sqlx::query("UPDATE operations_jobs SET spec=$2,status='succeeded' WHERE id=$1")
        .bind(completed_job)
        .bind(&legacy_spec)
        .execute(&pool)
        .await?;
    let parsed_plan: super::super::model::Plan =
        serde_json::from_value(legacy_spec["plan"].clone())?;
    assert!(parsed_plan.has_legacy_runtime_version());
    let mut tx = pool.begin().await?;
    assert!(matches!(
        super::super::runtime_reconciliation::checkpoint_target_tx(
            &state,
            &mut tx,
            completed_job,
            server_one,
            1,
        )
        .await,
        Err(ApiError::Conflict(_))
    ));
    assert!(matches!(
        request_automation_deployment_checkpoint_tx(&state, &mut tx, completed_op).await,
        Err(ApiError::Conflict(_))
    ));
    tx.rollback().await?;
    Ok(())
}

async fn fixture(
    pool: &PgPool,
    dispatched: bool,
    expired: bool,
    error: Option<RuntimeOperationError>,
    success: bool,
) -> anyhow::Result<(i64, Uuid, RuntimeBinding)> {
    let now = now_timestamp();
    sqlx::query("INSERT INTO admins(id,password_hash) VALUES(1,'TEST_ONLY-not-a-login-password') ON CONFLICT DO NOTHING").execute(pool).await?;
    let bundle = frozen_bundle_040();
    let capabilities = json!([
        "singbox",
        apsp_protocol::SELF_CONTAINED_AGENT_CAPABILITY,
        apsp_protocol::RUNTIME_OPERATIONS_CAPABILITY,
        apsp_protocol::RUNTIME_CHECKPOINT_CAPABILITY,
    ]);
    let server: i64 = sqlx::query_scalar("INSERT INTO servers(name,static_info,static_info_received_at,capabilities,last_seen) VALUES('TEST_ONLY automation deployment',$1,$2,$3,$2) RETURNING id")
        .bind(static_info_fixture(&bundle))
        .bind(now)
        .bind(capabilities)
        .fetch_one(pool)
        .await?;
    sqlx::query("INSERT INTO nodes(server_id,name,port,public_host,sni,private_key,public_key,short_id,enabled) VALUES($1,'TEST_ONLY configured node',443,'example.com','example.com','TEST_ONLY_PRIV','TEST_ONLY_PUB','01020304',true)")
        .bind(server)
        .execute(pool)
        .await?;
    let hash = crate::auth::hash_token("{}");
    sqlx::query("INSERT INTO deployments(server_id,module,rev,bundle,bundle_sha256,source_json,created_at) VALUES($1,'singbox',1,'{}',$2,'[]',$3)").bind(server).bind(&hash).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO server_module_status(server_id,module,target_rev,applied_rev,last_result_rev,healthy,last_error,updated_at) VALUES($1,'singbox',1,0,1,false,'TEST_ONLY failed target',$2)").bind(server).bind(now).execute(pool).await?;
    let binding = RuntimeBinding::new(Uuid::new_v4(), MODULE.into(), 1, hash);
    sqlx::query("INSERT INTO runtime_deployment_bindings(deployment_id,server_id,module,rev,bundle_sha256,binding_digest) VALUES($1,$2,'singbox',1,$3,$4)").bind(binding.deployment_id).bind(server).bind(&binding.bundle_sha256).bind(&binding.binding_digest).execute(pool).await?;
    let job = Uuid::new_v4();
    sqlx::query("INSERT INTO operations_jobs(id,name,requested_by,spec,targets,status,created_at,updated_at,expires_at,preview_digest) VALUES($1,'TEST_ONLY runtime reapply',1,'{}',$2,'running',$3,$3,$4,'TEST_ONLY')").bind(job).bind(vec![server]).bind(now - 10).bind(now + 3600).execute(pool).await?;
    let request = RuntimeOperationRequest {
        id: Uuid::new_v4(),
        module: MODULE.into(),
        operation: RuntimeOperation::RetryDeployment,
        expected_revision: Some(1),
        session_filter: None,
        requested_at: now - 10,
        expires_at: if expired { now - 1 } else { now + 300 },
    };
    let result = (error.is_some() || success).then(|| RuntimeOperationResult {
        id: request.id,
        module: MODULE.into(),
        operation: request.operation,
        sessions: None,
        finished_at: now,
        error,
        snapshot: success.then_some(RuntimeSnapshot {
            observed_at: now,
            applied_revision: Some(1),
            service: RuntimeServiceState::Active,
            healthy: Some(true),
            logs_available: false,
            logs_service_events: false,
            logs_truncated: false,
            logs: vec![],
        }),
    });
    sqlx::query("INSERT INTO runtime_operations(id,server_id,module,requested_at,spec,result,dispatched_at,automation_job_id) VALUES($1,$2,'singbox',$3,$4,$5,$6,$7)").bind(request.id).bind(server).bind(request.requested_at).bind(serde_json::to_value(&request)?).bind(result.map(serde_json::to_value).transpose()?).bind(dispatched.then_some(now - 5)).bind(job).execute(pool).await?;
    let candidate = json!({
        "kind": "singbox_retry_deployment",
        "module": MODULE,
        "binding": binding,
        "expected_failure_revision": 1,
        "target_status_updated_at": now,
        "observed_agent_version": "0.4.0",
        "observed_runtime_version": apsp_traffic::RUNTIME_VERSION,
        "changes_business_configuration": false,
        "retry_unknown_result": false,
        "agent_bundle": bundle,
    });
    sqlx::query("INSERT INTO operations_history(job_id,action,details,recorded_at) VALUES($1,'runtime_deployment_bound',$2,$3)").bind(job).bind(json!({"request_id":request.id,"candidate":candidate})).bind(now).execute(pool).await?;
    Ok((server, request.id, binding))
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn cancellation_never_fabricates_a_device_result_or_cancels_dispatched_execution(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (_, queued, _) = fixture(&pool, false, false, None, false).await?;
    let mut tx = pool.begin().await?;
    cancel_automation_deployment_tx(&mut tx, queued).await?;
    let receipt = automation_deployment_receipt(&mut tx, queued).await?;
    assert_eq!(receipt["state"], "cancelled");
    assert!(receipt["result"].is_null());
    tx.commit().await?;
    let (_, dispatched, _) = fixture(&pool, true, false, None, false).await?;
    let mut tx = pool.begin().await?;
    cancel_automation_deployment_tx(&mut tx, dispatched).await?;
    let receipt = automation_deployment_receipt(&mut tx, dispatched).await?;
    assert_eq!(receipt["state"], "running");
    assert_eq!(receipt["local_cancelled_before_dispatch"], false);
    tx.commit().await?;
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn expiry_and_interruption_keep_unknown_result_distinct_from_definitive_failure(
    pool: PgPool,
) -> anyhow::Result<()> {
    for (dispatched, expired, error, expected) in [
        (false, true, None, "cancelled"),
        (true, true, None, "uncertain"),
        (
            true,
            false,
            Some(RuntimeOperationError::Interrupted),
            "uncertain",
        ),
        (
            true,
            false,
            Some(RuntimeOperationError::OperationFailed),
            "failed",
        ),
    ] {
        let (_, request, _) = fixture(&pool, dispatched, expired, error, false).await?;
        let mut tx = pool.begin().await?;
        let receipt = automation_deployment_receipt(&mut tx, request).await?;
        assert_eq!(receipt["state"], expected);
        assert_eq!(receipt["checkpoint_confirmed"], false);
        tx.commit().await?;
    }
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn success_requires_a_fresh_exact_checkpoint_and_current_verified_agent_bundle(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (server, request, binding) = fixture(&pool, true, false, None, true).await?;
    let mut tx = pool.begin().await?;
    let receipt = automation_deployment_receipt(&mut tx, request).await?;
    assert_eq!(receipt["state"], "running");
    assert_eq!(receipt["checkpoint_confirmed"], false);
    let checkpoint_id: Uuid = receipt["checkpoint_request_id"].as_str().unwrap().parse()?;
    let again = automation_deployment_receipt(&mut tx, request).await?;
    assert_eq!(
        again["checkpoint_request_id"],
        receipt["checkpoint_request_id"]
    );
    tx.commit().await?;
    let checkpoint: RuntimeCheckpointRequest = serde_json::from_value(
        sqlx::query_scalar::<_, Value>(
            "SELECT request_json FROM runtime_control_requests WHERE request_id=$1",
        )
        .bind(checkpoint_id)
        .fetch_one(&pool)
        .await?,
    )?;
    record_verified_checkpoint(&pool, server, &checkpoint, binding, now_timestamp()).await?;

    let mut tx = pool.begin().await?;
    assert_eq!(
        automation_deployment_receipt(&mut tx, request).await?["state"],
        "succeeded"
    );
    let mut drifted_agent = frozen_bundle_040();
    drifted_agent.agent_sha256 = "c".repeat(64);
    sqlx::query("UPDATE servers SET static_info=$2 WHERE id=$1")
        .bind(server)
        .bind(static_info_fixture(&drifted_agent))
        .execute(&mut *tx)
        .await?;
    assert_eq!(
        automation_deployment_receipt(&mut tx, request).await?["state"],
        "uncertain"
    );
    tx.rollback().await?;

    let mut tx = pool.begin().await?;
    let mut drifted_component = frozen_bundle_040();
    drifted_component.component_sha256 = "9".repeat(64);
    sqlx::query("UPDATE servers SET static_info=$2 WHERE id=$1")
        .bind(server)
        .bind(static_info_fixture(&drifted_component))
        .execute(&mut *tx)
        .await?;
    assert_eq!(
        automation_deployment_receipt(&mut tx, request).await?["state"],
        "uncertain"
    );
    tx.rollback().await?;

    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE servers SET static_info_received_at=$2 WHERE id=$1")
        .bind(server)
        .bind(now_timestamp() - 601)
        .execute(&mut *tx)
        .await?;
    assert_eq!(
        automation_deployment_receipt(&mut tx, request).await?["state"],
        "uncertain"
    );
    tx.rollback().await?;
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn legacy_artifact_candidates_and_previews_are_explicitly_invalidated_and_never_dispatched(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (state, _directory) = signed_state(&pool).await?;
    let (server, request, binding) = fixture(&pool, false, false, None, false).await?;
    let legacy = legacy_candidate_fixture(&binding, now_timestamp());
    sqlx::query(
        "UPDATE operations_history SET details=jsonb_build_object('request_id',$1::text,'candidate',$2) WHERE details->>'request_id'=$1::text",
    )
    .bind(request)
    .bind(&legacy)
    .execute(&pool)
    .await?;
    let mut tx = pool.begin().await?;
    assert!(!automation_dispatch_matches_tx(&mut tx, server, request).await?);
    assert!(matches!(
        request_automation_deployment_checkpoint_tx(&state, &mut tx, request).await,
        Err(ApiError::Conflict(_))
    ));
    let receipt = automation_deployment_receipt(&mut tx, request).await?;
    assert_eq!(receipt["state"], "failed");
    assert_eq!(receipt["executed"], false);
    assert_eq!(receipt["local_cancelled_before_dispatch"], true);

    let legacy_spec = json!({
        "plan": {
            "name": "TEST_ONLY legacy preview",
            "steps": [{
                "kind": "singbox_retry_deployment",
                "service": null,
                "runtime_version": apsp_traffic::RUNTIME_VERSION,
                "timeout_secs": 300
            }],
            "batch_size": 1,
            "concurrency": 1,
            "pause_between_batches": false,
            "max_duration_secs": 1800
        },
        "typed_steps": [{
            "position": 0,
            "kind": "singbox_retry_deployment",
            "version": apsp_traffic::RUNTIME_VERSION,
            "targets": { server.to_string(): legacy }
        }]
    });
    let legacy_plan: super::super::model::Plan =
        serde_json::from_value(legacy_spec["plan"].clone())?;
    assert!(matches!(
        super::super::typed_steps::validate_frozen(
            &state,
            &mut tx,
            &[server],
            &legacy_plan,
            &legacy_spec,
        )
        .await,
        Err(ApiError::Conflict(_))
    ));
    tx.commit().await?;
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn automation_candidate_uses_signed_embedded_manifest_and_keeps_unapplied_observed_runtime_version_null(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (state, _directory) = signed_state(&pool).await?;
    let (server, _, _) = fixture(&pool, false, false, None, false).await?;
    let mut unapplied_info = static_info_fixture(&frozen_bundle_040());
    unapplied_info
        .as_object_mut()
        .expect("static_info object")
        .remove("runtime_version");
    sqlx::query("UPDATE servers SET static_info=$2 WHERE id=$1")
        .bind(server)
        .bind(&unapplied_info)
        .execute(&pool)
        .await?;

    let mut tx = pool.begin().await?;
    let candidate = automation::automation_candidate(&state, &mut tx, server).await?;
    assert_eq!(candidate["observed_agent_version"], "0.4.0");
    assert!(candidate["observed_runtime_version"].is_null());
    assert!(candidate.get("artifact").is_none());
    assert!(candidate.get("runtime_version").is_none());
    assert_eq!(candidate["agent_bundle"]["agent_sha256"], AGENT_040_SHA256);
    assert_eq!(
        candidate["agent_bundle"]["component_sha256"],
        singbox_040_sha256()
    );
    tx.rollback().await?;

    // Cross-component digest substitution (ipquality digest reported as sing-box) must be rejected.
    let mut cross_component = unapplied_info;
    cross_component["component_digests"]["sing-box"] = json!("2".repeat(64));
    sqlx::query("UPDATE servers SET static_info=$2 WHERE id=$1")
        .bind(server)
        .bind(&cross_component)
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    assert!(matches!(
        automation::automation_candidate(&state, &mut tx, server).await,
        Err(ApiError::Conflict(_))
    ));
    tx.rollback().await?;
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn retries_wait_for_unresolved_work_and_a_verified_read_after_an_interruption(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (state, _directory) = signed_state(&pool).await?;
    let (server, original, binding) = fixture(
        &pool,
        true,
        false,
        Some(RuntimeOperationError::Interrupted),
        false,
    )
    .await?;
    let mut tx = pool.begin().await?;
    assert!(
        !crate::runtime_operations::automation_blocked(&mut tx, server, now_timestamp()).await?
    );
    assert!(interrupted(&mut tx, server).await?);
    let read = request_automation_deployment_checkpoint_tx(&state, &mut tx, original).await?;
    tx.commit().await?;
    let result = RuntimeCheckpointResult {
        request_id: read.request_id,
        request_digest: read.digest()?,
        observed: Some(RuntimeCheckpoint {
            binding,
            activation_id: Uuid::new_v4(),
            instance_id: "TEST_ONLY-read-after-interruption".into(),
            healthy: true,
        }),
        success: true,
        error: None,
    };
    sqlx::query("INSERT INTO runtime_control_receipts(request_id,result_json,result_sha256,received_at,outcome) VALUES($1,$2,$3,$4,'verified')").bind(read.request_id).bind(serde_json::to_value(result)?).bind("e".repeat(64)).bind(now_timestamp()).execute(&pool).await?;
    let mut tx = pool.begin().await?;
    assert!(!interrupted(&mut tx, server).await?);
    let queued = RuntimeOperationRequest {
        id: Uuid::new_v4(),
        module: MODULE.into(),
        operation: RuntimeOperation::Inspect,
        expected_revision: None,
        session_filter: None,
        requested_at: now_timestamp(),
        expires_at: now_timestamp() + 600,
    };
    crate::runtime_operations::enqueue(&mut tx, server, MODULE, &queued).await?;
    assert!(crate::runtime_operations::automation_blocked(&mut tx, server, now_timestamp()).await?);
    tx.rollback().await?;
    Ok(())
}
