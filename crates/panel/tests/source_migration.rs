#![forbid(unsafe_code)]

//! The manual source migration (ADR 0079 phase 3, step S1b): numbered sources
//! move into ordered sources without changing bundles, user subscriptions,
//! grants or the catalog, and a rollback restores the numbered state.

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_panel::plugins::singbox::{
    source_migration as migration, sources::refresh_due, subscription_sources::worker,
};
use apsp_protocol::now_timestamp;
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

const ROOT: &str = "/api/plugins/sing-box";

async fn request(
    panel: &TestPanel,
    cookie: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<(StatusCode, Value)> {
    let response = panel
        .admin(method, &format!("{ROOT}{path}"), cookie, body)
        .await?;
    let status = response.status();
    let text = response.text().await?;
    Ok((
        status,
        if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)?
        },
    ))
}

async fn ok(
    panel: &TestPanel,
    cookie: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    let (status, value) = request(panel, cookie, method, path, body).await?;
    ensure!(status.is_success(), "{path}: {status}: {value}");
    Ok(value)
}

fn content(password: &str) -> String {
    json!({"outbounds":[
        {"type":"shadowsocks","tag":"甲","server":"a.example.com","server_port":443,"method":"aes-128-gcm","password":password},
        {"type":"shadowsocks","tag":"乙","server":"b.example.com","server_port":443,"method":"aes-128-gcm","password":"TEST_ONLY-stable"}
    ]})
    .to_string()
}

async fn settled(panel: &TestPanel, cookie: &str, source: i64) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut refresh = tokio::time::interval(Duration::from_secs(1));
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let value = ok(
                panel,
                cookie,
                Method::GET,
                &format!("/subscription-sources/{source}"),
                None,
            )
            .await?;
            if value["active_job_id"].is_null() {
                ensure!(
                    value["last_error"].is_null(),
                    "source import failed: {value}"
                );
                return Ok(value);
            }
            tokio::select! {
                _ = refresh.tick() => refresh_due(&panel.state).await?,
                _ = tokio::time::sleep(Duration::from_millis(20)) => {},
            }
        }
    })
    .await
    .context("source job timeout")?
}

async fn host(panel: &TestPanel, cookie: &str, name: &str) -> Result<i64> {
    let host = panel.create_server(cookie, name).await?;
    panel.enable_plugin(cookie, host).await?;
    sqlx::query("UPDATE servers SET capabilities='[\"singbox\",\"runtime:dependency-validation:v1\"]',last_seen=$2 WHERE id=$1").bind(host).bind(now_timestamp()).execute(&panel.state.pool).await?;
    Ok(host)
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

fn named<'a>(entries: &'a Value, name: &str) -> Result<&'a Value> {
    entries
        .as_array()
        .context("entries")?
        .iter()
        .find(|entry| entry["name"] == name)
        .with_context(|| format!("entry {name}"))
}

async fn accesses(panel: &TestPanel, cookie: &str, user: i64) -> Result<Value> {
    ok(
        panel,
        cookie,
        Method::GET,
        &format!("/users/{user}/external-accesses"),
        None,
    )
    .await
}

