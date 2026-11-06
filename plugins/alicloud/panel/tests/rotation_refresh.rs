use super::billing_cases::{bill, row, traffic};
use super::*;

/// A minimal unfinished rotation: the refresh pass only asks whether one runs.
async fn rotation(pool: &PgPool, resource: Uuid, status: &str) -> Uuid {
    sqlx::query("INSERT INTO admins(id,password_hash) VALUES(1,'TEST_ONLY-not-a-login-password') ON CONFLICT DO NOTHING")
        .execute(pool)
        .await
        .unwrap();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO alicloud_eip_rotations(id,resource_id,requested_by,before_state,probe_port,max_attempts,snapshot_digest,status,bound_id,created_at,expires_at,updated_at) VALUES($1,$2,1,'{}',443,3,'TEST_ONLY','running','eip-testonly',0,0,0)")
        .bind(id).bind(resource).execute(pool).await.unwrap();
    sqlx::query("UPDATE alicloud_eip_rotations SET status=$2 WHERE id=$1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    id
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn refresh_leaves_a_rotating_instance_to_its_rotation(pool: PgPool) {
    // Mid-rotation the instance may be bound to a candidate that is about to
    // be released; the rotation stores the final address itself when it ends.
    let (account_id, id) = seed(&pool, "ecs").await;
    let rotation = rotation(&pool, id, "running").await;
    let replies = || {
        vec![
            bill(1, 1, vec![row("1.000000", "x")]),
            traffic(),
            snapshot("ecs", 10),
        ]
    };
    let mock = Mock::start(replies()).await;
    worker::refresh(&pool, account_id, &Cloud::local(&mock.endpoint))
        .await
        .unwrap();
    mock.exhausted();
    let resource = super::super::resource(&pool, id).await.unwrap();
    assert!(resource.snapshot.is_none());
    assert_eq!(resource.checked_at, None);
    // Halted for a person, it no longer moves anything: the refresh shows the cloud.
    sqlx::query("UPDATE alicloud_eip_rotations SET status='unknown' WHERE id=$1")
        .bind(rotation)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE alicloud_accounts SET next_run_at=0 WHERE id=$1")
        .bind(account_id)
        .execute(&pool)
        .await
        .unwrap();
    let mock = Mock::start(replies()).await;
    worker::refresh(&pool, account_id, &Cloud::local(&mock.endpoint))
        .await
        .unwrap();
    mock.exhausted();
    let resource = super::super::resource(&pool, id).await.unwrap();
    assert_eq!(resource.snapshot.unwrap().public_ip, "192.0.2.1");
}
