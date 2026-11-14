use super::*;

#[sqlx::test]
async fn unresolved_cloud_receipts_block_credential_identity_changes_but_allow_disable(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let account = Uuid::new_v4();
    let bandwidth_resource = Uuid::new_v4();
    let power_resource = Uuid::new_v4();
    let bandwidth = Uuid::new_v4();
    let power = Uuid::new_v4();
    let now = apsp_protocol::now_timestamp();
    let original = credential(&pool).await?;
    let replacement = credential(&pool).await?;
    sqlx::query("INSERT INTO alicloud_accounts(id,name,credential_id) VALUES($1,'测试账号',$2)")
        .bind(account)
        .bind(original)
        .execute(&pool)
        .await?;
    for (id, cloud_id) in [
        (bandwidth_resource, "i-bandwidth-testonly"),
        (power_resource, "i-power-testonly"),
    ] {
        sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id) VALUES($1,$2,'测试实例','ecs','cn-hangzhou',$3)")
            .bind(id).bind(account).bind(cloud_id).execute(&pool).await?;
    }
    let snapshot = json!({"kind":"ecs","cloud_id":"i-bandwidth-testonly","region":"cn-hangzhou","public_ip":"192.0.2.1","bandwidth_mbps":10,"charge_type":"PayByTraffic","resource_charge_type":"PostPaid","status":"Running"});
    sqlx::query("INSERT INTO alicloud_operations(id,resource_id,account_revision,resource_revision,before_state,target,source,status,created_at,expires_at,updated_at) VALUES($1,$2,1,1,$3,$4,'manual','uncertain',$5,$6,$5)")
        .bind(bandwidth).bind(bandwidth_resource).bind(snapshot).bind(json!({"bandwidth_mbps":1,"charge_type":"PayByTraffic"})).bind(now).bind(now+300).execute(&pool).await?;
    let state = json!({"cloud_id":"i-power-testonly","region":"cn-hangzhou","status":"Running","stopped_mode":"KeepCharging","charge_type":"PostPaid","network_type":"vpc","spot_strategy":"NoSpot","interruption_behavior":null,"public_ips":["192.0.2.2"],"locked":false});
    sqlx::query("INSERT INTO alicloud_power_jobs(id,resource_id,account_revision,resource_revision,action,stop_mode,source,before_state,status,created_at,expires_at,updated_at) VALUES($1,$2,1,1,'stop','KeepCharging','manual',$3,'running',$4,$5,$4)")
        .bind(power).bind(power_resource).bind(state).bind(now).bind(now+300).execute(&pool).await?;
    let path = format!("/api/resources/alicloud/accounts/{account}");
    let body = json!({"name":"测试账号","site":"china","enabled":true,"auto_enabled":false,"limit_gb":100,"revision":1});
    let mut rotate = body.clone();
    rotate["credential_id"] = replacement.to_string().into();
    let mut move_site = body.clone();
    move_site["site"] = "international".into();
    for edit in [rotate.clone(), move_site] {
        assert_eq!(
            panel
                .admin(Method::PATCH, &path, &cookie, Some(edit))
                .await?
                .status(),
            StatusCode::CONFLICT
        );
    }
    let stored: (String, Option<Uuid>, i64) =
        sqlx::query_as("SELECT site,credential_id,revision FROM alicloud_accounts WHERE id=$1")
            .bind(account)
            .fetch_one(&pool)
            .await?;
    assert_eq!(stored, ("china".into(), Some(original), 1));
    let mut disable = body;
    disable["enabled"] = false.into();
    disable["name"] = "暂停核对账号".into();
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(disable))
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("/api/resources/alicloud/operations/{bandwidth}/dismiss"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    rotate["revision"] = 2.into();
    rotate["enabled"] = false.into();
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(rotate.clone()))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let stored: (Option<Uuid>, bool, i64) =
        sqlx::query_as("SELECT credential_id,enabled,revision FROM alicloud_accounts WHERE id=$1")
            .bind(account)
            .fetch_one(&pool)
            .await?;
    assert_eq!(stored, (Some(original), false, 2));
    sqlx::query("UPDATE alicloud_power_jobs SET status='uncertain' WHERE id=$1")
        .bind(power)
        .execute(&pool)
        .await?;
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("/api/resources/alicloud/power-jobs/{power}/dismiss"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(rotate))
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let stored: (Option<Uuid>, i64) =
        sqlx::query_as("SELECT credential_id,revision FROM alicloud_accounts WHERE id=$1")
            .bind(account)
            .fetch_one(&pool)
            .await?;
    assert_eq!(stored, (Some(replacement), 3));
    let overview: Value = panel
        .admin(Method::GET, "/api/resources/alicloud", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(!overview.to_string().contains("access_key"));
    Ok(())
}

