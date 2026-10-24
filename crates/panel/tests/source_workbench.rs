#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;

const ROOT: &str = "/api/plugins/sing-box";

// Source parsing admits a fixed number of concurrent parses per process and
// answers 409 when they are taken. Tests in this binary would otherwise
// compete for those permits and observe "busy" instead of their own outcome.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn call(
    panel: &TestPanel,
    cookie: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    let response = panel
        .admin(method, &format!("{ROOT}{path}"), cookie, body)
        .await?;
    let status = response.status();
    let text = response.text().await?;
    ensure!(status.is_success(), "{path}: {status}: {text}");
    Ok(if status == StatusCode::NO_CONTENT {
        Value::Null
    } else {
        serde_json::from_str(&text)?
    })
}

fn content() -> String {
    json!({"outbounds":[
        {"type":"shadowsocks","tag":"节点甲","server":"a.example.com","server_port":443,"method":"aes-128-gcm","password":"TEST_ONLY first private"},
        {"type":"http","tag":"节点乙","server":"b.example.com","server_port":8443,"username":"TEST_ONLY","password":"TEST_ONLY second private"},
        {"type":"unsupported-test","tag":"不支持项","server":"c.example.com","server_port":443,"password":"TEST_ONLY rejected private"}
    ]}).to_string()
}

async fn preview(panel: &TestPanel, cookie: &str) -> Result<Value> {
    call(
        panel,
        cookie,
        Method::POST,
        "/subscription-source-previews",
        Some(json!({"kind":"inline","content":content()})),
    )
    .await
}

async fn commit(
    panel: &TestPanel,
    cookie: &str,
    preview: &Value,
    selected: Value,
) -> Result<Value> {
    call(panel,cookie,Method::POST,&format!("/subscription-source-previews/{}/commit",preview["id"].as_str().context("preview id")?),Some(json!({"name":"已确认来源","selected":selected,"auto_refresh":false,"refresh_interval_seconds":3600}))).await
}

async fn settle(panel: &TestPanel, cookie: &str, source: i64) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let source = call(
                panel,
                cookie,
                Method::GET,
                &format!("/subscription-sources/{source}"),
                None,
            )
            .await?;
            if source["active_job_id"].is_null() {
                return Ok(source);
            }
            apsp_panel::plugins::singbox::sources::refresh_due(&panel.state).await?;
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .context("source refresh timeout")?
}

