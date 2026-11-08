use super::*;

#[tokio::test]
async fn malformed_record_lock_and_weight_never_reach_the_write_guard() {
    for weight in [json!("0"), json!(-1), json!({})] {
        let mut record = tencent_record(public_ip(9), false);
        record["Weight"] = weight;
        let mock = Mock::start(tencent_read(vec![record])).await;
        let result = Providers::local(&mock.endpoint)
            .reconcile_guarded(&configured(Provider::Tencent), public_ip(9), || async {
                Err::<(), _>(Failure::from("lease_lost"))
            })
            .await;
        assert_eq!(result.unwrap_err().code, "invalid_response");
        assert_eq!(mock.requests().len(), 2);
        mock.exhausted();
    }
    for locked in [json!("false"), json!(0), Value::Null] {
        let mut record = ali_record(public_ip(9));
        record["Locked"] = locked;
        let mock = Mock::start(ali_read(vec![record])).await;
        assert_eq!(
            Providers::local(&mock.endpoint)
                .reconcile(&configured(Provider::Aliyun), public_ip(9))
                .await
                .unwrap_err()
                .code,
            "invalid_response"
        );
        assert_eq!(mock.requests().len(), 2);
        mock.exhausted();
    }
}

#[tokio::test]
async fn incomplete_and_malformed_provider_pagination_never_write() {
    let rule = configured(Provider::Huawei);
    for next in [json!(23), json!({}), Value::Null] {
        let mut replies = hw_read(vec![]);
        replies[1].value["links"]["next"] = next;
        let mock = Mock::start(replies).await;
        assert_eq!(
            Providers::local(&mock.endpoint)
                .reconcile(&rule, public_ip(9))
                .await
                .unwrap_err()
                .code,
            "invalid_response"
        );
        assert_eq!(mock.requests().len(), 2);
        mock.exhausted();
    }
    let mut ali = ali_read(vec![ali_record(public_ip(9))]);
    ali[1].value["TotalCount"] = 2.into();
    let mut tencent = tencent_read(vec![tencent_record(public_ip(9), false)]);
    tencent[1].value["Response"]["RecordCountInfo"]["TotalCount"] = 2.into();
    let mut huawei = hw_read(vec![hw_record(&rule, "ACTIVE")]);
    huawei[1].value["links"]["next"] = "https://example.invalid/TEST_ONLY_SECRET".into();
    for (provider, replies) in [
        (Provider::Aliyun, ali),
        (Provider::Tencent, tencent),
        (Provider::Huawei, huawei),
    ] {
        let mock = Mock::start(replies).await;
        assert_eq!(
            Providers::local(&mock.endpoint)
                .reconcile(&configured(provider), public_ip(9))
                .await
                .unwrap_err()
                .code,
            "record_conflict"
        );
        assert_eq!(mock.requests().len(), 2);
        mock.exhausted();
    }
}

#[tokio::test]
async fn lost_create_receipts_cannot_turn_matching_addresses_into_ownership() {
    for provider in [Provider::Aliyun, Provider::Tencent] {
        let rule = configured(provider);
        let mut first = if provider == Provider::Aliyun {
            ali_read(vec![])
        } else {
            tencent_read(vec![])
        };
        first.push(if provider == Provider::Aliyun {
            Reply::ok("AddDomainRecord", json!({"RequestId":"lost-record-id"}))
        } else {
            Reply::ok(
                "CreateRecord",
                json!({"Response":{"RequestId":"lost-record-id"}}),
            )
        });
        let mock = Mock::start(first).await;
        assert_eq!(
            Providers::local(&mock.endpoint)
                .reconcile(&rule, public_ip(9))
                .await
                .unwrap_err()
                .code,
            "invalid_response"
        );
        assert_eq!(mock.requests().len(), 3);
        mock.exhausted();
        let observed = if provider == Provider::Aliyun {
            ali_read(vec![ali_record(public_ip(9))])
        } else {
            tencent_read(vec![tencent_record(public_ip(9), false)])
        };
        let mock = Mock::start(observed).await;
        assert_eq!(
            Providers::local(&mock.endpoint)
                .reconcile(&rule, public_ip(9))
                .await
                .unwrap_err()
                .code,
            "record_not_owned"
        );
        assert_eq!(mock.requests().len(), 2);
        mock.exhausted();
    }
}

#[tokio::test]
async fn huawei_adoption_updates_only_address_fields_and_keeps_description_and_tags() {
    let mut rule = configured(Provider::Huawei);
    rule.config.adopt_existing = true;
    let mut existing = hw_record(&rule, "ACTIVE");
    existing["description"] = "TEST_ONLY previous administrator description".into();
    existing["tags"] = json!([{"key":"TEST_ONLY ownership","value":"other"}]);
    let mut written = existing.clone();
    written["records"] = json!([public_ip(8)]);
    let mut replies = hw_read(vec![existing]);
    replies.push(Reply::ok(
        &format!("PUT /v2/zones/{ZONE}/recordsets/{RECORD}"),
        written,
    ));
    let mock = Mock::start(replies).await;
    let result = Providers::local(&mock.endpoint)
        .reconcile(&rule, public_ip(8))
        .await
        .unwrap();
    assert_eq!(result.record_id, RECORD);
    assert_eq!(result.status, "updated");
    let requests = mock.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].body.get("description").is_none());
    assert!(requests[2].body.get("tags").is_none());
    assert_eq!(requests[2].body["records"], json!([public_ip(8)]));
    mock.exhausted();
}

#[tokio::test]
async fn zone_apex_default_ns_is_not_subdomain_delegation_for_new_providers() {
    for provider in [Provider::Aliyun, Provider::Tencent, Provider::Huawei] {
        let mut rule = configured(provider);
        rule.config.record_name = "example.com".into();
        let replies = match provider {
            Provider::Aliyun => {
                let mut record = ali_record(public_ip(9));
                record["RR"] = "@".into();
                record["Type"] = "NS".into();
                record["Value"] = "ns.example.com.".into();
                ali_read(vec![record])
            }
            Provider::Tencent => {
                let mut record = tencent_record(public_ip(9), false);
                record["Name"] = "@".into();
                record["Type"] = "NS".into();
                record["Value"] = "ns.example.com.".into();
                tencent_read(vec![record])
            }
            Provider::Huawei => {
                let mut record = hw_record(&rule, "ACTIVE");
                record["name"] = "example.com.".into();
                record["type"] = "NS".into();
                record["records"] = json!(["ns1.example.com.", "ns2.example.com."]);
                hw_read(vec![record])
            }
            Provider::Cloudflare => unreachable!(),
        };
        let mock = Mock::start(replies).await;
        let result = Providers::local(&mock.endpoint)
            .reconcile_guarded(&rule, public_ip(9), || async {
                Err::<(), _>(Failure::from("lease_lost"))
            })
            .await;
        assert_eq!(result.unwrap_err().code, "lease_lost");
        assert_eq!(mock.requests().len(), 2);
        mock.exhausted();
    }
}
