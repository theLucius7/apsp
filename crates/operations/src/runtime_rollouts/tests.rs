use super::*;
use apsp_protocol::{RuntimeOperationError, RuntimeOperationResult};
use sqlx::PgPool;

/// A rollout of one dispatched target whose inspection finished on the device with `error`,
/// with the result as the device reported it.
async fn rollout(
    pool: &PgPool,
    error: RuntimeOperationError,
) -> anyhow::Result<(i64, Uuid, Value)> {
    let now = now_timestamp();
    sqlx::query("INSERT INTO admins(id,password_hash) VALUES(1,'TEST_ONLY-not-a-login-password') ON CONFLICT DO NOTHING").execute(pool).await?;
    let server: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name,last_seen) VALUES('TEST_ONLY rollout target',$1) RETURNING id",
    )
    .bind(now)
    .fetch_one(pool)
    .await?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_runtime_rollouts(id,administrator_id,runtime_version,targets,batch_size,dispatched_count,created_at,updated_at,artifact_snapshot) VALUES($1,1,$2,$3,1,1,$4,$4,'{}')").bind(id).bind(apsp_traffic::RUNTIME_VERSION).bind(vec![server]).bind(now).execute(pool).await?;
    let inspect = inspection();
    let mut tx = pool.begin().await?;
    crate::runtime_operations::enqueue(&mut tx, server, MODULE, &inspect).await?;
    rollouts::add_member_tx(
        &mut tx,
        id,
        server,
        inspect.id,
        inspect.requested_at,
        &"a".repeat(64),
    )
    .await?;
    tx.commit().await?;
    let result = serde_json::to_value(RuntimeOperationResult {
        id: inspect.id,
        module: MODULE.into(),
        operation: RuntimeOperation::Inspect,
        sessions: None,
        finished_at: now,
        error: Some(error),
        snapshot: None,
    })?;
    sqlx::query("UPDATE runtime_operations SET dispatched_at=$2,result=$3 WHERE id=$1")
        .bind(inspect.id)
        .bind(now)
        .bind(&result)
        .execute(pool)
        .await?;
    Ok((server, id, result))
}

async fn recorded(pool: &PgPool, rollout: Uuid) -> anyhow::Result<Option<Value>> {
    Ok(sqlx::query_scalar(
        "SELECT inspection_result FROM singbox_runtime_rollout_members WHERE rollout_id=$1",
    )
    .bind(rollout)
    .fetch_one(pool)
    .await?)
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn finished_inspections_reach_their_members_once(pool: PgPool) -> anyhow::Result<()> {
    let (server, first, failed) = rollout(&pool, RuntimeOperationError::OperationFailed).await?;
    let (_, second, interrupted) = rollout(&pool, RuntimeOperationError::Interrupted).await?;
    let mut tx = pool.begin().await?;
    let view = members(&mut tx, first).await?;
    tx.commit().await?;
    assert_eq!(view.len(), 1);
    assert_eq!(view[0]["server_id"], server);
    assert_eq!(view[0]["state"], "failed");
    assert_eq!(view[0]["confirmed"], false);
    assert_eq!(recorded(&pool, first).await?, Some(failed.clone()));
    assert_eq!(recorded(&pool, second).await?, None);
    let mut tx = pool.begin().await?;
    record_inspections(&mut tx, None).await?;
    tx.commit().await?;
    assert_eq!(recorded(&pool, second).await?, Some(interrupted.clone()));
    // A recorded result stays as it reached the rollout.
    sqlx::query("UPDATE runtime_operations SET result=$1")
        .bind(json!({"TEST_ONLY":"changed later"}))
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    record_inspections(&mut tx, None).await?;
    tx.commit().await?;
    assert_eq!(recorded(&pool, first).await?, Some(failed));
    assert_eq!(recorded(&pool, second).await?, Some(interrupted));
    Ok(())
}
