use crate::cloud::{
    automation::{self, AccountAutomation},
    bandwidth, worker,
};
use apsp_cloud_api::test_support::{Mock, Reply};
use apsp_costs::cloud::billing;
use apsp_resources::cloud::{
    client::Cloud,
    model::{Account, Target},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

mod attached_eip;
mod billing_cases;
mod least_privilege;
mod rotation_refresh;

const SECRET: &str = "TEST_ONLY_CLOUD_SECRET";
/// Registers an account that references a credential entry; the test handle answers
/// every reference with its fixed keys.
pub async fn credential_account(pool: &PgPool, id: Uuid, name: &str) {
    sqlx::query("WITH credential AS (INSERT INTO credential_entries(id,name,kind,key_id,nonce,ciphertext,created_at,updated_at) VALUES(gen_random_uuid(),'TEST_ONLY','cloud','TEST_ONLY',decode(repeat('00',12),'hex'),decode('00','hex'),0,0) RETURNING id) INSERT INTO alicloud_accounts(id,name,credential_id) SELECT $1,$2,id FROM credential")
        .bind(id).bind(name).execute(pool).await.unwrap();
}
/// Sets which of the account's bills costs reads.
pub async fn set_billing(pool: &PgPool, account: Uuid, bills: bool, balance: bool) {
    sqlx::query("INSERT INTO cloud_account_billing(account_id,billing_enabled,balance_enabled) VALUES($1,$2,$3) ON CONFLICT(account_id) DO UPDATE SET billing_enabled=EXCLUDED.billing_enabled,balance_enabled=EXCLUDED.balance_enabled")
        .bind(account).bind(bills).bind(balance).execute(pool).await.unwrap();
}
/// Gives the account and the resource the rows operations keeps about them, holding the
/// defaults, as the account and resource forms do.
pub async fn automate(pool: &PgPool, account: Uuid, resource: Uuid) {
    sqlx::query(
        "INSERT INTO cloud_account_automation(account_id) VALUES($1) ON CONFLICT DO NOTHING",
    )
    .bind(account)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cloud_resource_automation(resource_id) VALUES($1) ON CONFLICT DO NOTHING",
    )
    .bind(resource)
    .execute(pool)
    .await
    .unwrap();
}
pub async fn seed(pool: &PgPool, kind: &str) -> (Uuid, Uuid) {
    let account = Uuid::new_v4();
    let resource = Uuid::new_v4();
    credential_account(pool, account, "测试账号").await;
    set_billing(pool, account, true, true).await;
    sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id) VALUES($1,$2,'测试资源',$3,'cn-hangzhou',$4)")
        .bind(resource).bind(account).bind(kind).bind(if kind=="ecs"{"i-testonly"}else{"eip-testonly"}).execute(pool).await.unwrap();
    automate(pool, account, resource).await;
    (account, resource)
}
async fn account(pool: &PgPool, id: Uuid) -> Account {
    sqlx::query_as("SELECT * FROM alicloud_accounts WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn automatic(pool: &PgPool, id: Uuid) -> AccountAutomation {
    automation::account(pool, id).await.unwrap()
}
async fn costs(pool: &PgPool, id: Uuid) -> apsp_costs::cloud::accounts::AccountBilling {
    apsp_costs::cloud::accounts::account(pool, id)
        .await
        .unwrap()
}
fn target() -> Target {
    Target {
        bandwidth_mbps: 1,
        charge_type: "PayByTraffic".into(),
    }
}
fn snapshot(kind: &str, bandwidth: i64) -> Reply {
    let (action, value) = if kind == "ecs" {
        (
            "DescribeInstances",
            json!({"Instances":{"Instance":[{"InstanceId":"i-testonly","RegionId":"cn-hangzhou","PublicIpAddress":{"IpAddress":["192.0.2.1"]},"InternetMaxBandwidthOut":bandwidth,"InternetChargeType":"PayByTraffic","InstanceChargeType":"PostPaid","Status":"Running"}]}}),
        )
    } else {
        (
            "DescribeEipAddresses",
            json!({"EipAddresses":{"EipAddress":[{"AllocationId":"eip-testonly","RegionId":"cn-hangzhou","IpAddress":"192.0.2.1","Bandwidth":bandwidth.to_string(),"InternetChargeType":"PayByTraffic","ChargeType":"PostPaid","Status":"InUse","Netmode":"public","BandwidthPackageId":""}]}}),
        )
    };
    let mut value = value;
    value["RequestId"] = "read-request".into();
    value["TotalCount"] = 1.into();
    Reply::ok(action, value)
}
async fn due(pool: &PgPool, id: Uuid) {
    sqlx::query("UPDATE alicloud_operations SET next_check_at=0 WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn ecs_preview_is_read_only_confirmation_is_idempotent_and_change_is_verified(pool: PgPool) {
    let (_, id) = seed(&pool, "ecs").await;
    let mock = Mock::start(vec![
        snapshot("ecs", 10),
        snapshot("ecs", 10),
        Reply::ok(
            "ModifyInstanceNetworkSpec",
            json!({"RequestId":"write-request"}),
        ),
        snapshot("ecs", 1),
    ])
    .await;
    let cloud = Cloud::local(&mock.endpoint);
    let preview = bandwidth::preview(&pool, id, target(), 1, &cloud)
        .await
        .unwrap();
    assert_eq!(preview.status, "preview");
    assert_eq!(mock.requests().len(), 1);
    assert_eq!(
        bandwidth::confirm(&pool, preview.id).await.unwrap().status,
        "queued"
    );
    bandwidth::confirm(&pool, preview.id).await.unwrap();
    bandwidth::process(&pool, preview.id, &cloud).await.unwrap();
    assert_eq!(
        bandwidth::load(&pool, preview.id).await.unwrap().status,
        "succeeded"
    );
    bandwidth::process(&pool, preview.id, &cloud).await.unwrap();
    bandwidth::confirm(&pool, preview.id).await.unwrap();
    let requests = mock.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.action.starts_with("Modify"))
            .count(),
        1
    );
    let params = &requests[2].params;
    assert_eq!(params["ClientToken"], preview.id.to_string());
    assert_eq!(params["AllocatePublicIp"], "false");
    assert_eq!(params["AutoPay"], "true");
    assert!(!params.contains_key("NetworkChargeType"));
    assert_eq!(requests.len(), 4);
    mock.exhausted();
}
#[sqlx::test(migrations = "../panel/migrations")]
async fn eip_adjusts_bandwidth_only_and_refuses_charge_conversion_and_shared_packages(
    pool: PgPool,
) {
    let (account_id, id) = seed(&pool, "eip").await;
    let a = account(&pool, account_id).await;
    let r = apsp_resources::cloud::resource(&pool, id).await.unwrap();
    let mut shared = snapshot("eip", 10);
    shared.value["EipAddresses"]["EipAddress"][0]["BandwidthPackageId"] = "cbwp-testonly".into();
    let mock = Mock::start(vec![shared]).await;
    assert_eq!(
        Cloud::local(&mock.endpoint)
            .snapshot(&a, &r)
            .await
            .unwrap_err()
            .code,
        "unsupported_resource"
    );
    mock.exhausted();
    let mock = Mock::start(vec![snapshot("eip", 10)]).await;
    let invalid = Target {
        bandwidth_mbps: 1,
        charge_type: "PayByBandwidth".into(),
    };
    assert!(
        bandwidth::preview(&pool, id, invalid, 1, &Cloud::local(&mock.endpoint))
            .await
            .is_err()
    );
    mock.exhausted();
    let mock = Mock::start(vec![
        snapshot("eip", 10),
        snapshot("eip", 10),
        Reply::ok(
            "ModifyEipAddressAttribute",
            json!({"RequestId":"write-eip"}),
        ),
        snapshot("eip", 1),
    ])
    .await;
    let cloud = Cloud::local(&mock.endpoint);
    let preview = bandwidth::preview(&pool, id, target(), 1, &cloud)
        .await
        .unwrap();
    bandwidth::confirm(&pool, preview.id).await.unwrap();
    bandwidth::process(&pool, preview.id, &cloud).await.unwrap();
    let params = &mock.requests()[2].params;
    assert_eq!(params["AllocationId"], "eip-testonly");
    assert_eq!(params["Bandwidth"], "1");
    assert!(!params.contains_key("InternetChargeType"));
    assert!(!params.contains_key("ClientToken"));
    assert_eq!(
        bandwidth::load(&pool, preview.id).await.unwrap().status,
        "succeeded"
    );
    mock.exhausted();
}
#[sqlx::test(migrations = "../panel/migrations")]
async fn uncertain_write_survives_restart_and_never_replays_eip_mutation(pool: PgPool) {
    let (_, id) = seed(&pool, "eip").await;
    let mock = Mock::start(vec![
        snapshot("eip", 10),
        snapshot("eip", 10),
        Reply {
            action: "ModifyEipAddressAttribute".into(),
            status: 503,
            value: json!({"secret":SECRET}),
        },
    ])
    .await;
    let cloud = Cloud::local(&mock.endpoint);
    let preview = bandwidth::preview(&pool, id, target(), 1, &cloud)
        .await
        .unwrap();
    bandwidth::confirm(&pool, preview.id).await.unwrap();
    bandwidth::process(&pool, preview.id, &cloud).await.unwrap();
    mock.exhausted();
    let operation = bandwidth::load(&pool, preview.id).await.unwrap();
    assert_eq!(operation.status, "uncertain");
    assert!(!serde_json::to_string(&operation).unwrap().contains(SECRET));
    assert!(
        bandwidth::preview(&pool, id, target(), 1, &cloud)
            .await
            .is_err()
    );
    // A fresh client represents a restarted panel. It only issues a read.
    let restored = Mock::start(vec![snapshot("eip", 1)]).await;
    due(&pool, preview.id).await;
    bandwidth::process(&pool, preview.id, &Cloud::local(&restored.endpoint))
        .await
        .unwrap();
    assert_eq!(
        bandwidth::load(&pool, preview.id).await.unwrap().status,
        "succeeded"
    );
    restored.exhausted();
}
#[sqlx::test(migrations = "../panel/migrations")]
async fn committed_intent_and_external_drift_are_never_blindly_reapplied(pool: PgPool) {
    let (_, id) = seed(&pool, "ecs").await;
    let mock = Mock::start(vec![snapshot("ecs", 10), snapshot("ecs", 20)]).await;
    let cloud = Cloud::local(&mock.endpoint);
    let preview = bandwidth::preview(&pool, id, target(), 1, &cloud)
        .await
        .unwrap();
    bandwidth::confirm(&pool, preview.id).await.unwrap();
    bandwidth::process(&pool, preview.id, &cloud).await.unwrap();
    assert_eq!(
        bandwidth::load(&pool, preview.id)
            .await
            .unwrap()
            .error_code
            .as_deref(),
        Some("state_changed")
    );
    mock.exhausted();
    let mock = Mock::start(vec![snapshot("ecs", 10), snapshot("ecs", 10)]).await;
    let cloud = Cloud::local(&mock.endpoint);
    let preview = bandwidth::preview(&pool, id, target(), 1, &cloud)
        .await
        .unwrap();
    bandwidth::confirm(&pool, preview.id).await.unwrap();
    sqlx::query("UPDATE alicloud_operations SET status='running' WHERE id=$1")
        .bind(preview.id)
        .execute(&pool)
        .await
        .unwrap();
    bandwidth::process(&pool, preview.id, &cloud).await.unwrap();
    assert_eq!(
        bandwidth::load(&pool, preview.id).await.unwrap().status,
        "uncertain"
    );
    mock.exhausted();
    sqlx::query("UPDATE cloud_resource_automation SET auto_enabled=true WHERE resource_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    bandwidth::cancel(&pool, preview.id, true).await.unwrap();
    assert!(!automation::resource(&pool, id).await.unwrap().auto_enabled);
    assert_eq!(
        bandwidth::load(&pool, preview.id).await.unwrap().status,
        "dismissed"
    );
}
#[sqlx::test(migrations = "../panel/migrations")]
async fn cancelled_or_revised_policy_does_not_reach_the_cloud(pool: PgPool) {
    let (account_id, id) = seed(&pool, "ecs").await;
    let mock = Mock::start(vec![snapshot("ecs", 10), snapshot("ecs", 10)]).await;
    let cloud = Cloud::local(&mock.endpoint);
    let preview = bandwidth::preview(&pool, id, target(), 1, &cloud)
        .await
        .unwrap();
    bandwidth::confirm(&pool, preview.id).await.unwrap();
    bandwidth::cancel(&pool, preview.id, false).await.unwrap();
    bandwidth::process(&pool, preview.id, &cloud).await.unwrap();
    let second = bandwidth::preview(&pool, id, target(), 1, &cloud)
        .await
        .unwrap();
    bandwidth::confirm(&pool, second.id).await.unwrap();
    let mut tx = apsp_resources::cloud::lock(&pool, account_id)
        .await
        .unwrap();
    sqlx::query("UPDATE alicloud_accounts SET enabled=false,revision=revision+1 WHERE id=$1")
        .bind(account_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    bandwidth::process(&pool, second.id, &cloud).await.unwrap();
    assert_eq!(
        bandwidth::load(&pool, second.id).await.unwrap().status,
        "cancelled"
    );
    mock.exhausted();
}

/// A security group change that was sent and whose result is unknown keeps the resource
/// busy for every other cloud operation.
#[sqlx::test(migrations = "../panel/migrations")]
async fn queued_cloud_operations_are_blocked_by_unknown_membership_mutations(pool: PgPool) {
    let (account, resource) = seed(&pool, "ecs").await;
    let admin: i64 = sqlx::query_scalar(
        "INSERT INTO admins(password_hash) VALUES('TEST_ONLY_not_used') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO alicloud_security_group_operations(id,resource_id,requested_by,before_state,target_groups,impact,snapshot_digest,status,created_at,expires_at,updated_at) VALUES($1,$2,$3,'{}',ARRAY['sg-base'],'{}','fixture','unknown',1,2,1)")
        .bind(Uuid::new_v4()).bind(resource).bind(admin).execute(&pool).await.unwrap();
    let mut tx = apsp_resources::cloud::lock(&pool, account).await.unwrap();
    assert!(bandwidth::idle(&mut tx, resource).await.is_err());
}

/// An account or resource without a row of operations' reads exactly like one holding the
/// column defaults: the defaults in code are the columns' defaults, and the stored default
/// policy the operations page shows for a missing row is the column's.
#[sqlx::test(migrations = "../panel/migrations")]
async fn missing_automation_rows_read_as_the_column_defaults(pool: PgPool) {
    use crate::cloud::automation::ResourceAutomation;
    use crate::cloud::power::model::{DEFAULT_POLICY, Policy};
    fn value(item: impl serde::Serialize) -> Value {
        serde_json::to_value(item).unwrap()
    }
    let (account, resource) = seed(&pool, "ecs").await;
    let (bare_account, bare_resource) = (Uuid::new_v4(), Uuid::new_v4());
    credential_account(&pool, bare_account, "测试账号").await;
    sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id) VALUES($1,$2,'测试资源','ecs','cn-hangzhou','i-bare-testonly')")
        .bind(bare_resource).bind(bare_account).execute(&pool).await.unwrap();
    for id in [account, bare_account] {
        assert_eq!(
            value(automatic(&pool, id).await),
            value(AccountAutomation::default())
        );
    }
    for id in [resource, bare_resource] {
        assert_eq!(
            value(automation::resource(&pool, id).await.unwrap()),
            value(ResourceAutomation::default())
        );
    }
    let stored: Value = sqlx::query_scalar(
        "SELECT power_policy FROM cloud_resource_automation WHERE resource_id=$1",
    )
    .bind(resource)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        serde_json::from_str::<Value>(DEFAULT_POLICY).unwrap()
    );
    assert_eq!(
        value(serde_json::from_str::<Policy>(DEFAULT_POLICY).unwrap()),
        value(Policy::default())
    );
    let views = automation::resource_views(&pool, &[resource, bare_resource])
        .await
        .unwrap();
    assert_eq!(views[&resource], views[&bare_resource]);
    assert_eq!(
        views[&bare_resource],
        json!({"manual_hold":false,"power_policy":stored,"power_state":null,"power_error":null})
    );
}
