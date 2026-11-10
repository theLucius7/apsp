#![forbid(unsafe_code)]
//! Self-service accounts of proxy users (ADR 0113): quota-based invitation codes and
//! the sign-in that migrated members keep.
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_panel_host::auth::{hash_token, random_token};
use apsp_protocol::now_timestamp;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const ROOT: &str = "/api/plugins/sing-box";
// bcrypt of "fixture-password-123", as the original panel stored it.
const LEGACY_HASH: &str = "$2y$04$OuGqx53J01YMMaWjdHXc3OQiJLeNMgFD.WSQtOrUIHPIcruHwVKP.";

/// An administrator session that has just proven itself again.
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
/// A signed-in portal session for an existing proxy user.
async fn portal(panel: &TestPanel, id: i64) -> Result<(Uuid, String)> {
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
async fn diagnosis(panel: &TestPanel, cookie: &str, user: i64) -> Result<Value> {
    ok(request(
        panel,
        Method::GET,
        &format!("{ROOT}/users/{user}/diagnosis"),
        cookie,
        json!(null),
    )
    .await?)
    .await
}
async fn remaining(pool: &PgPool, id: i64) -> Result<i32> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT remaining FROM singbox_invitation_quotas WHERE user_id=$1),0)",
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

#[sqlx::test(migrations = "./migrations")]
async fn invitation_codes_spend_quota_and_create_accounts(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = admin(&panel).await?;
    let server = panel.create_server(&cookie, "Server").await?;
    let node = panel.create_node(&cookie, server, "Node").await?["id"]
        .as_i64()
        .context("node")?;
    let group = ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/policy-groups"),
        &cookie,
        json!({"name":"Friends","node_ids":[node],"chain_ids":[]}),
    )
    .await?)
    .await?["id"]
        .as_i64()
        .context("group")?;
    let package = ok(request(&panel, Method::POST, &format!("{ROOT}/package-groups"), &cookie, json!({"name":"Plan","monthly_bytes":null,"reset_day":1,"reset_hour":0,"reset_minute":0,"timezone":"UTC","duration_days":30})).await?).await?["id"].as_i64().context("package")?;
    let issuer = panel.create_user(&cookie, "Issuer").await?["id"]
        .as_i64()
        .context("issuer")?;
    let (account, session) = portal(&panel, issuer).await?;
    let create = format!("{ROOT}/portal/{account}/invitations");

    // Closed until the administrator opens invitations.
    let closed = request(
        &panel,
        Method::POST,
        &create,
        &session,
        json!({"request_id":Uuid::new_v4()}),
    )
    .await?;
    assert_eq!(closed.status(), StatusCode::FORBIDDEN);
    let settings = ok(request(
        &panel,
        Method::GET,
        &format!("{ROOT}/invitation-settings"),
        &cookie,
        json!(null),
    )
    .await?)
    .await?;
    ok(request(&panel, Method::PUT, &format!("{ROOT}/invitation-settings"), &cookie, json!({"enabled":true,"package_group_id":package,"policy_group_ids":[group],"default_quota":1,"version":settings["version"]})).await?).await?;
    // Out of quota until an administrator grants some.
    let empty = request(
        &panel,
        Method::POST,
        &create,
        &session,
        json!({"request_id":Uuid::new_v4()}),
    )
    .await?;
    assert_eq!(empty.status(), StatusCode::CONFLICT);
    ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/users/{issuer}/invitations/quota"),
        &cookie,
        json!({"delta":2,"reason":"friends"}),
    )
    .await?)
    .await?;
    assert_eq!(remaining(&pool, issuer).await?, 2);

    let request_id = Uuid::new_v4();
    let created = ok(request(
        &panel,
        Method::POST,
        &create,
        &session,
        json!({"request_id":request_id,"quantity":2}),
    )
    .await?)
    .await?;
    let codes: Vec<String> = created["codes"]
        .as_array()
        .context("codes")?
        .iter()
        .filter_map(|code| code.as_str().map(str::to_owned))
        .collect();
    assert_eq!(codes.len(), 2);
    assert!(
        codes
            .iter()
            .all(|code| code.len() == 19 && code.matches('-').count() == 3)
    );
    assert_eq!(remaining(&pool, issuer).await?, 0);
    // A retried request is not charged again and does not show the codes twice.
    let retried = ok(request(
        &panel,
        Method::POST,
        &create,
        &session,
        json!({"request_id":request_id,"quantity":2}),
    )
    .await?)
    .await?;
    assert_eq!(retried["codes"], json!([]));
    assert_eq!(remaining(&pool, issuer).await?, 0);

    // Revoking an unused code gives the invitation back.
    let view = ok(request(
        &panel,
        Method::GET,
        &format!("{ROOT}/portal/{account}/account"),
        &session,
        json!(null),
    )
    .await?)
    .await?;
    let issued = view["invitations"]["issued"].as_array().context("issued")?;
    assert_eq!(issued.len(), 2);
    assert!(
        view["invitations"]["issued"]
            .to_string()
            .find("token")
            .is_none()
    );
    let revoked = issued
        .iter()
        .find(|entry| codes[1].ends_with(entry["hint"].as_str().unwrap_or("-")))
        .context("second code")?["id"]
        .as_str()
        .context("id")?
        .to_owned();
    ok(request(
        &panel,
        Method::POST,
        &format!("{create}/{revoked}/revoke"),
        &session,
        json!(null),
    )
    .await?)
    .await?;
    assert_eq!(remaining(&pool, issuer).await?, 1);
    let inspected = ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/portal-public/invitation"),
        "",
        json!({"token":codes[1]}),
    )
    .await?)
    .await?;
    assert_eq!(inspected["available"], false);

    // The code is read leniently and creates an account with the configured start.
    let typed = codes[0].replace('-', "").to_lowercase();
    let inspected = ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/portal-public/invitation"),
        "",
        json!({"token":typed}),
    )
    .await?)
    .await?;
    assert_eq!(inspected["available"], true);
    assert_eq!(inspected["package_name"], "Plan");
    let claim = json!({"token":typed,"request_id":Uuid::new_v4(),"name":"Friend","email":"friend@example.com"});
    let claimed = ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/portal-public/claim"),
        "",
        claim.clone(),
    )
    .await?)
    .await?;
    assert!(
        claimed["url"]
            .as_str()
            .context("url")?
            .contains("?activate=")
    );
    let friend: i64 = sqlx::query_scalar("SELECT user_id FROM singbox_members WHERE email='friend@example.com' AND inviter_id=$1 AND state='active'")
        .bind(issuer).fetch_one(&pool).await?;
    let groups: Vec<i64> =
        sqlx::query_scalar("SELECT group_id FROM singbox_user_policies WHERE user_id=$1")
            .bind(friend)
            .fetch_all(&pool)
            .await?;
    assert_eq!(groups, vec![group]);
    let entitlement: Value = ok(request(
        &panel,
        Method::GET,
        &format!("{ROOT}/users/{friend}/entitlement"),
        &cookie,
        json!(null),
    )
    .await?)
    .await?;
    assert_eq!(entitlement["package_name"], "Plan");
    assert_eq!(entitlement["allowed"], true);
    assert_eq!(remaining(&pool, friend).await?, 1);
    // Repeating the same claim returns the same account; another one is refused.
    ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/portal-public/claim"),
        "",
        claim,
    )
    .await?)
    .await?;
    let again = request(&panel, Method::POST, &format!("{ROOT}/portal-public/claim"), "", json!({"token":codes[0],"request_id":Uuid::new_v4(),"name":"Other","email":"other@example.com"})).await?;
    assert_eq!(again.status(), StatusCode::CONFLICT);
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE deleted_at IS NULL")
        .fetch_one(&pool)
        .await?;
    assert_eq!(users, 2);

    let detail = ok(request(
        &panel,
        Method::GET,
        &format!("{ROOT}/users/{friend}/invitations"),
        &cookie,
        json!(null),
    )
    .await?)
    .await?;
    assert_eq!(detail["invited_by"]["id"], issuer);
    let history = ok(request(
        &panel,
        Method::GET,
        &format!("{ROOT}/users/{issuer}/invitations"),
        &cookie,
        json!(null),
    )
    .await?)
    .await?;
    assert_eq!(history["remaining"], 1);
    let reasons: Vec<&str> = history["events"]
        .as_array()
        .context("events")?
        .iter()
        .filter_map(|event| event["reason"].as_str())
        .collect();
    assert_eq!(
        reasons,
        ["invitation_revoked", "invitation_created", "administrator"]
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn migrated_member_signs_in_with_the_original_password(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = admin(&panel).await?;
    let user: i64 = sqlx::query_scalar("INSERT INTO users(name,subscription_token) VALUES('Original','fixture-original-token-000000000001') RETURNING id")
        .fetch_one(&pool).await?;
    sqlx::query("INSERT INTO singbox_members(user_id,email,legacy_login_email,password_hash,created_at) VALUES($1,'original@example.com','original@example.com',$2,$3)")
        .bind(user).bind(LEGACY_HASH).bind(now_timestamp()).execute(&pool).await?;
    sqlx::query("INSERT INTO singbox_wallets(user_id) VALUES($1)")
        .bind(user)
        .execute(&pool)
        .await?;
    let login_path = format!("{ROOT}/portal-public/account/login");
    let wrong = request(
        &panel,
        Method::POST,
        &login_path,
        "",
        json!({"email":"original@example.com","password":"wrong-password-0000"}),
    )
    .await?;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    let response = request(
        &panel,
        Method::POST,
        &login_path,
        "",
        json!({"email":"Original@Example.com","password":"fixture-password-123"}),
    )
    .await?;
    let session = response
        .headers()
        .get("set-cookie")
        .context("session cookie")?
        .to_str()?
        .split(';')
        .next()
        .context("cookie")?
        .to_string();
    let logged = ok(response).await?;
    let account = logged["account_id"].as_str().context("account")?;
    // The original hash is upgraded on first use.
    let upgraded: String =
        sqlx::query_scalar("SELECT password_hash FROM singbox_members WHERE user_id=$1")
            .bind(user)
            .fetch_one(&pool)
            .await?;
    assert!(upgraded.starts_with("$argon2"));
    let view = ok(request(
        &panel,
        Method::GET,
        &format!("{ROOT}/portal/{account}/account"),
        &session,
        json!(null),
    )
    .await?)
    .await?;
    assert_eq!(view["profile"]["email"], "original@example.com");
    assert_eq!(view["profile"]["has_password"], true);
    assert_eq!(view["invitations"]["enabled"], false);
    // A suspended member cannot sign in.
    sqlx::query("UPDATE singbox_members SET state='suspended' WHERE user_id=$1")
        .bind(user)
        .execute(&pool)
        .await?;
    let suspended = request(
        &panel,
        Method::POST,
        &login_path,
        "",
        json!({"email":"original@example.com","password":"fixture-password-123"}),
    )
    .await?;
    assert_eq!(suspended.status(), StatusCode::UNAUTHORIZED);
    let _ = cookie;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_profile_never_enrols_a_user_into_a_running_bridge(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = admin(&panel).await?;
    let user = panel.create_user(&cookie, "Native").await?["id"]
        .as_i64()
        .context("user")?;
    let (account, session) = portal(&panel, user).await?;
    let import = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_nexus_imports(id,source_id,digest,counts,imported_at,administrator_id) VALUES($1,'fixture','digest','{}',$2,1)")
        .bind(import).bind(now_timestamp()).execute(&pool).await?;
    sqlx::query("INSERT INTO singbox_nexus_bridge(import_id,subscription_base,enabled,activated_at) VALUES($1,'https://legacy.example.com/s/',TRUE,$2)")
        .bind(import).bind(now_timestamp()).execute(&pool).await?;
    let refused = request(
        &panel,
        Method::PUT,
        &format!("{ROOT}/portal/{account}/account/reminders"),
        &session,
        json!({"remind_expire":false,"remind_traffic":false}),
    )
    .await?;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM singbox_members")
        .fetch_one(&pool)
        .await?;
    assert_eq!(members, 0);
    sqlx::query("UPDATE singbox_nexus_bridge SET enabled=FALSE")
        .execute(&pool)
        .await?;
    ok(request(
        &panel,
        Method::PUT,
        &format!("{ROOT}/portal/{account}/account/reminders"),
        &session,
        json!({"remind_expire":false,"remind_traffic":true}),
    )
    .await?)
    .await?;
    let saved: (bool, bool) =
        sqlx::query_as("SELECT remind_expire,remind_traffic FROM singbox_members WHERE user_id=$1")
            .bind(user)
            .fetch_one(&pool)
            .await?;
    assert_eq!(saved, (false, true));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn diagnosis_explains_where_a_migrated_user_gets_their_lines(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = admin(&panel).await?;
    let server = panel.create_server(&cookie, "Legacy host").await?;
    let package = ok(request(&panel, Method::POST, &format!("{ROOT}/package-groups"), &cookie, json!({"name":"Monthly","monthly_bytes":null,"reset_day":1,"reset_hour":0,"reset_minute":0,"timezone":"UTC","duration_days":30})).await?).await?["id"].as_i64().context("package")?;
    let user = panel.create_user(&cookie, "Migrated").await?["id"]
        .as_i64()
        .context("user")?;
    let import = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_nexus_imports(id,source_id,digest,counts,imported_at,administrator_id) VALUES($1,'fixture','digest','{}',$2,1)")
        .bind(import).bind(now_timestamp()).execute(&pool).await?;
    sqlx::query("INSERT INTO singbox_nexus_plan_mappings(import_id,package_group_id,legacy_plan_id,legacy_group_id) VALUES($1,$2,1,9)")
        .bind(import).bind(package).execute(&pool).await?;
    sqlx::query("INSERT INTO singbox_nexus_bridge(import_id,subscription_base,enabled,activated_at) VALUES($1,'https://legacy.example.com/s/',TRUE,$2)")
        .bind(import).bind(now_timestamp()).execute(&pool).await?;
    sqlx::query("INSERT INTO singbox_nexus_identities(user_id,import_id,legacy_uuid,original_state,used_upload,used_download) VALUES($1,$2,$3,'active',0,0)")
        .bind(user).bind(import).bind(Uuid::new_v4()).execute(&pool).await?;
    // Two catalogued lines: one in the user's original permission group, one not.
    let digest = "0".repeat(64);
    sqlx::query("INSERT INTO singbox_nexus_node_imports(source_id,digest,infrastructure_digest,exported_at,imported_at,node_count) VALUES('nexus-fixture',$1,$1,$2,$2,2)")
        .bind(&digest).bind(now_timestamp()).execute(&pool).await?;
    for (legacy, group, name) in [(1_i64, 9_i64, "香港 01"), (2, 3, "日本 01")] {
        let credential = Uuid::new_v4();
        sqlx::query("INSERT INTO credential_entries(id,name,kind,key_id,nonce,ciphertext,created_at,updated_at) VALUES($1,'fixture','external-api','TEST_ONLY',$2,$3,$4,$4)")
            .bind(credential).bind(vec![0_u8; 12]).bind(vec![0_u8; 16]).bind(now_timestamp()).execute(&pool).await?;
        sqlx::query("INSERT INTO singbox_nexus_nodes(source_id,legacy_node_id,legacy_machine_id,legacy_host_id,server_id,credential_id,display) VALUES('nexus-fixture',$1,$1,1,$2,$3,$4)")
            .bind(legacy).bind(server).bind(credential)
            .bind(json!({"visible":true,"name":name,"sort_order":legacy,"groups":[{"id":group}]}))
            .execute(&pool).await?;
    }
    // Without a package the original system hands out no lines at all.
    let blocked = diagnosis(&panel, &cookie, user).await?;
    assert_eq!(blocked["subscription"]["status"], "blocked");
    assert!(
        blocked["subscription"]["message"]
            .as_str()
            .context("message")?
            .contains("尚未分配套餐")
    );
    ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/users/{user}/package"),
        &cookie,
        json!({"package_group_id":package,"request_id":Uuid::new_v4()}),
    )
    .await?)
    .await?;
    // A package alone no longer decides lines: the member needs the policy group
    // standing for the original group (ADR 0115).
    let outside = diagnosis(&panel, &cookie, user).await?;
    assert_eq!(outside["subscription"]["status"], "blocked");
    assert!(
        outside["subscription"]["message"]
            .as_str()
            .context("message")?
            .contains("不在任何原权限组对应的策略组中")
    );
    let (nine, three): (i64, i64) =
        sqlx::query_as("SELECT singbox_legacy_policy_group(9),singbox_legacy_policy_group(3)")
            .fetch_one(&pool)
            .await?;
    for (groups, expected) in [
        (vec![nine, three], Some(9_i64)),
        (vec![three], Some(3)),
        (vec![], None),
    ] {
        ok(request(
            &panel,
            Method::PUT,
            &format!("{ROOT}/users/{user}/policy-groups"),
            &cookie,
            json!({"group_ids":groups}),
        )
        .await?)
        .await?;
        let original: Option<i64> = sqlx::query_scalar("SELECT singbox_member_legacy_group($1)")
            .bind(user)
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            original, expected,
            "the package's original group wins, then the lowest"
        );
    }
    ok(request(
        &panel,
        Method::PUT,
        &format!("{ROOT}/users/{user}/policy-groups"),
        &cookie,
        json!({"group_ids":[nine]}),
    )
    .await?)
    .await?;
    let ready = diagnosis(&panel, &cookie, user).await?;
    assert_eq!(ready["subscription"]["status"], "legacy");
    let message = ready["subscription"]["message"]
        .as_str()
        .context("message")?;
    assert!(
        message.contains("原权限组 #9") && message.contains("可用 1 条线路"),
        "{message}"
    );
    assert_eq!(
        ready["legacy"]["lines"],
        json!([{"name":"香港 01","server_id":server,"taken_over":false,"served":true}])
    );
    assert_eq!(ready["permissions"], json!([]));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn administrators_delete_notices_and_guides(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = admin(&panel).await?;
    let saved = ok(request(
        &panel,
        Method::POST,
        &format!("{ROOT}/portal-guides"),
        &cookie,
        json!({"kind":"guide","title":"Outdated guide","category":"Help","body":"<p>Old steps</p>","published":true,"version":null}),
    )
    .await?)
    .await?;
    let guide = saved["id"].as_i64().context("guide")?;
    let member = panel.create_user(&cookie, "Member").await?["id"]
        .as_i64()
        .context("member")?;
    let (account, session) = portal(&panel, member).await?;
    let titles = |view: &Value| -> Vec<String> {
        view["guides"]
            .as_array()
            .map(|guides| {
                guides
                    .iter()
                    .filter_map(|guide| guide["title"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let view = format!("{ROOT}/portal/{account}/account");
    let before = ok(request(&panel, Method::GET, &view, &session, json!(null)).await?).await?;
    assert_eq!(titles(&before), ["Outdated guide"]);

    let path = format!("{ROOT}/portal-guides/{guide}");
    let removed = request(&panel, Method::DELETE, &path, &cookie, json!(null)).await?;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    let after = ok(request(&panel, Method::GET, &view, &session, json!(null)).await?).await?;
    assert!(
        titles(&after).is_empty(),
        "the portal stops showing it at once"
    );
    let listed = ok(request(
        &panel,
        Method::GET,
        &format!("{ROOT}/portal-guides"),
        &cookie,
        json!(null),
    )
    .await?)
    .await?;
    assert_eq!(listed["entries"], json!([]));
    let again = request(&panel, Method::DELETE, &path, &cookie, json!(null)).await?;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM singbox_commerce_events WHERE action='content_deleted' AND object_id=$1",
    )
    .bind(guide.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(audited, 1);
    Ok(())
}