#[sqlx::test]
async fn supplied_delete_revisions_reject_stale_resource_and_account_views(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let account = Uuid::new_v4();
    let resource = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO alicloud_accounts(id,name,credential_id,revision) VALUES($1,'测试账号',$2,2)",
    )
    .bind(account)
    .bind(credential(&pool).await?)
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id,revision) VALUES($1,$2,'测试实例','ecs','cn-hangzhou','i-testonly',2)")
        .bind(resource).bind(account).execute(&pool).await?;
    for (path, table, id) in [
        (
            format!("/api/resources/alicloud/resources/{resource}"),
            "alicloud_resources",
            resource,
        ),
        (
            format!("/api/resources/alicloud/accounts/{account}"),
            "alicloud_accounts",
            account,
        ),
    ] {
        assert_eq!(
            panel
                .admin(Method::DELETE, &path, &cookie, Some(json!({"revision":1})))
                .await?
                .status(),
            StatusCode::CONFLICT
        );
        let archived: bool =
            sqlx::query_scalar(&format!("SELECT archived FROM {table} WHERE id=$1"))
                .bind(id)
                .fetch_one(&pool)
                .await?;
        assert!(!archived);
        assert_eq!(
            panel
                .admin(Method::DELETE, &path, &cookie, Some(json!({"revision":2})))
                .await?
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    Ok(())
}

#[sqlx::test]
async fn bss_opt_ins_round_trip_preserve_omitted_fields_and_protect_billing_policies(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let mut body = json!({"name":"最小权限","site":"china","enabled":false,"auto_enabled":false,"limit_gb":100,"credential_id":credential(&pool).await?});
    let created: Value = panel
        .admin(
            Method::POST,
            "/api/resources/alicloud/accounts",
            &cookie,
            Some(body.clone()),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = created["id"].as_str().unwrap();
    let path = format!("/api/resources/alicloud/accounts/{id}");
    let flags: (bool, bool) = sqlx::query_as(
        "SELECT billing_enabled,balance_enabled FROM cloud_account_billing WHERE account_id=$1::text::uuid",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(flags, (false, false));
    body["revision"] = 1.into();
    body["balance_enabled"] = true.into();
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(body.clone()))
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    body["revision"] = 2.into();
    body.as_object_mut().unwrap().remove("balance_enabled");
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(body.clone()))
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let overview: Value = panel
        .admin(Method::GET, "/api/resources/alicloud", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(overview["accounts"][0]["balance_enabled"], true);
    assert_eq!(overview["accounts"][0]["billing_enabled"], false);
    body["revision"] = 3.into();
    body["auto_enabled"] = true.into();
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(body.clone()))
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    body["billing_enabled"] = true.into();
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(body.clone()))
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    body["revision"] = 4.into();
    body["billing_enabled"] = false.into();
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(body.clone()))
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    body["auto_enabled"] = false.into();
    body["balance_enabled"] = false.into();
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(body))
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let flags: (bool, bool) = sqlx::query_as(
        "SELECT billing_enabled,balance_enabled FROM cloud_account_billing WHERE account_id=$1::text::uuid",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(flags, (false, false));
    Ok(())
}

