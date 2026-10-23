#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_panel::plugins::singbox::sources::parse::{PARSER_VERSION, digest};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

const ROOT: &str = "/api/plugins/sing-box";
const FIRST: &str = "TEST_ONLY_external_provider_first";
const SECOND: &str = "TEST_ONLY_external_provider_second";

async fn source(pool: &PgPool) -> Result<(i64, i64)> {
    let source:i64=sqlx::query_scalar("INSERT INTO singbox_subscription_sources(name,kind,secret_content,created_at) VALUES('来源','inline','TEST_ONLY content',0) RETURNING id").fetch_one(pool).await?;
    let node:i64=sqlx::query_scalar("INSERT INTO singbox_external_nodes(source_id,identity_epoch,identity_key,name,adopted) VALUES($1,1,'stable','外部',TRUE) RETURNING id").bind(source).fetch_one(pool).await?;
    version(pool, source, node, FIRST).await?;
    Ok((source, node))
}

async fn version(pool: &PgPool, source: i64, node: i64, password: &str) -> Result<i64> {
    let config = json!({"type":"shadowsocks","server":"provider.example.com","server_port":443,"method":"aes-128-gcm","password":password});
    let hash = digest(&serde_json::to_vec(&config)?);
    let revision:i64=sqlx::query_scalar("INSERT INTO singbox_source_revisions(source_id,settings_revision,identity_epoch,parser_version,body_sha256,format,supported_count,unsupported_count,fetched_at) VALUES($1,1,1,$2,$3,'singbox_json',1,0,1) RETURNING id").bind(source).bind(PARSER_VERSION).bind(&hash).fetch_one(pool).await?;
    let version:i64=sqlx::query_scalar("INSERT INTO singbox_external_node_versions(external_node_id,source_id,source_revision_id,identity_epoch,parser_version,name,config_json,config_sha256,capabilities_json,created_at) VALUES($1,$2,$3,1,$4,'外部',$5,$6,'{}',1) RETURNING id").bind(node).bind(source).bind(revision).bind(PARSER_VERSION).bind(config).bind(hash).fetch_one(pool).await?;
    sqlx::query("UPDATE singbox_external_nodes SET current_version_id=$2,last_seen_revision_id=$3 WHERE id=$1").bind(node).bind(version).bind(revision).execute(pool).await?;
    sqlx::query("UPDATE singbox_subscription_sources SET current_revision_id=$2,last_success_at=1 WHERE id=$1").bind(source).bind(revision).execute(pool).await?;
    Ok(version)
}

async fn get(panel: &TestPanel, cookie: &str, user: i64) -> Result<Value> {
    let response = panel
        .admin(
            Method::GET,
            &format!("{ROOT}/users/{user}/external-accesses"),
            cookie,
            None,
        )
        .await?
        .error_for_status()?;
    ensure!(response.headers()["cache-control"] == "no-store");
    let value: Value = response.json().await?;
    let text = value.to_string();
    ensure!(
        !text.contains(FIRST)
            && !text.contains(SECOND)
            && !text.contains("config_json")
            && !text.contains("secret_content")
    );
    Ok(value)
}

fn reference(entry: &Value, mode: &str) -> Value {
    let mut result = json!({"update_mode":mode});
    for key in [
        "external_node_id",
        "source_id",
        "identity_epoch",
        "node_version_id",
        "metadata_revision",
    ] {
        result[key] = entry[key].clone();
    }
    result
}

async fn put(panel: &TestPanel, cookie: &str, user: i64, body: Value) -> Result<reqwest::Response> {
    panel
        .admin(
            Method::PUT,
            &format!("{ROOT}/users/{user}/external-accesses"),
            cookie,
            Some(body),
        )
        .await
}

async fn assign(panel: &TestPanel, cookie: &str, user: i64, mode: &str) -> Result<Value> {
    let current = get(panel, cookie, user).await?;
    let binding = reference(&current["available_nodes"][0], mode);
    Ok(put(
        panel,
        cookie,
        user,
        json!({"revision":current["revision"],"accesses":[binding]}),
    )
    .await?
    .error_for_status()?
    .json()
    .await?)
}