#[sqlx::test(migrations = "./migrations")]
async fn preview_is_private_then_commits_exactly_selected_nodes_once(pool: PgPool) -> Result<()> {
    let _serial = SERIAL.lock().await;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let result = preview(&panel, &cookie).await?;
    ensure!(result["supported_count"] == 2 && result["unsupported_count"] == 1);
    ensure!(!result.to_string().contains("private") && !result.to_string().contains("password"));
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM singbox_subscription_sources")
            .fetch_one(&pool)
            .await?
            == 0
    );
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM singbox_external_nodes")
            .fetch_one(&pool)
            .await?
            == 0
    );
    let source = commit(&panel, &cookie, &result, json!([result["nodes"][0]["key"]])).await?;
    ensure!(source["auto_refresh"] == false && source["active_job_id"].is_null());
    ensure!(source["changes"] == json!({"added":2,"updated":0,"missing":0,"unsupported":1}));
    let source_id = id(&source)?;
    let nodes = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{source_id}/nodes"),
        None,
    )
    .await?;
    ensure!(
        nodes
            .as_array()
            .context("nodes")?
            .iter()
            .filter(|n| n["adopted"] == true)
            .count()
            == 1
    );
    let saved: String =
        sqlx::query_scalar("SELECT secret_content FROM singbox_subscription_sources WHERE id=$1")
            .bind(source_id)
            .fetch_one(&pool)
            .await?;
    ensure!(saved == content());
    let config: Value=sqlx::query_scalar("SELECT v.config_json FROM singbox_external_nodes n JOIN singbox_external_node_versions v ON v.id=n.current_version_id WHERE n.source_id=$1 AND n.adopted").bind(source_id).fetch_one(&pool).await?;
    ensure!(config["password"] == "TEST_ONLY first private");
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM singbox_source_previews")
            .fetch_one(&pool)
            .await?
            == 0
    );
    let replay = panel
        .admin(
            Method::POST,
            &format!(
                "{ROOT}/subscription-source-previews/{}/commit",
                result["id"].as_str().unwrap()
            ),
            &cookie,
            Some(json!({"name":"重复","selected":["node-0"]})),
        )
        .await?;
    ensure!(replay.status() == StatusCode::CONFLICT);
    ensure!(
        !nodes.to_string().contains("private") && !source.to_string().contains("secret_content")
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn url_preview_commit_uses_captured_body_and_upstream_information_without_refetch(
    pool: PgPool,
) -> Result<()> {
    let _serial = SERIAL.lock().await;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let result = preview(&panel, &cookie).await?;
    // Seed the captured network response without connecting to a public host.
    sqlx::query("UPDATE singbox_source_previews SET kind='url',secret_url='https://source.example.com/TEST_ONLY-private-token',secret_authorization='Bearer TEST_ONLY-secret',traffic='{\"download\":25,\"total\":100,\"expire\":2000000000}' WHERE id=$1::text::uuid")
        .bind(result["id"].as_str().unwrap()).execute(&pool).await?;
    let source = commit(&panel, &cookie, &result, json!(["node-0"])).await?;
    ensure!(source["source_host"] == "source.example.com" && source["active_job_id"].is_null());
    ensure!(
        source["traffic"]["download"] == 25
            && source["traffic"]["total"] == 100
            && source["traffic"]["updated_at"].is_i64()
    );
    ensure!(source["traffic"]["upload"].is_null() && !source.to_string().contains("TEST_ONLY"));
    let stored: (Option<String>, String) = sqlx::query_as(
        "SELECT secret_content,secret_authorization FROM singbox_subscription_sources WHERE id=$1",
    )
    .bind(id(&source)?)
    .fetch_one(&pool)
    .await?;
    ensure!(stored.0.is_none() && stored.1 == "Bearer TEST_ONLY-secret");
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM singbox_source_jobs")
            .fetch_one(&pool)
            .await?
            == 0
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn preview_selection_expiry_parser_and_auth_fail_without_partial_sources(
    pool: PgPool,
) -> Result<()> {
    let _serial = SERIAL.lock().await;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let unauthorized = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/subscription-source-previews"),
            "",
            Some(json!({"kind":"inline","content":content()})),
        )
        .await?;
    ensure!(unauthorized.status() == StatusCode::UNAUTHORIZED);
    let result = preview(&panel, &cookie).await?;
    let preview_id = result["id"].as_str().context("id")?;
    for selected in [
        json!([]),
        json!(["node-0", "node-0"]),
        json!(["rejected-0"]),
        json!(["node-100"]),
    ] {
        let response = panel
            .admin(
                Method::POST,
                &format!("{ROOT}/subscription-source-previews/{preview_id}/commit"),
                &cookie,
                Some(json!({"name":"无效","selected":selected})),
            )
            .await?;
        ensure!(response.status() == StatusCode::BAD_REQUEST);
    }
    let unauthorized = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/subscription-source-previews/{preview_id}/commit"),
            "",
            Some(json!({"name":"无权","selected":["node-0"]})),
        )
        .await?;
    ensure!(unauthorized.status() == StatusCode::UNAUTHORIZED);
    sqlx::query("UPDATE singbox_source_previews SET parser_version='old-test-parser' WHERE id=$1::text::uuid").bind(preview_id).execute(&pool).await?;
    let response = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/subscription-source-previews/{preview_id}/commit"),
            &cookie,
            Some(json!({"name":"旧解析器","selected":["node-0"]})),
        )
        .await?;
    ensure!(response.status() == StatusCode::CONFLICT);
    let expiry = preview(&panel, &cookie).await?;
    let expiry_id = expiry["id"].as_str().unwrap();
    sqlx::query(
        "UPDATE singbox_source_previews SET created_at=0,expires_at=1 WHERE id=$1::text::uuid",
    )
    .bind(expiry_id)
    .execute(&pool)
    .await?;
    let response = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/subscription-source-previews/{expiry_id}/commit"),
            &cookie,
            Some(json!({"name":"过期","selected":["node-0"]})),
        )
        .await?;
    ensure!(response.status() == StatusCode::CONFLICT);
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM singbox_subscription_sources")
            .fetch_one(&pool)
            .await?
            == 0
    );
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/subscription-source-previews/{preview_id}"),
        None,
    )
    .await?;
    apsp_panel::plugins::singbox::sources::refresh_due(&panel.state).await?;
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM singbox_source_previews")
            .fetch_one(&pool)
            .await?
            == 0
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn preview_limits_and_fetch_validation_never_contact_private_hosts(
    pool: PgPool,
) -> Result<()> {
    let _serial = SERIAL.lock().await;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    for body in [
        json!({"kind":"url","url":"https://127.0.0.1/private"}),
        json!({"kind":"url","url":"https://localhost/private"}),
        json!({"kind":"inline","content":"x","authorization":"TEST_ONLY"}),
        json!({"kind":"inline","content":content(),"user_agent":"x\r\nCookie: secret"}),
        json!({"kind":"inline","content":content(),"user_agent":"x".repeat(257)}),
        json!({"kind":"inline","content":"x".repeat(2*1024*1024+1)}),
        json!({"kind":"inline","content":"<html>TEST_ONLY secret</html>"}),
    ] {
        let response = panel
            .admin(
                Method::POST,
                &format!("{ROOT}/subscription-source-previews"),
                &cookie,
                Some(body),
            )
            .await?;
        ensure!(response.status() == StatusCode::BAD_REQUEST);
        ensure!(!response.text().await?.contains("TEST_ONLY"));
    }
    for _ in 0..8 {
        preview(&panel, &cookie).await?;
    }
    let response = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/subscription-source-previews"),
            &cookie,
            Some(json!({"kind":"inline","content":content()})),
        )
        .await?;
    ensure!(response.status() == StatusCode::CONFLICT);
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM singbox_source_previews")
            .fetch_one(&pool)
            .await?
            == 8
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn adoption_is_explicit_and_refresh_preserves_metadata_and_reports_changes(
    pool: PgPool,
) -> Result<()> {
    let _serial = SERIAL.lock().await;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let result = preview(&panel, &cookie).await?;
    let source = commit(&panel, &cookie, &result, json!(["node-0"])).await?;
    let source_id = id(&source)?;
    let first = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{source_id}/nodes"),
        None,
    )
    .await?;
    let node = first
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["adopted"] == true)
        .context("adopted")?;
    let node_id = id(node)?;
    sqlx::query("INSERT INTO singbox_node_metadata(kind,id,name_override,tags,note) VALUES('external',$1,'本地别名','[\"自用\"]','本地备注')").bind(node_id).execute(&pool).await?;
    let next=json!({"outbounds":[{"type":"shadowsocks","tag":"远端改名","server":"a.example.com","server_port":443,"method":"aes-128-gcm","password":"TEST_ONLY rotated"},{"type":"http","tag":"新发现","server":"new.example.com","server_port":80}]}).to_string();
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/subscription-sources/{source_id}"),
        Some(json!({"settings_revision":1,"content":next})),
    )
    .await?;
    let refreshed = settle(&panel, &cookie, source_id).await?;
    ensure!(refreshed["changes"] == json!({"added":1,"updated":1,"missing":1,"unsupported":0}));
    let rows = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{source_id}/nodes"),
        None,
    )
    .await?;
    ensure!(
        rows.as_array()
            .unwrap()
            .iter()
            .filter(|node| node["adopted"] == true)
            .count()
            == 1
    );
    ensure!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|node| node["name"] == "新发现" && node["adopted"] == false)
    );
    let local: (String, Value, String) = sqlx::query_as(
        "SELECT name_override,tags,note FROM singbox_node_metadata WHERE kind='external' AND id=$1",
    )
    .bind(node_id)
    .fetch_one(&pool)
    .await?;
    ensure!(local == ("本地别名".into(), json!(["自用"]), "本地备注".into()));
    let current_node = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == node_id)
        .context("current node")?;
    let stale = panel.admin(Method::PATCH,&format!("{ROOT}/subscription-sources/{source_id}/nodes/{node_id}"),&cookie,
        Some(json!({"adopted":false,"settings_revision":source["settings_revision"],"identity_epoch":node["identity_epoch"],"node_version_id":node["node_version_id"],"metadata_revision":node["metadata_revision"]}))).await?;
    ensure!(stale.status() == StatusCode::CONFLICT);
    let stale_version = panel.admin(Method::PATCH,&format!("{ROOT}/subscription-sources/{source_id}/nodes/{node_id}"),&cookie,
        Some(json!({"adopted":false,"settings_revision":refreshed["settings_revision"],"identity_epoch":node["identity_epoch"],"node_version_id":node["node_version_id"],"metadata_revision":node["metadata_revision"]}))).await?;
    ensure!(stale_version.status() == StatusCode::CONFLICT);
    ensure!(
        sqlx::query_scalar::<_, bool>("SELECT adopted FROM singbox_external_nodes WHERE id=$1")
            .bind(node_id)
            .fetch_one(&pool)
            .await?
    );
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/subscription-sources/{source_id}/nodes/{node_id}"),
        Some(json!({"adopted":false,"settings_revision":refreshed["settings_revision"],"identity_epoch":current_node["identity_epoch"],"node_version_id":current_node["node_version_id"],"metadata_revision":current_node["metadata_revision"]})),
    )
    .await?;
    sqlx::query("UPDATE singbox_node_metadata SET deleted_at=1 WHERE kind='external' AND id=$1")
        .bind(node_id)
        .execute(&pool)
        .await?;
    let fresh_nodes = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{source_id}/nodes"),
        None,
    )
    .await?;
    let current_node = fresh_nodes
        .as_array()
        .context("refreshed adoption nodes")?
        .iter()
        .find(|node| node["id"] == node_id)
        .context("fresh adoption revision")?;
    let restored = call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/subscription-sources/{source_id}/nodes/{node_id}"),
        Some(json!({"adopted":true,"settings_revision":refreshed["settings_revision"],"identity_epoch":current_node["identity_epoch"],"node_version_id":current_node["node_version_id"],"metadata_revision":current_node["metadata_revision"]})),
    )
    .await?;
    ensure!(restored["adopted"] == true);
    let tombstone: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM singbox_node_metadata WHERE kind='external' AND id=$1",
    )
    .bind(node_id)
    .fetch_one(&pool)
    .await?;
    ensure!(tombstone.is_none());
    let missing = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["reason"] == "node_missing")
        .context("missing")?;
    let response = panel
        .admin(
            Method::PATCH,
            &format!(
                "{ROOT}/subscription-sources/{source_id}/nodes/{}",
                id(missing)?
            ),
            &cookie,
            Some(json!({"adopted":true,"settings_revision":refreshed["settings_revision"],"identity_epoch":missing["identity_epoch"],"node_version_id":missing["node_version_id"],"metadata_revision":missing["metadata_revision"]})),
        )
        .await?;
    ensure!(response.status() == StatusCode::CONFLICT);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn refresh_settings_keep_interval_and_failures_keep_successful_snapshot(
    pool: PgPool,
) -> Result<()> {
    let _serial = SERIAL.lock().await;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let source=call(&panel,&cookie,Method::POST,"/subscription-sources",Some(json!({"name":"旧API兼容","kind":"inline","content":content(),"auto_refresh":false,"refresh_interval_seconds":3600,"user_agent":"sing-box/1.14.2"}))).await?;
    let source_id = id(&source)?;
    let initial = settle(&panel, &cookie, source_id).await?;
    let nodes = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{source_id}/nodes"),
        None,
    )
    .await?;
    ensure!(
        nodes
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["adopted"] == false)
    );
    for value in [json!({"auto_refresh":true}), json!({"auto_refresh":false})] {
        let current = call(
            &panel,
            &cookie,
            Method::GET,
            &format!("/subscription-sources/{source_id}"),
            None,
        )
        .await?;
        let mut body = value;
        body["settings_revision"] = current["settings_revision"].clone();
        let updated = call(
            &panel,
            &cookie,
            Method::PATCH,
            &format!("/subscription-sources/{source_id}"),
            Some(body),
        )
        .await?;
        ensure!(
            updated["refresh_interval_seconds"] == 3600
                && updated["user_agent"] == "sing-box/1.14.2"
                && updated["stale"] == false
        );
    }
    let current = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{source_id}"),
        None,
    )
    .await?;
    for invalid in [
        json!({"refresh_interval_seconds":299}),
        json!({"user_agent":"bad\nheader"}),
    ] {
        let mut body = invalid;
        body["settings_revision"] = current["settings_revision"].clone();
        let response = panel
            .admin(
                Method::PATCH,
                &format!("{ROOT}/subscription-sources/{source_id}"),
                &cookie,
                Some(body),
            )
            .await?;
        ensure!(response.status() == StatusCode::BAD_REQUEST);
    }
    sqlx::query("UPDATE singbox_subscription_sources SET traffic='{\"upload\":10,\"total\":100,\"updated_at\":1}' WHERE id=$1").bind(source_id).execute(&pool).await?;
    call(&panel,&cookie,Method::PATCH,&format!("/subscription-sources/{source_id}"),Some(json!({"settings_revision":current["settings_revision"],"content":"invalid TEST_ONLY private"}))).await?;
    let failed = settle(&panel, &cookie, source_id).await?;
    ensure!(
        failed["stale"] == true
            && failed["current_revision_id"] == initial["current_revision_id"]
            && failed["last_success_at"] == initial["last_success_at"]
    );
    ensure!(failed["traffic"] == json!({"upload":10,"total":100,"updated_at":1}));
    ensure!(!failed.to_string().contains("TEST_ONLY"));
    sqlx::query("UPDATE singbox_subscription_sources SET kind='url',secret_content=NULL,secret_url='https://source.example.com/test',next_refresh_at=0 WHERE id=$1").bind(source_id).execute(&pool).await?;
    apsp_panel::plugins::singbox::sources::refresh_due(&panel.state).await?;
    ensure!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM singbox_source_jobs WHERE source_id=$1 AND state IN ('queued','running')").bind(source_id).fetch_one(&pool).await?==0);
    Ok(())
}

