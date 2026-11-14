use super::*;
use crate::cloud::power::PolicyRules;
use apsp_costs::cloud::cost_cache;
use apsp_resources::cloud::billing::{Change, Settings};

#[sqlx::test(migrations = "../panel/migrations")]
async fn minimum_permissions_skip_all_bss_calls_and_keep_cdt_and_ecs_queries(pool: PgPool) {
    let (id, resource_id) = seed(&pool, "ecs").await;
    super::set_billing(&pool, id, false, false).await;
    let mock = Mock::start(vec![billing_cases::traffic(), snapshot("ecs", 10)]).await;
    let cloud = Cloud::local(&mock.endpoint);
    worker::refresh(&pool, id, &cloud).await.unwrap();
    let now = apsp_protocol::now_timestamp();
    cost_cache::tick(&pool, &cloud).await.unwrap();
    cost_cache::refresh_balance(&pool, id, &cloud, now)
        .await
        .unwrap();
    cost_cache::refresh_bill(&pool, resource_id, &cloud, now)
        .await
        .unwrap();
    let a = costs(&pool, id).await;
    assert!(a.traffic.is_some());
    assert!(a.bill_error.is_none());
    assert!(a.balance_error.is_none());
    assert!(
        apsp_resources::cloud::resource(&pool, resource_id)
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

#[sqlx::test(migrations = "../panel/migrations")]
async fn optional_balance_failure_does_not_enable_bills_or_block_monitoring(pool: PgPool) {
    let (id, resource_id) = seed(&pool, "ecs").await;
    super::set_billing(&pool, id, false, true).await;
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
    cost_cache::tick(&pool, &cloud).await.unwrap();
    worker::refresh(&pool, id, &cloud).await.unwrap();
    let a = costs(&pool, id).await;
    assert!(a.balance_error.is_some());
    assert!(a.bill_error.is_none());
    assert!(a.traffic.is_some());
    assert!(
        apsp_resources::cloud::resource(&pool, resource_id)
            .await
            .unwrap()
            .error_code
            .is_none()
    );
    assert_eq!(mock.requests().len(), 3);
    mock.exhausted();
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn new_accounts_default_to_minimum_permissions(pool: PgPool) {
    let id = Uuid::new_v4();
    credential_account(&pool, id, "测试最小权限").await;
    let a = costs(&pool, id).await;
    assert!(!a.billing_enabled);
    assert!(!a.balance_enabled);
    // The account form saves the defaults through costs' answer.
    apsp_costs::cloud::accounts::install();
    let mut tx = pool.begin().await.unwrap();
    apsp_resources::cloud::billing::changed(&mut tx, id, Change::Created(Settings::default()))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let stored: (bool, bool) = sqlx::query_as(
        "SELECT billing_enabled,balance_enabled FROM cloud_account_billing WHERE account_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, (false, false));
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn disabled_billing_never_reuses_cached_evidence_for_automatic_control(pool: PgPool) {
    let (id, resource_id) = seed(&pool, "ecs").await;
    let now = apsp_protocol::now_timestamp();
    let cached = json!({"month":billing::month(now),"queried_at":now,"usage_micro_gb":999_000_000_000_u64,"rows":[]});
    sqlx::query("UPDATE alicloud_accounts SET auto_enabled=true WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE cloud_account_billing SET billing_enabled=false,bill=$2 WHERE account_id=$1",
    )
    .bind(id)
    .bind(sqlx::types::Json(cached))
    .execute(&pool)
    .await
    .unwrap();
    let a = account(&pool, id).await;
    let c = costs(&pool, id).await;
    let r = apsp_resources::cloud::resource(&pool, resource_id)
        .await
        .unwrap();
    assert!(!billing::exceeded(&a, &c, now));
    assert_eq!(r.power_policy.exceeded(&a, &c, now), None);
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn reopening_does_not_implicitly_enable_billable_services(pool: PgPool) {
    let (automatic, _) = seed(&pool, "ecs").await;
    let threshold = Uuid::new_v4();
    let display = Uuid::new_v4();
    for id in [threshold, display] {
        credential_account(&pool, id, "测试升级").await;
    }
    sqlx::query("UPDATE alicloud_accounts SET auto_enabled=true WHERE id=$1")
        .bind(automatic)
        .execute(&pool)
        .await
        .unwrap();
    super::set_billing(&pool, automatic, true, false).await;
    sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id,power_policy) VALUES($1,$2,'测试阈值','ecs','cn-hangzhou','i-threshold', '{\"enabled\":true,\"threshold_action\":\"stop\"}')")
        .bind(Uuid::new_v4()).bind(threshold).execute(&pool).await.unwrap();
    apsp_panel_host::schema::initialize(&pool).await.unwrap();
    for (id, expected) in [(automatic, true), (threshold, false), (display, false)] {
        let stored = costs(&pool, id).await;
        let flags = (stored.billing_enabled, stored.balance_enabled);
        assert_eq!(flags, (expected, false));
    }
}