/// The keys of an object in a view, in the order the response lists them.
fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// What costs keeps of cloud accounts and resources moved to its own tables (ADR 0137 step
/// 6); both cloud pages still show it under the keys and values the account and resource
/// rows gave them, objects without a costs row read as the old column defaults, and every
/// response lists its keys in order.
#[sqlx::test]
async fn cloud_views_keep_their_keys_and_values_after_costs_moved_out(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let now = apsp_protocol::now_timestamp();
    let (account, bare_account) = (Uuid::new_v4(), Uuid::new_v4());
    let (resource, bare_resource) = (Uuid::new_v4(), Uuid::new_v4());
    let reference = credential(&pool).await?;
    sqlx::query("INSERT INTO alicloud_accounts(id,name,site,enabled,auto_enabled,limit_gb,revision,next_run_at,last_attempt_at,credential_id) VALUES($1,'TEST_ONLY a','international',true,true,250,7,$2,$3,$4)")
        .bind(account).bind(now + 300).bind(now).bind(reference).execute(&pool).await?;
    sqlx::query("INSERT INTO alicloud_accounts(id,name,credential_id) VALUES($1,'TEST_ONLY b',$2)")
        .bind(bare_account)
        .bind(reference)
        .execute(&pool)
        .await?;
    let bill = json!({"month":"2026-11","queried_at":now - 60,"rows":[{"instance_id":"i-testonly","region":"cn-hangzhou","product_type":"cdt","billing_item":"internet_traffic","usage":"12.5","unit":"GB","amount":"1.25","currency":"CNY"}],"usage_micro_gb":12_500_000});
    let traffic = json!({"queried_at":now - 59,"mainland_bytes":"10","overseas_bytes":"9007199254740993","regions":[{"region":"cn-hangzhou","bytes":"10"}]});
    let balance = json!({"available":"12.30","currency":"USD","queried_at":now - 58});
    sqlx::query("INSERT INTO cloud_account_billing(account_id,billing_enabled,balance_enabled,bill,bill_error,traffic,traffic_error,traffic_history_identity,balance,balance_error,balance_next_at) VALUES($1,true,true,$2,'rate_limited',$3,'network_error','TEST_ONLY identity',$4,'invalid_response',$5)")
        .bind(account).bind(&bill).bind(&traffic).bind(&balance).bind(now + 21600).execute(&pool).await?;
    let snapshot = json!({"kind":"ecs","cloud_id":"i-testonly","region":"cn-hangzhou","public_ip":"192.0.2.1","bandwidth_mbps":10,"charge_type":"PayByTraffic","resource_charge_type":"PostPaid","status":"Running"});
    for (id, name, cloud_id) in [
        (resource, "TEST_ONLY a", "i-testonly"),
        (bare_resource, "TEST_ONLY b", "i-bare-testonly"),
    ] {
        sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id,snapshot,checked_at,error_code) VALUES($1,$2,$3,'ecs','cn-hangzhou',$4,$5,$6,'resource_busy')")
            .bind(id).bind(account).bind(name).bind(cloud_id).bind(&snapshot).bind(now - 30).execute(&pool).await?;
    }
    let instance_bill = json!({"month":"2026-11","queried_at":now - 57,"rows":[{"item":"PayAsYouGoBill","amount":"12.3456","currency":"CNY"},{"item":"Refund","amount":"-1.25","currency":"USD"},{"item":"PayAsYouGoBill","amount":"1.2","currency":"CNY"}]});
    sqlx::query("INSERT INTO cloud_resource_billing(resource_id,instance_bill,bill_error,bill_next_at) VALUES($1,$2,'billing_incomplete',$3)")
        .bind(resource).bind(&instance_bill).bind(now + 21600).execute(&pool).await?;

    let text = panel
        .admin(Method::GET, "/api/resources/alicloud", &cookie, None)
        .await?
        .error_for_status()?
        .text()
        .await?;
    let overview: Value = serde_json::from_str(&text)?;
    assert_eq!(text, serde_json::to_string(&overview)?);
    assert_eq!(
        overview["accounts"],
        json!([
            {"auto_enabled":true,"balance":balance,"balance_enabled":true,"balance_error":"invalid_response","balance_next_at":now + 21600,"bill":bill,"billing_enabled":true,"credential_id":reference,"enabled":true,"error_code":"rate_limited","id":account,"limit_gb":250,"name":"TEST_ONLY a","next_run_at":now + 300,"revision":7,"site":"international","traffic":traffic,"traffic_error":"network_error"},
            {"auto_enabled":false,"balance":null,"balance_enabled":false,"balance_error":null,"balance_next_at":0,"bill":null,"billing_enabled":false,"credential_id":reference,"enabled":true,"error_code":null,"id":bare_account,"limit_gb":100,"name":"TEST_ONLY b","next_run_at":0,"revision":1,"site":"china","traffic":null,"traffic_error":null}
        ])
    );
    let resource_keys = [
        "account_id",
        "auto_enabled",
        "bill_error",
        "bill_next_at",
        "cap_mbps",
        "cdt_guard_error",
        "cdt_guard_next_at",
        "cdt_guard_state",
        "checked_at",
        "cloud_id",
        "error_code",
        "id",
        "instance_bill",
        "ipv6_addresses",
        "ipv6_checked_at",
        "ipv6_error",
        "kind",
        "manual_hold",
        "name",
        "next_power_at",
        "power_checked_at",
        "power_error",
        "power_policy",
        "power_state",
        "region",
        "revision",
        "snapshot",
        "threshold_hold",
    ];
    let listed = overview["resources"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    for item in listed {
        assert_eq!(keys(item), resource_keys);
        assert_eq!(item["snapshot"], snapshot);
        assert_eq!(item["error_code"], "resource_busy");
    }
    assert_eq!(listed[0]["id"], resource.to_string());
    assert_eq!(listed[0]["instance_bill"], instance_bill);
    assert_eq!(listed[0]["bill_error"], "billing_incomplete");
    assert_eq!(listed[0]["bill_next_at"], now + 21600);
    assert_eq!(listed[1]["id"], bare_resource.to_string());
    assert!(listed[1]["instance_bill"].is_null() && listed[1]["bill_error"].is_null());
    assert_eq!(listed[1]["bill_next_at"], 0);

    let text = panel
        .admin(Method::GET, "/api/operations/cloud", &cookie, None)
        .await?
        .error_for_status()?
        .text()
        .await?;
    let page: Value = serde_json::from_str(&text)?;
    assert_eq!(text, serde_json::to_string(&page)?);
    assert_eq!(keys(&page), ["comparison", "providers", "resources"]);
    let listed = page["resources"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    for item in listed {
        assert_eq!(
            keys(item),
            [
                "account_id",
                "account_name",
                "actual_costs_by_currency",
                "balance",
                "balance_error",
                "balance_scope",
                "bill_error",
                "bill_stale",
                "checked_at",
                "cloud_id",
                "error_code",
                "id",
                "instance_bill",
                "kind",
                "link",
                "manual_hold",
                "name",
                "power_error",
                "power_policy",
                "power_state",
                "provider",
                "provider_read_only_source",
                "region",
                "snapshot",
            ]
        );
        assert_eq!(item["account_name"], "TEST_ONLY a");
        assert_eq!(item["balance"], balance);
        assert_eq!(item["balance_error"], "invalid_response");
        assert_eq!(item["balance_scope"], "cloud_account_global");
        assert_eq!(item["checked_at"], now - 30);
        assert!(item["link"].is_null());
        assert_eq!(
            item["provider_read_only_source"],
            "阿里云 ECS/VPC/BSS 官方 API 缓存"
        );
    }
    assert_eq!(listed[0]["id"], resource.to_string());
    assert_eq!(listed[0]["instance_bill"], instance_bill);
    assert_eq!(listed[0]["bill_error"], "billing_incomplete");
    assert_eq!(
        listed[0]["actual_costs_by_currency"],
        json!({"CNY":"13.5456","USD":"-1.25"})
    );
    assert_eq!(listed[0]["bill_stale"], false);
    assert_eq!(listed[1]["id"], bare_resource.to_string());
    assert!(listed[1]["instance_bill"].is_null() && listed[1]["bill_error"].is_null());
    assert_eq!(listed[1]["actual_costs_by_currency"], json!({}));
    assert_eq!(listed[1]["bill_stale"], true);
    Ok(())
}

/// Readings costs keeps for the account and its two resources: an account bill whose last
/// read failed, a traffic reading, a balance, and an instance bill per resource, the second
/// with a failed last read.
async fn fill_costs(pool: &PgPool, account: Uuid, resources: [Uuid; 2], now: i64) -> Result<()> {
    sqlx::query("INSERT INTO cloud_account_billing(account_id,billing_enabled,balance_enabled,bill,bill_error,traffic,traffic_error,traffic_history_identity,balance,balance_error,balance_next_at) VALUES($1,true,true,$2,'network_error',$3,'network_error','TEST_ONLY identity',$4,NULL,$5) ON CONFLICT(account_id) DO UPDATE SET bill=EXCLUDED.bill,bill_error=EXCLUDED.bill_error,traffic=EXCLUDED.traffic,traffic_error=EXCLUDED.traffic_error,balance=EXCLUDED.balance,balance_error=EXCLUDED.balance_error,balance_next_at=EXCLUDED.balance_next_at")
        .bind(account)
        .bind(json!({"month":"2026-11","queried_at":now,"rows":[],"usage_micro_gb":1}))
        .bind(json!({"queried_at":now,"mainland_bytes":"1","overseas_bytes":"2","regions":[]}))
        .bind(json!({"available":"1.00","currency":"CNY","queried_at":now}))
        .bind(now + 999)
        .execute(pool)
        .await?;
    for (id, error) in [
        (resources[0], None),
        (resources[1], Some("billing_incomplete")),
    ] {
        sqlx::query("INSERT INTO cloud_resource_billing(resource_id,instance_bill,bill_error,bill_next_at) VALUES($1,$2,$3,$4) ON CONFLICT(resource_id) DO UPDATE SET instance_bill=EXCLUDED.instance_bill,bill_error=EXCLUDED.bill_error,bill_next_at=EXCLUDED.bill_next_at")
            .bind(id)
            .bind(json!({"month":"2026-11","queried_at":now,"rows":[]}))
            .bind(error)
            .bind(now + 999)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// The resources' instance bills, `first` first.
async fn resource_bills(
    pool: &PgPool,
    first: Uuid,
) -> Result<Vec<(Uuid, Option<Value>, Option<String>, i64)>> {
    Ok(sqlx::query_as("SELECT resource_id,instance_bill,bill_error,bill_next_at FROM cloud_resource_billing ORDER BY resource_id=$1 DESC")
        .bind(first).fetch_all(pool).await?)
}

/// The account's readings costs keeps: the bill and its read error, the CDT traffic and
/// its error, the balance and its error.
type Readings = (
    Option<Value>,
    Option<String>,
    Option<Value>,
    Option<String>,
    Option<Value>,
    Option<String>,
);

async fn readings(pool: &PgPool, account: Uuid) -> Result<Readings> {
    Ok(sqlx::query_as("SELECT bill,bill_error,traffic,traffic_error,balance,balance_error FROM cloud_account_billing WHERE account_id=$1")
        .bind(account).fetch_one(pool).await?)
}

async fn history(pool: &PgPool, account: Uuid) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM alicloud_traffic_history WHERE account_id=$1")
            .bind(account)
            .fetch_one(pool)
            .await?,
    )
}

/// Costs follows what the account form does to an account: a manual refresh re-reads the
/// balance and instance bills whose last read succeeded, an edit discards every reading
/// taken under the old configuration (the CDT history only when the credential or site
/// changes), and an archive drops the bill, CDT reading and balance.
#[sqlx::test]
async fn account_refreshes_edits_and_archives_reach_what_costs_keeps(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let now = apsp_protocol::now_timestamp();
    let account = Uuid::new_v4();
    let (active, archived) = (Uuid::new_v4(), Uuid::new_v4());
    let reference = credential(&pool).await?;
    sqlx::query("INSERT INTO alicloud_accounts(id,name,credential_id,next_run_at) VALUES($1,'TEST_ONLY 账号',$2,$3)")
        .bind(account).bind(reference).bind(now + 999).execute(&pool).await?;
    for (id, cloud_id, archive) in [
        (active, "i-active-testonly", false),
        (archived, "i-archived-testonly", true),
    ] {
        sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id,archived) VALUES($1,$2,'TEST_ONLY 资源','ecs','cn-hangzhou',$3,$4)")
            .bind(id).bind(account).bind(cloud_id).bind(archive).execute(&pool).await?;
    }
    fill_costs(&pool, account, [active, archived], now).await?;
    sqlx::query("INSERT INTO alicloud_traffic_history VALUES($1,3600,$2,$3,1,2)")
        .bind(account)
        .bind(now.div_euclid(3600) * 3600)
        .bind(now)
        .execute(&pool)
        .await?;

    let path = format!("/api/resources/alicloud/accounts/{account}");
    assert_eq!(
        panel
            .admin(Method::POST, &format!("{path}/refresh"), &cookie, None)
            .await?
            .status(),
        StatusCode::ACCEPTED
    );
    let schedule: (i64, i64) = sqlx::query_as("SELECT a.next_run_at,b.balance_next_at FROM alicloud_accounts a JOIN cloud_account_billing b ON b.account_id=a.id WHERE a.id=$1")
        .bind(account).fetch_one(&pool).await?;
    assert_eq!(schedule, (0, 0));
    let bills = resource_bills(&pool, active).await?;
    assert_eq!((bills[0].0, bills[0].3), (active, 0));
    assert_eq!((bills[1].0, bills[1].3), (archived, now + 999));

    let mut body = json!({"name":"TEST_ONLY 改名","site":"china","enabled":true,"auto_enabled":false,"limit_gb":100,"revision":1});
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(body.clone()))
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let flags: (bool, bool, Option<String>, i64) = sqlx::query_as("SELECT billing_enabled,balance_enabled,traffic_history_identity,balance_next_at FROM cloud_account_billing WHERE account_id=$1")
        .bind(account).fetch_one(&pool).await?;
    assert_eq!(flags, (true, true, Some("TEST_ONLY identity".into()), 0));
    assert_eq!(
        readings(&pool, account).await?,
        (None, None, None, None, None, None)
    );
    for (_, bill, error, next) in resource_bills(&pool, active).await? {
        assert_eq!((bill, error, next), (None, None, 0));
    }
    assert_eq!(history(&pool, account).await?, 1);
    body["revision"] = 2.into();
    body["credential_id"] = credential(&pool).await?.to_string().into();
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(body))
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(history(&pool, account).await?, 0);

    fill_costs(&pool, account, [active, archived], now).await?;
    assert_eq!(
        panel
            .admin(
                Method::DELETE,
                &format!("/api/resources/alicloud/resources/{active}"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel
            .admin(Method::DELETE, &path, &cookie, None)
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        readings(&pool, account).await?,
        (
            None,
            Some("network_error".into()),
            None,
            Some("network_error".into()),
            None,
            None
        )
    );
    Ok(())
}