async fn content(panel: &TestPanel, user: &Value) -> Result<String> {
    Ok(panel
        .client
        .get(format!(
            "{}?format=singbox",
            user["subscription_url"].as_str().context("url")?
        ))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

#[sqlx::test]
async fn assignment_is_atomic_redacted_revision_checked_and_revocable(pool: PgPool) -> Result<()> {
    let (source, node) = source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = panel.create_user(&cookie, "外部用户").await?;
    let uid = id(&user)?;
    ensure!(
        panel
            .client
            .get(format!(
                "{}{ROOT}/users/{uid}/external-accesses",
                panel.base
            ))
            .send()
            .await?
            .status()
            == StatusCode::UNAUTHORIZED
    );
    let current = get(&panel, &cookie, uid).await?;
    ensure!(current["revision"] == 0 && current["accesses"] == json!([]));
    let assigned = assign(&panel, &cookie, uid, "follow_node").await?;
    ensure!(assigned["revision"] == 1);
    ensure!(
        panel
            .admin(
                Method::PATCH,
                &format!("{ROOT}/subscription-sources/{source}/nodes/{node}"),
                &cookie,
                Some(json!({"adopted":false,"settings_revision":1,"identity_epoch":1,"node_version_id":assigned["accesses"][0]["current_version_id"],"metadata_revision":0}))
            )
            .await?
            .status()
            == StatusCode::CONFLICT
    );
    ensure!(
        panel
            .admin(
                Method::DELETE,
                &format!("{ROOT}/subscription-sources/{source}"),
                &cookie,
                None
            )
            .await?
            .status()
            == StatusCode::CONFLICT
    );
    ensure!(content(&panel, &user).await?.contains(FIRST));
    ensure!(
        panel
            .client
            .get(user["subscription_url"].as_str().unwrap())
            .send()
            .await?
            .status()
            == StatusCode::CONFLICT
    );
    let saved = reference(&assigned["accesses"][0], "follow_node");
    ensure!(
        put(&panel, &cookie, uid, json!({"revision":0,"accesses":[]}))
            .await?
            .status()
            == StatusCode::CONFLICT
    );
    ensure!(
        put(
            &panel,
            &cookie,
            uid,
            json!({"revision":1,"accesses":[saved.clone(),saved.clone()]})
        )
        .await?
        .status()
            == StatusCode::BAD_REQUEST
    );
    let mut forged = saved.clone();
    forged["source_id"] = json!(999999);
    ensure!(
        put(
            &panel,
            &cookie,
            uid,
            json!({"revision":1,"accesses":[forged]})
        )
        .await?
        .status()
            == StatusCode::CONFLICT
    );
    ensure!(get(&panel, &cookie, uid).await?["revision"] == 1);
    let other = panel.create_user(&cookie, "未授权用户").await?;
    ensure!(
        panel
            .client
            .get(format!(
                "{}?format=singbox",
                other["subscription_url"].as_str().unwrap()
            ))
            .send()
            .await?
            .status()
            == StatusCode::CONFLICT
    );
    let preview: Value = panel
        .admin(
            Method::GET,
            &format!("{ROOT}/users/{uid}/subscription"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    ensure!(preview["external_nodes"] == 1 && preview["managed_nodes"] == 0);
    ensure!(
        preview["ready_nodes"][0]["kind"] == "external"
            && preview["available_formats"] == json!(["singbox"])
    );
    let cleared: Value = put(&panel, &cookie, uid, json!({"revision":1,"accesses":[]}))
        .await?
        .error_for_status()?
        .json()
        .await?;
    ensure!(cleared["revision"] == 2 && cleared["accesses"] == json!([]));
    panel
        .admin(
            Method::PATCH,
            &format!("{ROOT}/subscription-sources/{source}/nodes/{node}"),
            &cookie,
            Some(json!({"adopted":false,"settings_revision":1,"identity_epoch":1,"node_version_id":assigned["accesses"][0]["current_version_id"],"metadata_revision":0})),
        )
        .await?
        .error_for_status()?;
    let adoption_revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM singbox_node_metadata WHERE kind='external' AND id=$1",
    )
    .bind(node)
    .fetch_one(&pool)
    .await?;
    panel
        .admin(
            Method::PATCH,
            &format!("{ROOT}/subscription-sources/{source}/nodes/{node}"),
            &cookie,
            Some(json!({"adopted":true,"settings_revision":1,"identity_epoch":1,"node_version_id":assigned["accesses"][0]["current_version_id"],"metadata_revision":adoption_revision})),
        )
        .await?
        .error_for_status()?;
    ensure!(
        panel
            .client
            .get(format!(
                "{}?format=singbox",
                user["subscription_url"].as_str().unwrap()
            ))
            .send()
            .await?
            .status()
            == StatusCode::CONFLICT
    );
    assign(&panel, &cookie, uid, "pinned").await?;
    sqlx::query("UPDATE users SET deleted_at=1 WHERE id=$1")
        .bind(uid)
        .execute(&pool)
        .await?;
    ensure!(
        panel
            .client
            .get(format!(
                "{}?format=singbox",
                user["subscription_url"].as_str().unwrap()
            ))
            .send()
            .await?
            .status()
            == StatusCode::NOT_FOUND
    );
    Ok(())
}

#[sqlx::test]
async fn follow_and_pinned_versions_obey_identity_availability_and_failed_refresh(
    pool: PgPool,
) -> Result<()> {
    let (source, node) = source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let follow = panel.create_user(&cookie, "跟随").await?;
    let pinned = panel.create_user(&cookie, "固定").await?;
    assign(&panel, &cookie, id(&follow)?, "follow_node").await?;
    assign(&panel, &cookie, id(&pinned)?, "pinned").await?;
    version(&pool, source, node, SECOND).await?;
    ensure!(content(&panel, &follow).await?.contains(SECOND));
    ensure!(content(&panel, &pinned).await?.contains(FIRST));
    sqlx::query(
        "UPDATE singbox_subscription_sources SET last_error='fixture_fetch_failed' WHERE id=$1",
    )
    .bind(source)
    .execute(&pool)
    .await?;
    ensure!(content(&panel, &follow).await?.contains(SECOND));
    ensure!(
        get(&panel, &cookie, id(&follow)?).await?["accesses"][0]["source_last_error"]
            == "fixture_fetch_failed"
    );
    let transitions = [
        (
            "UPDATE singbox_external_nodes SET present=FALSE WHERE id=$1",
            "UPDATE singbox_external_nodes SET present=TRUE WHERE id=$1",
            node,
        ),
        (
            "UPDATE singbox_external_nodes SET adopted=FALSE WHERE id=$1",
            "UPDATE singbox_external_nodes SET adopted=TRUE WHERE id=$1",
            node,
        ),
        (
            "UPDATE singbox_external_nodes SET identity_unique=FALSE WHERE id=$1",
            "UPDATE singbox_external_nodes SET identity_unique=TRUE WHERE id=$1",
            node,
        ),
        (
            "UPDATE singbox_subscription_sources SET archived=TRUE WHERE id=$1",
            "UPDATE singbox_subscription_sources SET archived=FALSE WHERE id=$1",
            source,
        ),
        (
            "UPDATE singbox_subscription_sources SET identity_epoch=2 WHERE id=$1",
            "UPDATE singbox_subscription_sources SET identity_epoch=1 WHERE id=$1",
            source,
        ),
    ];
    for (disable, restore, target) in transitions {
        sqlx::query(disable).bind(target).execute(&pool).await?;
        for user in [&follow, &pinned] {
            ensure!(
                panel
                    .client
                    .get(format!(
                        "{}?format=singbox",
                        user["subscription_url"].as_str().unwrap()
                    ))
                    .send()
                    .await?
                    .status()
                    == StatusCode::CONFLICT
            );
            ensure!(get(&panel, &cookie, id(user)?).await?["accesses"][0]["available"] == false);
        }
        sqlx::query(restore).bind(target).execute(&pool).await?;
    }
    sqlx::query("INSERT INTO singbox_node_metadata(kind,id,enabled) VALUES('external',$1,FALSE)")
        .bind(node)
        .execute(&pool)
        .await?;
    ensure!(
        panel
            .client
            .get(format!(
                "{}?format=singbox",
                follow["subscription_url"].as_str().unwrap()
            ))
            .send()
            .await?
            .status()
            == StatusCode::CONFLICT
    );
    let current = get(&panel, &cookie, id(&follow)?).await?;
    let same = reference(&current["accesses"][0], "follow_node");
    put(
        &panel,
        &cookie,
        id(&follow)?,
        json!({"revision":current["revision"],"accesses":[same]}),
    )
    .await?
    .error_for_status()?;
    ensure!(get(&panel, &cookie, id(&follow)?).await?["accesses"][0]["available"] == false);
    Ok(())
}

#[sqlx::test]
async fn catalog_alias_and_order_reach_authorized_external_subscriptions(
    pool: PgPool,
) -> Result<()> {
    let (_, first) = source(&pool).await?;
    let (_, second) = source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = panel.create_user(&cookie, "目录订阅").await?;
    let uid = id(&user)?;
    let current = get(&panel, &cookie, uid).await?;
    let references: Vec<_> = current["available_nodes"]
        .as_array()
        .context("available nodes")?
        .iter()
        .map(|entry| reference(entry, "follow_node"))
        .collect();
    put(
        &panel,
        &cookie,
        uid,
        json!({"revision":0,"accesses":references}),
    )
    .await?
    .error_for_status()?;
    let before: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(
        before["outbounds"][0]["outbounds"]
            == json!([
                format!("external-node-{first} 外部"),
                format!("external-node-{second} 外部")
            ])
    );
    let catalog: Vec<Value> = panel
        .admin(Method::GET, &format!("{ROOT}/node-catalog"), &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    let alias = "香港 \"线路\" \\ A";
    let items: Vec<_> = catalog
        .iter()
        .map(|entry| {
            json!({
                "kind":"external", "id":entry["id"], "revision":entry["revision"],
                "name":alias, "sort_order":if entry["id"] == first { 20 } else { 10 }
            })
        })
        .collect();
    panel
        .admin(
            Method::PATCH,
            &format!("{ROOT}/node-catalog/batch"),
            &cookie,
            Some(json!({"items":items})),
        )
        .await?
        .error_for_status()?;
    let updated: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(
        updated["outbounds"][0]["outbounds"]
            == json!([
                format!("external-node-{second} {alias}"),
                format!("external-node-{first} {alias}")
            ])
    );
    ensure!(updated["outbounds"][1]["tag"] == format!("external-node-{second} {alias}"));
    ensure!(updated["outbounds"][1]["password"] == FIRST);
    ensure!(get(&panel, &cookie, uid).await?["accesses"][0]["name"] == alias);
    ensure!(content(&panel, &user).await? == serde_json::to_string_pretty(&updated)? + "\n");
    Ok(())
}

#[sqlx::test]
async fn mixed_subscription_keeps_managed_applied_health_and_entitlement_gates(
    pool: PgPool,
) -> Result<()> {
    source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "受管").await?;
    let managed = panel.create_node(&cookie, server, "受管入口").await?;
    let user = panel.create_user(&cookie, "混合订阅").await?;
    let uid = id(&user)?;
    panel.grant(&cookie, uid, id(&managed)?).await?;
    assign(&panel, &cookie, uid, "follow_node").await?;
    let before: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(
        before["outbounds"][0]["outbounds"]
            .as_array()
            .unwrap()
            .len()
            == 1
    );
    panel.publish_now().await?;
    sqlx::query("UPDATE server_module_status SET applied_rev=target_rev,healthy=TRUE")
        .execute(&pool)
        .await?;
    let ready: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(ready["outbounds"][0]["outbounds"] == json!(["node-1", "external-node-1 外部"]));
    sqlx::query("UPDATE server_module_status SET healthy=FALSE")
        .execute(&pool)
        .await?;
    let failed: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(
        failed["outbounds"][0]["outbounds"]
            .as_array()
            .unwrap()
            .len()
            == 1
    );
    let package:Value=panel.admin(Method::POST,&format!("{ROOT}/package-groups"),&cookie,Some(json!({"name":"过期套餐","monthly_bytes":"1024","reset_day":1,"reset_hour":0,"reset_minute":0,"timezone":"UTC","duration_days":1}))).await?.error_for_status()?.json().await?;
    panel
        .admin(
            Method::POST,
            &format!("{ROOT}/users/{uid}/package"),
            &cookie,
            Some(json!({"package_group_id":id(&package)?,"request_id":uuid::Uuid::new_v4()})),
        )
        .await?
        .error_for_status()?;
    sqlx::query("UPDATE singbox_package_assignments SET starts_at=$2-172800,expires_at=$2-86400 WHERE user_id=$1").bind(uid).bind(apsp_protocol::now_timestamp()).execute(&pool).await?;
    ensure!(
        panel
            .client
            .get(format!(
                "{}?format=singbox",
                user["subscription_url"].as_str().unwrap()
            ))
            .send()
            .await?
            .status()
            == StatusCode::CONFLICT
    );
    let blocked: Value = panel
        .admin(
            Method::GET,
            &format!("{ROOT}/users/{uid}/subscription"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    ensure!(
        blocked["status"] == "blocked"
            && blocked["content"].is_null()
            && blocked["ready_nodes"] == json!([])
    );
    Ok(())
}

#[sqlx::test]
async fn inactive_pinned_identity_is_retained_but_never_reissued_and_revisions_remain_exact(
    pool: PgPool,
) -> Result<()> {
    const MAX: i64 = 9_007_199_254_740_991;
    let (source, node) = source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = panel.create_user(&cookie, "Pinned history").await?;
    let uid = id(&user)?;
    let assigned = assign(&panel, &cookie, uid, "pinned").await?;
    let first = reference(&assigned["accesses"][0], "pinned");
    let original: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM singbox_external_accesses a WHERE user_id=$1")
            .bind(uid)
            .fetch_one(&pool)
            .await?;
    version(&pool, source, node, SECOND).await?;
    sqlx::query("UPDATE singbox_subscription_sources SET last_error='TEST_ONLY cached fetch failure' WHERE id=$1")
        .bind(source).execute(&pool).await?;
    let other = panel.create_user(&cookie, "New current grant").await?;
    let current_grant = assign(&panel, &cookie, id(&other)?, "follow_node").await?;
    assert_eq!(
        current_grant["revision"], 1,
        "the first observed revision zero is valid"
    );
    assert!(content(&panel, &other).await?.contains(SECOND));
    assert!(content(&panel, &user).await?.contains(FIRST));
    sqlx::query("INSERT INTO singbox_node_metadata(kind,id,enabled) VALUES('external',$1,FALSE)")
        .bind(node)
        .execute(&pool)
        .await?;
    let current = get(&panel, &cookie, uid).await?;
    let retained_reference = reference(&current["accesses"][0], "pinned");
    assert_eq!(
        retained_reference["node_version_id"],
        first["node_version_id"]
    );
    let kept: Value = put(
        &panel,
        &cookie,
        uid,
        json!({"revision":current["revision"],"accesses":[retained_reference.clone()]}),
    )
    .await?
    .error_for_status()?
    .json()
    .await?;
    assert_eq!(kept["accesses"][0]["available"], false);
    let retained: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM singbox_external_accesses a WHERE user_id=$1")
            .bind(uid)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        retained, original,
        "retaining an inactive identity preserves its creation receipt"
    );
    let fresh = panel.create_user(&cookie, "No inactive issuance").await?;
    assert_eq!(
        put(
            &panel,
            &cookie,
            id(&fresh)?,
            json!({"revision":0,"accesses":[retained_reference.clone()]})
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let mut changed_mode = retained_reference.clone();
    changed_mode["update_mode"] = json!("follow_node");
    assert_eq!(
        put(
            &panel,
            &cookie,
            uid,
            json!({"revision":kept["revision"],"accesses":[changed_mode]})
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        panel
            .client
            .get(format!(
                "{}?format=singbox",
                user["subscription_url"].as_str().context("url")?
            ))
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        put(
            &panel,
            &cookie,
            uid,
            json!({"revision":MAX+1,"accesses":[]})
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    let mut unsafe_identity = retained_reference;
    unsafe_identity["identity_epoch"] = json!(MAX + 1);
    assert_eq!(
        put(
            &panel,
            &cookie,
            uid,
            json!({"revision":kept["revision"],"accesses":[unsafe_identity]})
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    sqlx::query("UPDATE singbox_external_access_state SET revision=$2 WHERE user_id=$1")
        .bind(uid)
        .bind(MAX)
        .execute(&pool)
        .await?;
    assert_eq!(
        put(&panel, &cookie, uid, json!({"revision":MAX,"accesses":[]}))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(get(&panel, &cookie, uid).await?["revision"], MAX);
    let retained: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM singbox_external_accesses a WHERE user_id=$1")
            .bind(uid)
            .fetch_one(&pool)
            .await?;
    assert_eq!(retained, original);
    Ok(())
}
