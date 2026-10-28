use super::super::costs;
use super::*;

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn minimum_permissions_skip_all_bss_calls_and_keep_cdt_and_ecs_queries(pool: PgPool) {
    let (id, resource_id) = seed(&pool, "ecs").await;
    sqlx::query(
        "UPDATE alicloud_accounts SET billing_enabled=false,balance_enabled=false WHERE id=$1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let mock = Mock::start(vec![billing_cases::traffic(), snapshot("ecs", 10)]).await;
    let cloud = Cloud::local(&mock.endpoint);
    worker::refresh(&pool, id, &cloud).await.unwrap();
    let now = apsp_protocol::now_timestamp();
    costs::tick(&pool, &cloud).await.unwrap();
    costs::refresh_balance(&pool, id, &cloud, now)
        .await
        .unwrap();
    costs::refresh_bill(&pool, resource_id, &cloud, now)
        .await
        .unwrap();
    let a = account(&pool, id).await;
    assert!(a.traffic.is_some());
    assert!(a.error_code.is_none());
    assert!(a.balance_error.is_none());
    assert!(
        super::super::resource(&pool, resource_id)
            .await
            .unwrap()
            .snapshot
            .is_some()
    );
    assert_eq!(
        mock.requests()
            .iter()
            .map(|r| r.action.as_str())
            .collect::<Vec<_>>(),
        ["ListCdtInternetTraffic", "DescribeInstances"]
    );
    mock.exhausted();
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn optional_balance_failure_does_not_enable_bills_or_block_monitoring(pool: PgPool) {
    let (id, resource_id) = seed(&pool, "ecs").await;
    sqlx::query(
        "UPDATE alicloud_accounts SET billing_enabled=false,balance_enabled=true WHERE id=$1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let mock = Mock::start(vec![
        Reply {
            action: "QueryAccountBalance".into(),
            status: 403,
            value: json!({"Code":"Forbidden.RAM", "RequestId":"denied"}),
        },
        billing_cases::traffic(),
        snapshot("ecs", 10),
    ])
    .await;
    let cloud = Cloud::local(&mock.endpoint);
    costs::tick(&pool, &cloud).await.unwrap();
    worker::refresh(&pool, id, &cloud).await.unwrap();
    let a = account(&pool, id).await;
    assert!(a.balance_error.is_some());
    assert!(a.error_code.is_none());
    assert!(a.traffic.is_some());
    assert!(
        super::super::resource(&pool, resource_id)
            .await
            .unwrap()
            .error_code
            .is_none()
    );
    assert_eq!(mock.requests().len(), 3);
    mock.exhausted();
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn new_accounts_default_to_minimum_permissions(pool: PgPool) {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO alicloud_accounts(id,name,access_key_id,access_key_secret) VALUES($1,'测试最小权限',$2,$3)")
        .bind(id).bind(KEY).bind(SECRET).execute(&pool).await.unwrap();
    let a = account(&pool, id).await;
    assert!(!a.billing_enabled);
    assert!(!a.balance_enabled);
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn disabled_billing_never_reuses_cached_evidence_for_automatic_control(pool: PgPool) {
    let (id, resource_id) = seed(&pool, "ecs").await;
    let now = apsp_protocol::now_timestamp();
    let cached = json!({"month":billing::month(now),"queried_at":now,"usage_micro_gb":999_000_000_000_u64,"rows":[]});
    sqlx::query(
        "UPDATE alicloud_accounts SET billing_enabled=false,auto_enabled=true,bill=$2 WHERE id=$1",
    )
    .bind(id)
    .bind(sqlx::types::Json(cached))
    .execute(&pool)
    .await
    .unwrap();
    let a = account(&pool, id).await;
    let r = super::super::resource(&pool, resource_id).await.unwrap();
    assert!(!billing::exceeded(&a, now));
    assert_eq!(r.power_policy.exceeded(&a, now), None);
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn upgrade_preserves_only_existing_billing_dependent_policies(pool: PgPool) {
    let (automatic, _) = seed(&pool, "ecs").await;
    let threshold = Uuid::new_v4();
    let display = Uuid::new_v4();
    for id in [threshold, display] {
        sqlx::query("INSERT INTO alicloud_accounts(id,name,access_key_id,access_key_secret) VALUES($1,'测试升级',$2,$3)")
            .bind(id).bind(KEY).bind(SECRET).execute(&pool).await.unwrap();
    }
    sqlx::query("UPDATE alicloud_accounts SET auto_enabled=true WHERE id=$1")
        .bind(automatic)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id,power_policy) VALUES($1,$2,'测试阈值','ecs','cn-hangzhou','i-threshold', '{\"enabled\":true,\"threshold_action\":\"stop\"}')")
        .bind(Uuid::new_v4()).bind(threshold).execute(&pool).await.unwrap();
    sqlx::raw_sql(
        "ALTER TABLE alicloud_accounts DROP COLUMN billing_enabled, DROP COLUMN balance_enabled",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../crates/panel/migrations/0077_alicloud_optional_bss.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    for (id, expected) in [(automatic, true), (threshold, true), (display, false)] {
        let flags: (bool, bool) = sqlx::query_as(
            "SELECT billing_enabled,balance_enabled FROM alicloud_accounts WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(flags, (expected, false));
    }
}