#[sqlx::test]
async fn source_revision_and_identity_limits_reject_without_replacing_cached_history(
    pool: PgPool,
) -> Result<()> {
    let _serial = SERIAL.lock().await;
    const MAX: i64 = 9_007_199_254_740_991;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let source = call(&panel,&cookie,Method::POST,"/subscription-sources",Some(json!({"name":"Bounded source","kind":"inline","content":content(),"auto_refresh":false}))).await?;
    let sid = id(&source)?;
    let initial = settle(&panel, &cookie, sid).await?;
    for (revision, epoch, body, expected) in [
        (
            1,
            1,
            json!({"settings_revision":MAX+1,"name":"Unsafe revision"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            MAX,
            1,
            json!({"settings_revision":MAX,"name":"Counter overflow"}),
            StatusCode::CONFLICT,
        ),
        (
            1,
            MAX,
            json!({"settings_revision":1,"replace_source":true,"content":content()}),
            StatusCode::CONFLICT,
        ),
    ] {
        sqlx::query("UPDATE singbox_subscription_sources SET settings_revision=$2,identity_epoch=$3 WHERE id=$1").bind(sid).bind(revision).bind(epoch).execute(&pool).await?;
        let before: Value = sqlx::query_scalar(
            "SELECT to_jsonb(s) FROM singbox_subscription_sources s WHERE id=$1",
        )
        .bind(sid)
        .fetch_one(&pool)
        .await?;
        let jobs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM singbox_source_jobs WHERE source_id=$1")
                .bind(sid)
                .fetch_one(&pool)
                .await?;
        let response = panel
            .admin(
                Method::PATCH,
                &format!("{ROOT}/subscription-sources/{sid}"),
                &cookie,
                Some(body),
            )
            .await?;
        assert_eq!(response.status(), expected);
        let after: Value = sqlx::query_scalar(
            "SELECT to_jsonb(s) FROM singbox_subscription_sources s WHERE id=$1",
        )
        .bind(sid)
        .fetch_one(&pool)
        .await?;
        assert_eq!(after, before);
        assert_eq!(after["current_revision_id"], initial["current_revision_id"]);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM singbox_source_jobs WHERE source_id=$1"
            )
            .bind(sid)
            .fetch_one(&pool)
            .await?,
            jobs
        );
    }
    sqlx::query(
        "UPDATE singbox_subscription_sources SET settings_revision=1,identity_epoch=1 WHERE id=$1",
    )
    .bind(sid)
    .execute(&pool)
    .await?;
    let (node,version): (i64,i64) = sqlx::query_as("SELECT id,current_version_id FROM singbox_external_nodes WHERE source_id=$1 AND current_version_id IS NOT NULL ORDER BY id LIMIT 1").bind(sid).fetch_one(&pool).await?;
    sqlx::query(
        "INSERT INTO singbox_node_metadata(kind,id,revision,deleted_at) VALUES('external',$1,$2,1)",
    )
    .bind(node)
    .bind(MAX)
    .execute(&pool)
    .await?;
    let before: Value = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM singbox_node_metadata m WHERE kind='external' AND id=$1",
    )
    .bind(node)
    .fetch_one(&pool)
    .await?;
    let denied = panel.admin(Method::PATCH,&format!("{ROOT}/subscription-sources/{sid}/nodes/{node}"),&cookie,Some(json!({"adopted":true,"settings_revision":1,"identity_epoch":1,"node_version_id":version,"metadata_revision":MAX}))).await?;
    assert_eq!(denied.status(), StatusCode::CONFLICT);
    assert_eq!(
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(m) FROM singbox_node_metadata m WHERE kind='external' AND id=$1"
        )
        .bind(node)
        .fetch_one(&pool)
        .await?,
        before
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT adopted FROM singbox_external_nodes WHERE id=$1")
            .bind(node)
            .fetch_one(&pool)
            .await?
    );
    Ok(())
}

