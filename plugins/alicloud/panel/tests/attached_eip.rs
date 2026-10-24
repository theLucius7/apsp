use super::*;

fn attached(bandwidth: i64, charge: &str, status: &str) -> Reply {
    let mut reply = snapshot("ecs", 0);
    let row = &mut reply.value["Instances"]["Instance"][0];
    row["InternetChargeType"] = "".into();
    row["PublicIpAddress"]["IpAddress"] = json!([]);
    row["Status"] = status.into();
    row["EipAddress"] = json!({"AllocationId":"eip-testonly","IpAddress":"192.0.2.1","Bandwidth":bandwidth,"InternetChargeType":charge});
    reply
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn attached_eip_reads_actual_network_fields_for_running_and_stopped_ecs(pool: PgPool) {
    let (a, r) = seed(&pool, "ecs").await;
    let account = account(&pool, a).await;
    let resource = super::super::resource(&pool, r).await.unwrap();
    let mock = Mock::start(vec![
        attached(2000, "PayByDominantTraffic", "Stopped"),
        attached(500, "PayByDominantTraffic", "Running"),
    ])
    .await;
    let cloud = Cloud::local(&mock.endpoint);
    for (bandwidth, status) in [(2000, "Stopped"), (500, "Running")] {
        let result = cloud.snapshot(&account, &resource).await.unwrap();
        assert_eq!(result.cloud_id, resource.cloud_id);
        assert_eq!(result.region, resource.region);
        assert_eq!(result.kind, "ecs");
        assert_eq!(result.resource_charge_type, "PostPaid");
        assert_eq!(result.attached_eip_id.as_deref(), Some("eip-testonly"));
        assert_eq!(result.public_ip, "192.0.2.1");
        assert_eq!(result.bandwidth_mbps, bandwidth);
        assert_eq!(result.charge_type, "PayByDominantTraffic");
        assert_eq!(result.status, status);
    }
    assert_eq!(mock.requests().len(), 2);
    assert!(
        mock.requests()
            .iter()
            .all(|r| r.action == "DescribeInstances")
    );
    mock.exhausted();
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn attached_eip_rejects_wrong_identity_ambiguous_or_incomplete_network_data(pool: PgPool) {
    let (a, r) = seed(&pool, "ecs").await;
    let account = account(&pool, a).await;
    let resource = super::super::resource(&pool, r).await.unwrap();
    let cases = [
        ("InstanceId", json!("i-other")),
        ("RegionId", json!("cn-other")),
        ("PublicIpAddress/IpAddress", json!(["192.0.2.2"])),
        ("EipAddress/AllocationId", json!("i-wrongkind")),
        ("EipAddress/AllocationId", Value::Null),
        ("EipAddress/IpAddress", json!("invalid")),
        ("EipAddress/Bandwidth", Value::Null),
        ("EipAddress/Bandwidth", json!(-1)),
        ("EipAddress/Bandwidth", json!(100001)),
        ("EipAddress/Bandwidth", json!(0.5)),
        ("EipAddress/InternetChargeType", json!("")),
        ("EipAddress/InternetChargeType", json!("bad\nvalue")),
    ];
    for (field, value) in cases {
        let mut response = attached(2000, "PayByDominantTraffic", "Running");
        *response
            .value
            .pointer_mut(&format!("/Instances/Instance/0/{field}"))
            .unwrap() = value;
        let mock = Mock::start(vec![response]).await;
        assert!(
            Cloud::local(&mock.endpoint)
                .snapshot(&account, &resource)
                .await
                .is_err(),
            "{field}"
        );
        mock.exhausted();
    }
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn attached_eip_never_authorizes_ecs_or_eip_bandwidth_writes(pool: PgPool) {
    let (a, r) = seed(&pool, "ecs").await;
    let account = account(&pool, a).await;
    let resource = super::super::resource(&pool, r).await.unwrap();
    for charge in ["PayByTraffic", "PayByDominantTraffic"] {
        let mock = Mock::start(vec![
            attached(500, charge, "Running"),
            attached(500, charge, "Running"),
        ])
        .await;
        let cloud = Cloud::local(&mock.endpoint);
        let result = cloud.snapshot(&account, &resource).await.unwrap();
        assert!(target().validate(&result).is_err());
        assert_eq!(
            cloud
                .modify(&account, &resource, &result, &target(), Uuid::new_v4())
                .await
                .unwrap_err()
                .code,
            "attached_eip_read_only"
        );
        assert!(
            operations::preview(&pool, r, target(), 1, &cloud)
                .await
                .is_err()
        );
        assert_eq!(mock.requests().len(), 2);
        assert!(
            mock.requests()
                .iter()
                .all(|r| r.action == "DescribeInstances")
        );
        mock.exhausted();
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM alicloud_operations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn attached_eip_refresh_commits_queries_without_automatic_bandwidth_jobs(pool: PgPool) {
    let (a, r) = seed(&pool, "ecs").await;
    sqlx::query("UPDATE alicloud_accounts SET auto_enabled=true,limit_gb=100 WHERE id=$1")
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE alicloud_resources SET auto_enabled=true,error_code='invalid_response' WHERE id=$1",
    )
    .bind(r)
    .execute(&pool)
    .await
    .unwrap();
    let original = super::super::resource(&pool, r).await.unwrap();
    for charge in ["PayByTraffic", "PayByDominantTraffic"] {
        sqlx::query("UPDATE alicloud_accounts SET next_run_at=0 WHERE id=$1")
            .bind(a)
            .execute(&pool)
            .await
            .unwrap();
        let mock = Mock::start(vec![
            billing_cases::bill(1, 1, vec![billing_cases::row("200", "test-instance")]),
            billing_cases::traffic(),
            attached(2000, charge, "Running"),
        ])
        .await;
        worker::refresh(&pool, a, &Cloud::local(&mock.endpoint))
            .await
            .unwrap();
        let account = account(&pool, a).await;
        assert!(account.error_code.is_none());
        assert!(account.traffic_error.is_none());
        assert_eq!(account.bill.unwrap().usage_micro_gb, Some(200_000_000));
        let updated = super::super::resource(&pool, r).await.unwrap();
        assert!(updated.error_code.is_none());
        assert!(updated.checked_at.is_some());
        assert_eq!(updated.snapshot.unwrap().bandwidth_mbps, 2000);
        assert_eq!(updated.revision, original.revision);
        assert_eq!(
            serde_json::to_value(updated.power_policy).unwrap(),
            serde_json::to_value(&original.power_policy).unwrap()
        );
        assert_eq!(
            serde_json::to_value(updated.cdt_guard_state).unwrap(),
            serde_json::to_value(&original.cdt_guard_state).unwrap()
        );
        assert_eq!(mock.requests().len(), 3);
        mock.exhausted();
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM alicloud_operations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn attaching_eip_after_preview_rejects_queued_ecs_network_change(pool: PgPool) {
    let (_, r) = seed(&pool, "ecs").await;
    let mock = Mock::start(vec![
        snapshot("ecs", 10),
        attached(10, "PayByTraffic", "Running"),
    ])
    .await;
    let cloud = Cloud::local(&mock.endpoint);
    let operation = operations::preview(&pool, r, target(), 1, &cloud)
        .await
        .unwrap();
    operations::confirm(&pool, operation.id).await.unwrap();
    operations::process(&pool, operation.id, &cloud)
        .await
        .unwrap();
    let result = operations::load(&pool, operation.id).await.unwrap();
    assert_eq!(result.status, "failed");
    assert_eq!(result.error_code.as_deref(), Some("state_changed"));
    assert_eq!(mock.requests().len(), 2);
    mock.exhausted();
}

#[sqlx::test(migrations = "../../../crates/panel/migrations")]
async fn attached_eip_matching_ip_and_target_cannot_confirm_uncertain_ecs_change(pool: PgPool) {
    let (_, r) = seed(&pool, "ecs").await;
    let mock = Mock::start(vec![
        snapshot("ecs", 10),
        attached(1, "PayByTraffic", "Running"),
    ])
    .await;
    let cloud = Cloud::local(&mock.endpoint);
    let operation = operations::preview(&pool, r, target(), 1, &cloud)
        .await
        .unwrap();
    sqlx::query("UPDATE alicloud_operations SET status='uncertain',next_check_at=0 WHERE id=$1")
        .bind(operation.id)
        .execute(&pool)
        .await
        .unwrap();
    operations::process(&pool, operation.id, &cloud)
        .await
        .unwrap();
    let result = operations::load(&pool, operation.id).await.unwrap();
    assert_eq!(result.status, "uncertain");
    assert_eq!(result.error_code.as_deref(), Some("awaiting_confirmation"));
    assert_eq!(mock.requests().len(), 2);
    mock.exhausted();
}
