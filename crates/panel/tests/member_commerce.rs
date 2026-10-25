#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_panel::auth::{hash_token, random_token};
use apsp_protocol::now_timestamp;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
const ROOT: &str = "/api/plugins/sing-box/commerce";

async fn admin(panel: &TestPanel) -> Result<String> {
    let cookie = panel.admin_cookie().await?;
    let hash = hash_token(cookie.split_once('=').context("cookie")?.1);
    sqlx::query("INSERT INTO administrator_reauth(session_hash,verified_at,expires_at) VALUES($1,$2,$3) ON CONFLICT(session_hash) DO UPDATE SET verified_at=EXCLUDED.verified_at,expires_at=EXCLUDED.expires_at").bind(hash).bind(now_timestamp()).bind(now_timestamp()+3600).execute(&panel.state.pool).await?;
    Ok(cookie)
}
async fn request(
    panel: &TestPanel,
    method: Method,
    path: &str,
    cookie: &str,
    body: Value,
) -> Result<reqwest::Response> {
    Ok(panel
        .client
        .request(method, format!("{}{path}", panel.base))
        .header("Cookie", cookie)
        .header("Origin", &panel.state.config.public_url)
        .json(&body)
        .send()
        .await?)
}
async fn ok(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let body = response.text().await?;
    ensure!(status.is_success(), "{status}: {body}");
    Ok(serde_json::from_str(&body)?)
}
async fn member(panel: &TestPanel, id: i64) -> Result<(Uuid, String)> {
    let account = Uuid::new_v4();
    let token = random_token();
    sqlx::query("INSERT INTO passkey_accounts(id) VALUES($1)")
        .bind(account)
        .execute(&panel.state.pool)
        .await?;
    sqlx::query("INSERT INTO singbox_portal_accounts(user_id,account_id) VALUES($1,$2)")
        .bind(id)
        .bind(account)
        .execute(&panel.state.pool)
        .await?;
    sqlx::query("INSERT INTO singbox_portal_sessions(token_hash,account_id,expires_at,verified_at) VALUES($1,$2,$3,$4)").bind(hash_token(&token)).bind(account).bind(now_timestamp()+3600).bind(now_timestamp()).execute(&panel.state.pool).await?;
    Ok((account, format!("apsp_proxy_session={token}")))
}
async fn balance(pool: &PgPool, id: i64) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT balance_cents FROM singbox_wallets WHERE user_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn wallet_purchase_and_invites_are_atomic_and_idempotent(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let admin = admin(&panel).await?;
    let user = panel.create_user(&admin, "Fixture member").await?;
    let id = user["id"].as_i64().context("id")?;
    let (account, cookie) = member(&panel, id).await?;
    let portal = format!("/api/plugins/sing-box/portal/{account}/commerce");
    assert_eq!(
        panel
            .client
            .get(format!("{}{ROOT}/members", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    ok(request(
        &panel,
        Method::PUT,
        &format!("{ROOT}/settings"),
        &admin,
        json!({"enabled":true,"invitations_enabled":true,"invite_price_cents":100,"version":1}),
    )
    .await?)
    .await?;
    let adjust = json!({"request_id":Uuid::new_v4(),"delta_cents":1500,"expected_balance_cents":0,"reason":"fixture opening"});
    for _ in 0..2 {
        ok(request(
            &panel,
            Method::POST,
            &format!("{ROOT}/members/{id}/adjust"),
            &admin,
            adjust.clone(),
        )
        .await?)
        .await?;
    }
    assert_eq!(balance(&pool, id).await?, 1500);
    let mut changed = adjust.clone();
    changed["delta_cents"] = json!(500);
    assert_eq!(
        request(
            &panel,
            Method::POST,
            &format!("{ROOT}/members/{id}/adjust"),
            &admin,
            changed
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let package:i64=sqlx::query_scalar("INSERT INTO singbox_package_groups(name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,duration_days) VALUES('Fixture',10000,1,0,0,'UTC',31) RETURNING id").fetch_one(&pool).await?;
    let offer=ok(request(&panel,Method::POST,&format!("{ROOT}/offers"),&admin,json!({"name":"Monthly","description":"fixture","package_group_id":package,"policy_group_ids":[],"price_cents":1000,"duration_months":1,"enabled":true,"renewable":true,"capacity":null,"version":null})).await?).await?;
    let buy = json!({"request_id":Uuid::new_v4(),"offer_id":offer["id"],"offer_version":1,"beneficiary_id":null});
    let other = json!({"request_id":Uuid::new_v4(),"offer_id":offer["id"],"offer_version":1,"beneficiary_id":null});
    let purchase_path = format!("{portal}/purchase");
    let (one, two) = tokio::join!(
        request(&panel, Method::POST, &purchase_path, &cookie, buy.clone()),
        request(&panel, Method::POST, &purchase_path, &cookie, other.clone())
    );
    let one = one?;
    let two = two?;
    let winner = if one.status().is_success() {
        assert_eq!(two.status(), StatusCode::CONFLICT);
        buy
    } else {
        assert_eq!(one.status(), StatusCode::CONFLICT);
        assert!(two.status().is_success());
        other
    };
    assert_eq!(balance(&pool, id).await?, 500);
    ok(request(&panel, Method::POST, &purchase_path, &cookie, winner).await?).await?;
    assert_eq!(balance(&pool, id).await?, 500);
    let invite=ok(request(&panel,Method::POST,&format!("{portal}/invitations"),&cookie,json!({"request_id":Uuid::new_v4(),"offer_id":null,"offer_version":null,"expected_admission_cents":100})).await?).await?;
    assert_eq!(balance(&pool, id).await?, 400);
    let token = invite["url"]
        .as_str()
        .context("invite url")?
        .rsplit('/')
        .next()
        .context("token")?;
    let claim = json!({"token":token,"request_id":Uuid::new_v4(),"name":"Invited","email":"invited@example.com"});
    let claimed = ok(request(
        &panel,
        Method::POST,
        "/api/plugins/sing-box/commerce-public/claim",
        "",
        claim.clone(),
    )
    .await?)
    .await?;
    ok(request(
        &panel,
        Method::POST,
        "/api/plugins/sing-box/commerce-public/claim",
        "",
        claim,
    )
    .await?)
    .await?;
    assert!(claimed["url"].is_string());
    let invited: i64 =
        sqlx::query_scalar("SELECT user_id FROM singbox_members WHERE email='invited@example.com'")
            .fetch_one(&pool)
            .await?;
    let allowed: bool =
        sqlx::query_scalar("SELECT allowed FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(now_timestamp())
            .bind(invited)
            .fetch_one(&pool)
            .await?;
    assert!(!allowed);
    assert_eq!(balance(&pool, id).await?, 400);
    let refund_path = format!(
        "{portal}/invitations/{}/revoke",
        invite["invitation"]["id"].as_str().context("id")?
    );
    assert_eq!(
        request(&panel, Method::POST, &refund_path, &cookie, json!({}))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    ok(request(&panel,Method::POST,&format!("{ROOT}/members/{id}/adjust"),&admin,json!({"request_id":Uuid::new_v4(),"delta_cents":1000,"expected_balance_cents":400,"reason":"fixture prepaid"})).await?).await?;
    let prepaid=ok(request(&panel,Method::POST,&format!("{portal}/invitations"),&cookie,json!({"request_id":Uuid::new_v4(),"offer_id":offer["id"],"offer_version":1,"expected_admission_cents":100})).await?).await?;
    assert_eq!(balance(&pool, id).await?, 300);
    let revoke = format!(
        "{portal}/invitations/{}/revoke",
        prepaid["invitation"]["id"].as_str().context("prepaid id")?
    );
    for _ in 0..2 {
        ok(request(&panel, Method::POST, &revoke, &cookie, json!({})).await?).await?;
    }
    assert_eq!(balance(&pool, id).await?, 1400);
    let batch = json!({"request_id":Uuid::new_v4(),"offer_id":null,"offer_version":null,"expected_admission_cents":100,"quantity":2});
    for _ in 0..2 {
        let result = ok(request(
            &panel,
            Method::POST,
            &format!("{portal}/invitations"),
            &cookie,
            batch.clone(),
        )
        .await?)
        .await?;
        assert_eq!(result["invitations"].as_array().unwrap().len(), 2);
    }
    assert_eq!(balance(&pool, id).await?, 1200);
    let bad_origin=panel.client.post(format!("{}{portal}/purchase",panel.base)).header("Cookie",cookie).header("Origin","https://attacker.example").json(&json!({"request_id":Uuid::new_v4(),"offer_id":offer["id"],"offer_version":1,"beneficiary_id":null})).send().await?;
    assert_eq!(bad_origin.status(), StatusCode::BAD_REQUEST);
    let rejected: Value = bad_origin.json().await?;
    assert_eq!(
        rejected["error"],
        "访问地址与面板配置不一致，请使用配置的公开地址打开页面。"
    );
    assert_eq!(balance(&pool, id).await?, 1200);
    Ok(())
}

fn snapshot() -> Value {
    json!({"format":"apsp.nexus.v1","source_id":"fixture","exported_at":now_timestamp(),"members":[{"id":41,"name":"Original","email":"original@example.com","balance_cents":2500,"state":"active","inviter_id":null,"sponsor_required":false,"first_purchase_at":null,"subscription_token":"fixture-original-token-000000000001","legacy_uuid":"00000000-0000-4000-8000-000000000041","plan_id":7,"quota_bytes":"1000000","used_upload":"123","used_download":"456","expires_at":now_timestamp()+86400,"reset_at":now_timestamp()+3600,"last_reset_at":now_timestamp()-86400,"remind_expire":false,"remind_traffic":false,"legacy_password_hash":"$2y$04$OuGqx53J01YMMaWjdHXc3OQiJLeNMgFD.WSQtOrUIHPIcruHwVKP."}],"plans":[{"id":7,"name":"Original plan","quota_bytes":"1000000","legacy_group_id":9,"prices":[{"label":"Monthly","cents":1000,"months":1,"days":31},{"label":"Annual","cents":10000,"months":12,"days":366}]}],"history":[{"kind":"wallet","id":"old:1","member_id":41,"record":{"delta":2500}}],"content":[]})
}

#[sqlx::test(migrations = "./migrations")]
async fn migration_preserves_identity_balance_login_and_legacy_meter(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let admin = admin(&panel).await?;
    let source = snapshot();
    let preview = ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/migration/preview"),
        &admin,
        source.clone(),
    )
    .await?)
    .await?;
    let input =
        json!({"snapshot":source,"expected_digest":preview["digest"],"confirm_staged":true});
    let imported = ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/migration/import"),
        &admin,
        input.clone(),
    )
    .await?)
    .await?;
    ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/migration/import"),
        &admin,
        input,
    )
    .await?)
    .await?;
    assert_eq!(balance(&pool, 41).await?, 2500);
    let packages: i64 = sqlx::query_scalar("SELECT count(*) FROM singbox_package_groups")
        .fetch_one(&pool)
        .await?;
    assert_eq!(packages, 1);
    let ledger: i64 = sqlx::query_scalar("SELECT count(*) FROM singbox_wallet_ledger")
        .fetch_one(&pool)
        .await?;
    assert_eq!(ledger, 1);
    ok(request(
        &panel,
        Method::PUT,
        &format!("{ROOT}/settings"),
        &admin,
        json!({"enabled":true,"invitations_enabled":false,"invite_price_cents":100,"version":1}),
    )
    .await?)
    .await?;
    let login_path = "/api/plugins/sing-box/commerce-public/account/login";
    let login = json!({"email":"original@example.com","password":"fixture-password-123"});
    assert_eq!(
        request(&panel, Method::POST, login_path, "", login.clone())
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    ok(request(&panel,Method::POST,&format!("{ROOT}/compatibility"),&admin,json!({"import_id":imported["id"],"confirmed_digest":preview["digest"],"subscription_base":"https://legacy.example.com/assets/","confirm_legacy_read_only":true})).await?).await?;
    assert_eq!(
        request(
            &panel,
            Method::POST,
            "/api/plugins/sing-box/users/41/subscription/reset",
            &admin,
            json!({})
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let preferences: (bool, bool) =
        sqlx::query_as("SELECT remind_expire,remind_traffic FROM singbox_members WHERE user_id=41")
            .fetch_one(&pool)
            .await?;
    assert_eq!(preferences, (false, false));
    let sample = json!({"import_id":imported["id"],"observed_at":now_timestamp(),"members":[{"id":41,"upload":"200","download":"500","last_reset_at":now_timestamp()-86400,"next_reset_at":now_timestamp()+3600}]});
    let desired: Value = sqlx::query_scalar("SELECT singbox_nexus_bridge_exchange($1)")
        .bind(&sample)
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        desired["members"][0]["uuid"],
        "00000000-0000-4000-8000-000000000041"
    );
    assert_eq!(
        desired["members"][0]["subscription_token"],
        source["members"][0]["subscription_token"]
    );
    assert_eq!(desired["members"][0]["balance"], 2500);
    assert_eq!(desired["members"][0]["group_id"], 9);
    let entitlement: Value =
        sqlx::query_scalar("SELECT to_jsonb(e) FROM singbox_entitlements($1) e WHERE user_id=41")
            .bind(now_timestamp())
            .fetch_one(&pool)
            .await?;
    assert_eq!(entitlement["used_bytes"], "700");
    assert_eq!(entitlement["allowed"], true);
    assert!(
        sqlx::query("SELECT singbox_nexus_bridge_exchange('{}'::jsonb)")
            .execute(&pool)
            .await
            .is_err()
    );
    let response = request(&panel, Method::POST, login_path, "", login).await?;
    if !response.status().is_success() {
        let _ = ok(response).await?;
        unreachable!();
    }
    let cookie = response
        .headers()
        .get("set-cookie")
        .context("member cookie")?
        .to_str()?
        .split(';')
        .next()
        .context("cookie")?
        .to_string();
    let logged = ok(response).await?;
    let account = logged["account_id"].as_str().context("account")?;
    let view = ok(panel
        .client
        .get(format!(
            "{}/api/plugins/sing-box/portal/{account}",
            panel.base
        ))
        .header("Cookie", &cookie)
        .send()
        .await?)
    .await?;
    assert_eq!(
        view["subscription_url"],
        "https://legacy.example.com/assets/fixture-original-token-000000000001"
    );
    assert_eq!(view["subscription_status"]["status"], "ready");
    assert_eq!(view["usage"]["uplink"], "200");
    let upgraded: bool = sqlx::query_scalar(
        "SELECT password_hash LIKE '$argon2%' FROM singbox_members WHERE user_id=41",
    )
    .fetch_one(&pool)
    .await?;
    assert!(upgraded);
    let details = ok(request(
        &panel,
        Method::GET,
        &format!("{ROOT}/members/41"),
        &admin,
        Value::Null,
    )
    .await?)
    .await?;
    assert!(details["profile"].get("password_hash").is_none());
    // A password session cannot remove existing passkeys; a passkey session can.
    let portal_account: Uuid = account.parse()?;
    let mut keys = Vec::new();
    for index in 1u8..=2 {
        let key = Uuid::new_v4();
        sqlx::query("INSERT INTO passkey_credentials(id,account_id,credential_id,payload,name,created_at) VALUES($1,$2,$3,'{}'::jsonb,'Fixture key',$4)")
            .bind(key).bind(portal_account).bind(vec![index]).bind(now_timestamp()).execute(&pool).await?;
        keys.push(key);
    }
    let remove = format!(
        "/api/plugins/sing-box/portal/{account}/keys/{}/remove",
        keys[0]
    );
    assert_eq!(
        request(&panel, Method::POST, &remove, &cookie, json!({}))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    sqlx::query("UPDATE singbox_portal_sessions SET proof='passkey' WHERE account_id=$1")
        .bind(portal_account)
        .execute(&pool)
        .await?;
    ok(request(&panel, Method::POST, &remove, &cookie, json!({})).await?).await?;
    // An administrator-changed address is unverified, so the password stops working there.
    ok(request(
        &panel,
        Method::PUT,
        &format!("{ROOT}/members/41"),
        &admin,
        json!({"name":"Original","email":"moved@example.com","state":details["profile"]["state"],
            "notes":"","version":details["profile"]["version"],"reason":"fixture address change"}),
    )
    .await?)
    .await?;
    for email in ["original@example.com", "moved@example.com"] {
        assert_eq!(
            request(
                &panel,
                Method::POST,
                login_path,
                "",
                json!({"email":email,"password":"fixture-password-123"})
            )
            .await?
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    // A legacy lifetime subscription stays permanent until an explicit native assignment.
    sqlx::query("UPDATE singbox_nexus_identities SET expires_at=NULL WHERE user_id=41")
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE singbox_package_assignments SET expires_at=253402300799 WHERE user_id=41")
        .execute(&pool)
        .await?;
    let lifetime: Value = sqlx::query_scalar("SELECT singbox_nexus_bridge_exchange($1)")
        .bind(&sample)
        .fetch_one(&pool)
        .await?;
    assert!(lifetime["members"][0]["expires_at"].is_null());
    assert_eq!(
        request(
            &panel,
            Method::POST,
            "/api/plugins/sing-box/users/41/package",
            &admin,
            json!({"package_group_id":999999,"request_id":Uuid::new_v4()})
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let package: i64 =
        sqlx::query_scalar("SELECT package_group_id FROM singbox_nexus_plan_mappings LIMIT 1")
            .fetch_one(&pool)
            .await?;
    let assignment = ok(request(
        &panel,
        Method::POST,
        "/api/plugins/sing-box/users/41/package",
        &admin,
        json!({"package_group_id":package,"request_id":Uuid::new_v4()}),
    )
    .await?)
    .await?;
    let reassigned: Value = sqlx::query_scalar("SELECT singbox_nexus_bridge_exchange($1)")
        .bind(&sample)
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        reassigned["members"][0]["expires_at"],
        assignment["expires_at"]
    );
    assert!(
        reassigned["members"][0]["expires_at"]
            .as_i64()
            .context("new expiry")?
            < 253402300799
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "Requires an isolated fixture credential keyring; run explicitly with test-only keys"]
async fn epay_duplicate_and_late_callbacks_do_not_double_credit(pool: PgPool) -> Result<()> {
    use md5::{Digest, Md5};
    use std::collections::BTreeMap;
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let admin = admin(&panel).await?;
    let user = panel.create_user(&admin, "Payment fixture").await?;
    let id = user["id"].as_i64().context("id")?;
    let (account, cookie) = member(&panel, id).await?;
    ok(request(
        &panel,
        Method::PUT,
        &format!("{ROOT}/settings"),
        &admin,
        json!({"enabled":true,"invitations_enabled":false,"invite_price_cents":100,"version":1}),
    )
    .await?)
    .await?;
    let credential = apsp_panel::control_center::credentials::store_generated(
        &pool,
        "Fixture payment",
        "external-api",
        json!({"key":"fixture-secret-only"}),
    )
    .await?;
    let channel=ok(request(&panel,Method::POST,&format!("{ROOT}/channels"),&admin,json!({"name":"Fixture","gateway":"https://pay.example.com/submit.php","merchant":"fixture","credential_id":credential,"payment_type":"alipay","fixed_fee_cents":0,"fee_basis_points":0,"enabled":true,"version":null})).await?).await?;
    let topup_path = format!("/api/plugins/sing-box/portal/{account}/commerce/topups");
    let topup=ok(request(&panel,Method::POST,&topup_path,&cookie,json!({"request_id":Uuid::new_v4(),"channel_id":channel["id"],"channel_version":1,"amount_cents":1234,"expected_fee_cents":0})).await?).await?;
    let signed = |topup: &Value, reference: &str| {
        let mut fields = BTreeMap::from([
            ("pid".to_string(), "fixture".to_string()),
            ("type".into(), "alipay".into()),
            (
                "out_trade_no".into(),
                topup["id"].as_str().unwrap().replace('-', ""),
            ),
            ("trade_no".into(), reference.into()),
            ("money".into(), "12.34".into()),
            ("trade_status".into(), "TRADE_SUCCESS".into()),
        ]);
        let message = fields
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&")
            + "fixture-secret-only";
        fields.insert("sign".into(), format!("{:x}", Md5::digest(message)));
        fields.insert("sign_type".into(), "MD5".into());
        fields
    };
    let endpoint = format!(
        "{}/api/plugins/sing-box/commerce-public/epay/{}",
        panel.base, channel["id"]
    );
    let first = signed(&topup, "receipt-1");
    for _ in 0..2 {
        let response = panel.client.get(&endpoint).query(&first).send().await?;
        ensure!(response.status().is_success(), "{}", response.text().await?);
    }
    assert_eq!(balance(&pool, id).await?, 1234);
    let late=ok(request(&panel,Method::POST,&topup_path,&cookie,json!({"request_id":Uuid::new_v4(),"channel_id":channel["id"],"channel_version":1,"amount_cents":1234,"expected_fee_cents":0})).await?).await?;
    ok(request(
        &panel,
        Method::POST,
        &format!("{topup_path}/{}/cancel", late["id"].as_str().unwrap()),
        &cookie,
        json!({}),
    )
    .await?)
    .await?;
    panel
        .client
        .post(&endpoint)
        .form(&signed(&late, "receipt-2"))
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(balance(&pool, id).await?, 1234);
    let exception: i64 =
        sqlx::query_scalar("SELECT id FROM singbox_payment_exceptions WHERE code='late_cancelled'")
            .fetch_one(&pool)
            .await?;
    for _ in 0..2 {
        ok(request(
            &panel,
            Method::POST,
            &format!("{ROOT}/exceptions/{exception}/resolve"),
            &admin,
            json!({"action":"credit","reason":"fixture confirmed receipt"}),
        )
        .await?)
        .await?;
    }
    assert_eq!(balance(&pool, id).await?, 2468);
    Ok(())
}