#[sqlx::test]
async fn adoption_metadata_cas_prevents_stale_catalog_resurrection_and_preserves_explicit_zero(
    pool: PgPool,
) -> Result<()> {
    let _serial = SERIAL.lock().await;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let parsed = preview(&panel, &cookie).await?;
    let source = commit(&panel, &cookie, &parsed, json!(["node-0"])).await?;
    let sid = id(&source)?;
    let nodes = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{sid}/nodes"),
        None,
    )
    .await?;
    let adopted = nodes
        .as_array()
        .context("source nodes")?
        .iter()
        .find(|node| node["adopted"] == true)
        .context("adopted node")?;
    let node = id(adopted)?;
    assert_eq!(adopted["metadata_revision"], 0);
    let stale = json!({"adopted":true,"settings_revision":source["settings_revision"],"identity_epoch":adopted["identity_epoch"],"node_version_id":adopted["node_version_id"],"metadata_revision":adopted["metadata_revision"]});
    let missing = json!({"adopted":true,"settings_revision":source["settings_revision"],"identity_epoch":adopted["identity_epoch"],"node_version_id":adopted["node_version_id"]});
    let path = format!("{ROOT}/subscription-sources/{sid}/nodes/{node}");
    assert_eq!(
        panel
            .client
            .patch(format!("{}{path}", panel.base))
            .json(&missing)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(missing))
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let catalog = call(&panel, &cookie, Method::GET, "/node-catalog", None).await?;
    let row = catalog
        .as_array()
        .context("catalog")?
        .iter()
        .find(|row| row["kind"] == "external" && row["id"] == node)
        .context("adopted catalog")?;
    call(
        &panel,
        &cookie,
        Method::DELETE,
        "/node-catalog/batch",
        Some(json!({"items":[{"kind":"external","id":node,"revision":row["revision"]}]})),
    )
    .await?;
    let before: Value = sqlx::query_scalar("SELECT jsonb_build_object('node',to_jsonb(n),'metadata',to_jsonb(m),'versions',(SELECT jsonb_agg(to_jsonb(v) ORDER BY id) FROM singbox_external_node_versions v WHERE v.external_node_id=n.id)) FROM singbox_external_nodes n JOIN singbox_node_metadata m ON m.kind='external' AND m.id=n.id WHERE n.id=$1").bind(node).fetch_one(&pool).await?;
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(stale.clone()))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(sqlx::query_scalar::<_,Value>("SELECT jsonb_build_object('node',to_jsonb(n),'metadata',to_jsonb(m),'versions',(SELECT jsonb_agg(to_jsonb(v) ORDER BY id) FROM singbox_external_node_versions v WHERE v.external_node_id=n.id)) FROM singbox_external_nodes n JOIN singbox_node_metadata m ON m.kind='external' AND m.id=n.id WHERE n.id=$1").bind(node).fetch_one(&pool).await?, before);
    let current = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{sid}/nodes"),
        None,
    )
    .await?;
    let fresh = current
        .as_array()
        .context("fresh source nodes")?
        .iter()
        .find(|row| row["id"] == node)
        .context("current removed node")?;
    assert_eq!(fresh["metadata_revision"], 1);
    let mut confirmed = stale;
    confirmed["metadata_revision"] = fresh["metadata_revision"].clone();
    let restored = call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/subscription-sources/{sid}/nodes/{node}"),
        Some(confirmed),
    )
    .await?;
    assert_eq!(restored["metadata_revision"], 2);
    assert_eq!(restored["adopted"], true);
    let after: Value = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(v) ORDER BY id) FROM singbox_external_node_versions v WHERE v.external_node_id=$1").bind(node).fetch_one(&pool).await?;
    assert_eq!(after, before["versions"]);
    let new = current
        .as_array()
        .context("current nodes")?
        .iter()
        .find(|row| row["id"].is_i64() && row["adopted"] == false && row["metadata_revision"] == 0)
        .context("unadopted explicit zero")?;
    let adopted_zero = call(&panel,&cookie,Method::PATCH,&format!("/subscription-sources/{sid}/nodes/{}",id(new)?),Some(json!({"adopted":true,"settings_revision":source["settings_revision"],"identity_epoch":new["identity_epoch"],"node_version_id":new["node_version_id"],"metadata_revision":0}))).await?;
    assert_eq!(adopted_zero["adopted"], true);
    assert_eq!(adopted_zero["metadata_revision"], 1);
    Ok(())
}