async fn subscription(panel: &TestPanel, user: &Value) -> Result<String> {
    // The public address is a placeholder; fetch the same path from the test server.
    let url = user["subscription_url"].as_str().context("url")?;
    let path = url
        .strip_prefix("https://panel.example.com")
        .context("public subscription address")?;
    Ok(panel
        .client
        .get(format!("{}{path}?format=singbox", panel.base))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

async fn ordered_sources(pool: &PgPool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM singbox_ordered_subscription_sources")
            .fetch_one(pool)
            .await?,
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn migration_keeps_every_output_and_rollback_restores_it(pool: PgPool) -> Result<()> {
    // Mixed chains need a reachable panel address for their path checks.
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example.com")).await?;
    let cookie = panel.admin_cookie().await?;

    // Numbered source with two adopted nodes.
    let source = id(&ok(
        &panel,
        &cookie,
        Method::POST,
        "/subscription-sources",
        Some(json!({"name":"机场","kind":"inline","content":content("TEST_ONLY-first")})),
    )
    .await?)?;
    let settings = settled(&panel, &cookie, source).await?;
    let listed = ok(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{source}/nodes"),
        None,
    )
    .await?;
    for node in listed.as_array().context("nodes")? {
        if node["adopted"] == false {
            ok(&panel, &cookie, Method::PATCH, &format!("/subscription-sources/{source}/nodes/{}", node["id"]), Some(json!({"adopted":true,"settings_revision":settings["settings_revision"],"identity_epoch":node["identity_epoch"],"node_version_id":node["node_version_id"],"metadata_revision":node["metadata_revision"]}))).await?;
        }
    }
    let numbered_nodes: Vec<i64> = listed
        .as_array()
        .context("nodes")?
        .iter()
        .filter_map(|node| node["id"].as_i64())
        .collect();

    // One grant pinned to the first version, one following its node.
    let user = panel.create_user(&cookie, "迁移用户").await?;
    let uid = id(&user)?;
    let view = accesses(&panel, &cookie, uid).await?;
    let first = named(&view["available_nodes"], "甲")?;
    let second = named(&view["available_nodes"], "乙")?;
    ok(&panel, &cookie, Method::PUT, &format!("/users/{uid}/external-accesses"), Some(json!({"revision":view["revision"],"accesses":[reference(first,"pinned"),reference(second,"follow_node")]}))).await?;

    // A mixed chain whose first hop is a numbered-source node.
    let entry = host(&panel, &cookie, "入口").await?;
    let exit = host(&panel, &cookie, "出口").await?;
    let managed = id(&ok(&panel, &cookie, Method::POST, "/nodes", Some(json!({"name":"内部节点","server_id":exit,"public_host":"hop.example.com","sni":"www.example.com"}))).await?)?;
    let created = ok(&panel, &cookie, Method::POST, "/chains/batch", Some(json!({"request_id":Uuid::new_v4(),"items":[{"name":"混合链路","entry":{"mode":"new","server_id":entry,"public_host":"entry.example.com","sni":"www.example.com"},"hops":[{"kind":"subscription","source_id":source,"external_node_id":second["external_node_id"],"node_version_id":second["node_version_id"],"update_mode":"follow_node"},{"kind":"managed","node_id":managed}]}]}))).await?;
    let chain = created["chain_ids"][0].as_i64().context("chain")?;
    let policy = ok(
        &panel,
        &cookie,
        Method::POST,
        "/policy-groups",
        Some(json!({"name":"链路授权","node_ids":[],"chain_ids":[chain]})),
    )
    .await?;
    ok(
        &panel,
        &cookie,
        Method::PUT,
        &format!("/users/{uid}/policy-groups"),
        Some(json!({"group_ids":[id(&policy)?]})),
    )
    .await?;

    // Rotate the first node, so the pinned grant and the chain hop now refer
    // to versions that are no longer current.
    ok(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/subscription-sources/{source}"),
        Some(json!({"settings_revision":settings["settings_revision"],"content":content("TEST_ONLY-second")})),
    )
    .await?;
    settled(&panel, &cookie, source).await?;
    panel.publish_now().await?;
    let rendered = subscription(&panel, &user).await?;
    ensure!(rendered.contains("TEST_ONLY-first") && !rendered.contains("TEST_ONLY-second"));

    let state = ok(&panel, &cookie, Method::GET, "/source-migration", None).await?;
    ensure!(
        state == json!({"migrated":false,"migrated_at":null}),
        "{state}"
    );

    let at = now_timestamp();
    let before = migration::snapshot(&panel.state, at).await?;
    ensure!(before["bundles"][entry.to_string()].is_string());
    ensure!(
        before["subscriptions"]
            .as_object()
            .context("subscriptions")?
            .len()
            == 2
    );

    // The precheck changes nothing and finds no blocker.
    let report = migration::precheck(&pool).await?;
    ensure!(report.ready() && !report.migrated, "{:?}", report.blockers);
    ensure!(report.numbered_sources == 1 && report.numbered_nodes == 2);
    ensure!(
        report.versions_to_import == 4 && report.revisions_to_import == 2,
        "{} versions, {} revisions",
        report.versions_to_import,
        report.revisions_to_import
    );
    ensure!(report.external_accesses == 2 && report.mixed_chains_with_subscription_hops == 1);
    ensure!(
        report.versions_not_normalized == 0
            && report.identity_collisions == 0
            && report.inline_identity_changes == 0
            && report.inline_sources_not_reparsed == 0,
        "the ordered parser must reproduce the imported identities"
    );
    ensure!(report.warnings == ["mixed_chains_keep_numbered_versions"]);
    ensure!(ordered_sources(&pool).await? == 0);

    let report = migration::apply(&pool).await?;
    ensure!(report.migrated && report.ready());
    let state = ok(&panel, &cookie, Method::GET, "/source-migration", None).await?;
    ensure!(
        state["migrated"] == true && state["migrated_at"].is_i64(),
        "{state}"
    );
    let after = migration::snapshot(&panel.state, at).await?;
    let differences = migration::compare(&before, &after);
    ensure!(differences.is_empty(), "migration changed {differences:?}");
    ensure!(subscription(&panel, &user).await? == rendered);

    // Numbered sources are a read-only archive that points at their successor.
    let numbered = ok(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{source}"),
        None,
    )
    .await?;
    let ordered = numbered["migrated_to"].as_i64().context("migrated_to")?;
    for (method, path, body) in [
        (
            Method::PATCH,
            format!("/subscription-sources/{source}"),
            json!({"settings_revision":numbered["settings_revision"],"name":"改名"}),
        ),
        (
            Method::POST,
            "/subscription-sources".into(),
            json!({"name":"新来源","kind":"inline","content":content("TEST_ONLY-third")}),
        ),
    ] {
        let (status, _) = request(&panel, &cookie, method, &path, Some(body)).await?;
        ensure!(status == StatusCode::CONFLICT, "{path}: {status}");
    }
    ensure!(
        sqlx::query("UPDATE singbox_subscription_sources SET name='改名' WHERE id=$1")
            .bind(source)
            .execute(&pool)
            .await
            .is_err(),
        "the archive rejects direct writes too"
    );
    let (status, _) = request(&panel, &cookie, Method::POST, "/chains/batch", Some(json!({"request_id":Uuid::new_v4(),"items":[{"name":"新混合链路","entry":{"mode":"new","server_id":entry,"public_host":"entry2.example.com","sni":"www.example.com"},"hops":[{"kind":"subscription","source_id":source,"external_node_id":second["external_node_id"],"node_version_id":second["node_version_id"],"update_mode":"follow_node"},{"kind":"managed","node_id":managed}]}]}))).await?;
    ensure!(
        status == StatusCode::CONFLICT,
        "mixed subscription hop: {status}"
    );

    // The ordered source carries the same nodes under the same public ids.
    let page = ok(
        &panel,
        &cookie,
        Method::GET,
        &format!("/ordered-subscription-sources/{ordered}/nodes"),
        None,
    )
    .await?;
    let imported = page["nodes"].as_array().context("ordered nodes")?;
    ensure!(imported.len() == 2);
    for node in imported {
        ensure!(numbered_nodes.contains(&node["public_id"].as_i64().context("public id")?));
        ensure!(
            node["adopted"] == true && node["supported"] == true,
            "{node}"
        );
    }
    let report = migration::apply(&pool).await?;
    ensure!(report.blockers == ["already_migrated"]);

    // Imported nodes stay grantable; ordered-only nodes become grantable.
    let added = ok(&panel, &cookie, Method::POST, "/ordered-subscription-sources", Some(json!({"request_id":Uuid::new_v4(),"name":"有序来源","input":{"kind":"inline","content":json!({"outbounds":[{"type":"shadowsocks","tag":"丙","server":"c.example.com","server_port":443,"method":"aes-128-gcm","password":"TEST_ONLY-third"}]}).to_string()}}))).await?;
    let added = added["source_id"].as_i64().context("ordered source")?;
    worker::run_once(&panel.state)
        .await
        .map_err(|_| anyhow::anyhow!("source worker failed"))?;
    let page = ok(
        &panel,
        &cookie,
        Method::GET,
        &format!("/ordered-subscription-sources/{added}/nodes"),
        None,
    )
    .await?;
    let node = &page["nodes"][0];
    ok(&panel, &cookie, Method::PATCH, &format!("/ordered-subscription-sources/{added}/nodes/{}", node["id"].as_str().context("node")?), Some(json!({"adopted":true,"settings_revision":1,"identity_epoch":1,"node_version_id":node["version_id"],"metadata_revision":0}))).await?;
    let view = accesses(&panel, &cookie, uid).await?;
    for entry in view["available_nodes"].as_array().context("available")? {
        ensure!(entry["available"] == true, "{entry}");
    }
    let third = named(&view["available_nodes"], "丙")?;
    let kept: Vec<Value> = view["accesses"]
        .as_array()
        .context("accesses")?
        .iter()
        .map(|entry| reference(entry, entry["update_mode"].as_str().unwrap_or_default()))
        .collect();
    let mut widened = kept.clone();
    widened.push(reference(third, "follow_node"));
    ok(
        &panel,
        &cookie,
        Method::PUT,
        &format!("/users/{uid}/external-accesses"),
        Some(json!({"revision":view["revision"],"accesses":widened})),
    )
    .await?;
    ensure!(subscription(&panel, &user).await?.contains("c.example.com"));

    // A rollback must not strand that grant.
    let outcome = migration::rollback(&pool).await?;
    ensure!(!outcome.rolled_back && outcome.blockers.len() == 1);
    ensure!(outcome.blockers[0].starts_with("grants_on_nodes_or_versions_after_migration"));
    let view = accesses(&panel, &cookie, uid).await?;
    ok(
        &panel,
        &cookie,
        Method::PUT,
        &format!("/users/{uid}/external-accesses"),
        Some(json!({"revision":view["revision"],"accesses":kept})),
    )
    .await?;
    let outcome = migration::rollback(&pool).await?;
    ensure!(outcome.rolled_back, "{:?}", outcome.blockers);
    ensure!(outcome.restored_sources == 1 && outcome.restored_accesses == 2);
    ensure!(
        ordered_sources(&pool).await? == 1,
        "ordered-only sources stay"
    );
    let restored = migration::snapshot(&panel.state, at).await?;
    let differences = migration::compare(&before, &restored);
    ensure!(differences.is_empty(), "rollback left {differences:?}");

    // Numbered sources are writable again, and the migration can be retried.
    ok(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/subscription-sources/{source}"),
        Some(json!({"settings_revision":numbered["settings_revision"],"name":"改名"})),
    )
    .await?;
    ensure!(migration::precheck(&pool).await?.ready());
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Imported {
    input_config: Value,
    host: Option<String>,
    refresh_interval_secs: i64,
    next_refresh_at: Option<i64>,
    error_kind: Option<String>,
    user_agent: Option<String>,
    deleted_at: Option<i64>,
    created_at: i64,
}

#[sqlx::test(migrations = "./migrations")]
async fn source_settings_errors_and_deletions_carry_over(pool: PgPool) -> Result<()> {
    let url: i64 = sqlx::query_scalar("INSERT INTO singbox_subscription_sources(name,kind,secret_url,secret_authorization,source_host,refresh_interval_seconds,next_refresh_at,last_error,created_at,auto_refresh) VALUES('订阅','url','https://feed.example.com/sub','Bearer TEST_ONLY-token','feed.example.com',3600,100,'http_status',7,TRUE) RETURNING id")
        .fetch_one(&pool).await?;
    let deleted: i64 = sqlx::query_scalar("INSERT INTO singbox_subscription_sources(name,kind,secret_content,deleted_at,archived,created_at) VALUES('旧来源','inline','TEST_ONLY content',9,TRUE,8) RETURNING id")
        .fetch_one(&pool).await?;
    sqlx::query("INSERT INTO singbox_source_jobs(id,source_id,settings_revision,identity_epoch,parser_version,state,phase,created_at) VALUES($1,$2,1,1,'apsp-subscriptions-2','queued','queued',10)")
        .bind(Uuid::new_v4()).bind(url).execute(&pool).await?;
    let report = migration::precheck(&pool).await?;
    ensure!(report.ready() && report.active_numbered_jobs == 1);
    ensure!(report.numbered_sources == 2 && report.numbered_sources_deleted == 1);
    ensure!(migration::apply(&pool).await?.migrated);

    let state: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM singbox_source_jobs WHERE state IN ('queued','running')",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(state == 0, "numbered work stops");
    let row: Imported = sqlx::query_as("SELECT s.input_config,s.host,s.refresh_interval_secs,s.next_refresh_at,s.last_error->>'kind' AS error_kind,s.user_agent,s.deleted_at,s.created_at FROM singbox_ordered_subscription_sources s JOIN singbox_source_id_map m ON m.b_source_id=s.id WHERE m.a_source_id=$1")
        .bind(url).fetch_one(&pool).await?;
    ensure!(
        row.input_config
            == json!({"kind":"url","url":"https://feed.example.com/sub","auth_headers":{"authorization":"Bearer TEST_ONLY-token"}})
    );
    ensure!(
        row.host.as_deref() == Some("feed.example.com")
            && row.refresh_interval_secs == 3600
            && row.next_refresh_at == Some(100)
    );
    ensure!(row.error_kind.as_deref() == Some("http_status"));
    ensure!(
        row.user_agent.as_deref() == Some("Apsp-subscription-import/1")
            && row.deleted_at.is_none()
            && row.created_at == 7
    );
    let row: (Value, Option<String>, i64, Option<i64>, Option<i64>) = sqlx::query_as("SELECT s.input_config,s.host,s.refresh_interval_secs,s.next_refresh_at,s.deleted_at FROM singbox_ordered_subscription_sources s JOIN singbox_source_id_map m ON m.b_source_id=s.id WHERE m.a_source_id=$1")
        .bind(deleted).fetch_one(&pool).await?;
    ensure!(
        row == (json!({}), None, 0, None, Some(9)),
        "deleted sources keep no secret"
    );

    let outcome = migration::rollback(&pool).await?;
    ensure!(outcome.rolled_back && outcome.restored_sources == 2);
    ensure!(ordered_sources(&pool).await? == 0);
    let outcome = migration::rollback(&pool).await?;
    ensure!(outcome.blockers == ["not_migrated"]);
    Ok(())
}
