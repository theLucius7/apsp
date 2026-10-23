#![forbid(unsafe_code)]

//! Ordered sources gain the settings, preview and adoption of numbered sources
//! (ADR 0079 phase 3, step S1a).

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_panel::plugins::singbox::subscription_sources::worker;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::{PgPool, migrate::Migrator};
use std::borrow::Cow;
use uuid::Uuid;

const ROOT: &str = "/api/plugins/sing-box";

async fn call(
    panel: &TestPanel,
    cookie: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
    expected: StatusCode,
) -> Result<Value> {
    let response = panel
        .admin(method, &format!("{ROOT}{path}"), cookie, body)
        .await?;
    ensure!(
        response.status() == expected,
        "unexpected status for {path}: {}",
        response.status()
    );
    if expected == StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    Ok(response.json().await?)
}

fn proxy(name: &str, host: &str) -> Value {
    json!({"type":"socks","tag":name,"server":host,"server_port":1080,"version":"5","username":"TEST_ONLY-user","password":"TEST_ONLY-password"})
}

fn config(nodes: Vec<Value>) -> String {
    json!({"outbounds":nodes}).to_string()
}

async fn source(panel: &TestPanel, cookie: &str, id: i64) -> Result<Value> {
    call(
        panel,
        cookie,
        Method::GET,
        &format!("/ordered-subscription-sources/{id}"),
        None,
        StatusCode::OK,
    )
    .await
}

async fn nodes(panel: &TestPanel, cookie: &str, id: i64) -> Result<Vec<Value>> {
    let page = call(
        panel,
        cookie,
        Method::GET,
        &format!("/ordered-subscription-sources/{id}/nodes"),
        None,
        StatusCode::OK,
    )
    .await?;
    Ok(page["nodes"].as_array().context("nodes")?.clone())
}

fn named<'a>(nodes: &'a [Value], name: &str) -> Result<&'a Value> {
    nodes
        .iter()
        .find(|node| node["name"] == name)
        .with_context(|| format!("node {name}"))
}

async fn run_worker(panel: &TestPanel) -> Result<()> {
    worker::run_once(&panel.state)
        .await
        .map_err(|_| anyhow::anyhow!("source worker failed"))
}

#[sqlx::test(migrations = "./migrations")]
async fn ported_settings_round_trip_and_control_automatic_refresh(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let url = |interval: i64| json!({"request_id":Uuid::new_v4(),"name":"Provider","input":{"kind":"url","url":"https://source.example.com/subscription"},"refresh_interval_secs":interval,"user_agent":"TEST_ONLY-agent/1","auto_refresh":false});
    for invalid in [299, 2_592_001] {
        call(
            &panel,
            &cookie,
            Method::POST,
            "/ordered-subscription-sources",
            Some(url(invalid)),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let receipt = call(
        &panel,
        &cookie,
        Method::POST,
        "/ordered-subscription-sources",
        Some(url(300)),
        StatusCode::ACCEPTED,
    )
    .await?;
    let id = receipt["source_id"].as_i64().context("source")?;
    let view = source(&panel, &cookie, id).await?;
    assert_eq!(view["user_agent"], "TEST_ONLY-agent/1");
    assert_eq!(view["auto_refresh"], false);
    assert_eq!(view["refresh_interval_secs"], 300);
    // The initial job still runs, but nothing is scheduled after it.
    let next: Option<i64> = sqlx::query_scalar(
        "SELECT next_refresh_at FROM singbox_ordered_subscription_sources WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;
    assert!(next.is_none());
    call(&panel, &cookie, Method::POST, "/ordered-subscription-sources", Some(json!({"request_id":Uuid::new_v4(),"name":"Header injection","input":{"kind":"url","url":"https://source.example.com/other"},"user_agent":"agent\r\nAuthorization: secret"})), StatusCode::BAD_REQUEST).await?;
    let patch = |revision: i64, body: Value| {
        let mut body = body;
        body["request_id"] = json!(Uuid::new_v4());
        body["settings_revision"] = json!(revision);
        body
    };
    let changed = call(&panel, &cookie, Method::PATCH, &format!("/ordered-subscription-sources/{id}"), Some(patch(1, json!({"user_agent":{"action":"clear"},"auto_refresh":true,"refresh_interval_secs":2_592_000}))), StatusCode::OK).await?;
    assert_eq!(changed["settings_revision"], 2);
    assert_eq!(
        changed["identity_epoch"], 1,
        "a User-Agent is not the source identity"
    );
    let view = source(&panel, &cookie, id).await?;
    assert!(view["user_agent"].is_null());
    assert_eq!(view["auto_refresh"], true);
    assert_eq!(view["refresh_interval_secs"], 2_592_000);
    let next: Option<i64> = sqlx::query_scalar(
        "SELECT next_refresh_at FROM singbox_ordered_subscription_sources WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;
    assert!(next.is_some());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn change_summary_counts_added_updated_missing_and_unsupported(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let first = config(vec![
        proxy("Stays", "a.example.com"),
        proxy("Renamed", "b.example.com"),
        proxy("Leaves", "c.example.com"),
    ]);
    let receipt = call(&panel, &cookie, Method::POST, "/ordered-subscription-sources", Some(json!({"request_id":Uuid::new_v4(),"name":"Inline","input":{"kind":"inline","content":first}})), StatusCode::ACCEPTED).await?;
    let id = receipt["source_id"].as_i64().context("source")?;
    run_worker(&panel).await?;
    let view = source(&panel, &cookie, id).await?;
    assert_eq!(
        view["changes"],
        json!({"added":3,"updated":0,"missing":0,"unsupported":0})
    );
    let second = config(vec![
        proxy("Stays", "a.example.com"),
        proxy("New name", "b.example.com"),
        proxy("Arrives", "d.example.com"),
        json!({"type":"wireguard","tag":"Unsupported","server":"e.example.com","server_port":51820}),
    ]);
    call(&panel, &cookie, Method::PATCH, &format!("/ordered-subscription-sources/{id}"), Some(json!({"request_id":Uuid::new_v4(),"settings_revision":view["settings_revision"],"input":{"kind":"inline","content":second,"identity_action":"update"}})), StatusCode::OK).await?;
    run_worker(&panel).await?;
    let view = source(&panel, &cookie, id).await?;
    assert_eq!(
        view["changes"],
        json!({"added":1,"updated":1,"missing":1,"unsupported":1})
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn preview_commit_adopts_only_selected_nodes_and_replays_its_receipt(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let content = config(vec![
        proxy("Chosen", "a.example.com"),
        proxy("Skipped", "b.example.com"),
        json!({"type":"wireguard","tag":"Unsupported","server":"c.example.com","server_port":51820}),
    ]);
    let preview_body = json!({"input":{"kind":"inline","content":content}});
    let preview = call(
        &panel,
        &cookie,
        Method::POST,
        "/ordered-subscription-source-previews",
        Some(preview_body.clone()),
        StatusCode::CREATED,
    )
    .await?;
    assert!(!preview.to_string().contains("TEST_ONLY-password"));
    let preview_id = preview["id"].as_str().context("preview")?;
    let listed = preview["nodes"].as_array().context("preview nodes")?;
    assert_eq!(listed.len(), 3);
    assert_eq!(named(listed, "Chosen")?["selectable"], true);
    assert_eq!(named(listed, "Unsupported")?["selectable"], false);
    let unsupported = named(listed, "Unsupported")?["key"].clone();
    let commit = |selected: Value, request_id: Uuid| json!({"request_id":request_id,"name":"Previewed","selected":selected,"refresh_interval_secs":0});
    call(
        &panel,
        &cookie,
        Method::POST,
        &format!("/ordered-subscription-source-previews/{preview_id}/commit"),
        Some(commit(json!([unsupported]), Uuid::new_v4())),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let request_id = Uuid::new_v4();
    let chosen = named(listed, "Chosen")?["key"].clone();
    let receipt = call(
        &panel,
        &cookie,
        Method::POST,
        &format!("/ordered-subscription-source-previews/{preview_id}/commit"),
        Some(commit(json!([chosen.clone()]), request_id)),
        StatusCode::CREATED,
    )
    .await?;
    let replay = call(
        &panel,
        &cookie,
        Method::POST,
        &format!("/ordered-subscription-source-previews/{preview_id}/commit"),
        Some(commit(json!([chosen.clone()]), request_id)),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(receipt, replay);
    call(
        &panel,
        &cookie,
        Method::POST,
        &format!("/ordered-subscription-source-previews/{preview_id}/commit"),
        Some(commit(json!([chosen]), Uuid::new_v4())),
        StatusCode::CONFLICT,
    )
    .await?;
    let id = receipt["source_id"].as_i64().context("source")?;
    let view = source(&panel, &cookie, id).await?;
    assert_eq!(view["counts"]["supported"], 2);
    assert_eq!(view["changes"]["added"], 2);
    let job = call(
        &panel,
        &cookie,
        Method::GET,
        &format!(
            "/ordered-subscription-source-jobs/{}",
            receipt["job_id"].as_str().context("job")?
        ),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(job["status"], "succeeded");
    assert_eq!(job["source_revision_id"], view["latest_success"]["id"]);
    let listed = nodes(&panel, &cookie, id).await?;
    assert_eq!(named(&listed, "Chosen")?["adopted"], true);
    assert_eq!(named(&listed, "Skipped")?["adopted"], false);
    // Each administrator keeps at most eight pending previews.
    for _ in 0..8 {
        call(
            &panel,
            &cookie,
            Method::POST,
            "/ordered-subscription-source-previews",
            Some(preview_body.clone()),
            StatusCode::CREATED,
        )
        .await?;
    }
    call(
        &panel,
        &cookie,
        Method::POST,
        "/ordered-subscription-source-previews",
        Some(preview_body),
        StatusCode::CONFLICT,
    )
    .await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn adoption_uses_the_current_version_and_keeps_chain_selection_unchanged(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let receipt = call(&panel, &cookie, Method::POST, "/ordered-subscription-sources", Some(json!({"request_id":Uuid::new_v4(),"name":"Inline","input":{"kind":"inline","content":config(vec![proxy("Node", "a.example.com")])}})), StatusCode::ACCEPTED).await?;
    let id = receipt["source_id"].as_i64().context("source")?;
    run_worker(&panel).await?;
    let listed = nodes(&panel, &cookie, id).await?;
    let node = named(&listed, "Node")?;
    assert_eq!(node["adopted"], false, "refreshed nodes wait for adoption");
    assert_eq!(
        node["selectable"], true,
        "adoption does not gate chain selection"
    );
    let path = format!(
        "/ordered-subscription-sources/{id}/nodes/{}",
        node["id"].as_str().context("node")?
    );
    let body = |adopted: bool, version: &Value, metadata: i64| json!({"adopted":adopted,"settings_revision":1,"identity_epoch":1,"node_version_id":version,"metadata_revision":metadata});
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &path,
        Some(body(true, &json!(Uuid::new_v4()), 0)),
        StatusCode::CONFLICT,
    )
    .await?;
    let adopted = call(
        &panel,
        &cookie,
        Method::PATCH,
        &path,
        Some(body(true, &node["version_id"], 0)),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(adopted["adopted"], true);
    assert_eq!(node["metadata_revision"], 0);
    assert_eq!(
        adopted["metadata_revision"], 1,
        "node pages report the shared catalog revision"
    );
    // Adoption shares the catalog metadata revision, so a stale page is refused.
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &path,
        Some(body(false, &node["version_id"], 0)),
        StatusCode::CONFLICT,
    )
    .await?;
    let released = call(
        &panel,
        &cookie,
        Method::PATCH,
        &path,
        Some(body(false, &node["version_id"], 1)),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(released["adopted"], false);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn public_ids_share_the_numbered_source_sequences(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let receipt = call(&panel, &cookie, Method::POST, "/ordered-subscription-sources", Some(json!({"request_id":Uuid::new_v4(),"name":"Inline","input":{"kind":"inline","content":config(vec![proxy("One", "a.example.com"), proxy("Two", "b.example.com")])}})), StatusCode::ACCEPTED).await?;
    run_worker(&panel).await?;
    let id = receipt["source_id"].as_i64().context("source")?;
    let listed = nodes(&panel, &cookie, id).await?;
    let issued: Vec<i64> = listed
        .iter()
        .filter_map(|node| node["public_id"].as_i64())
        .collect();
    assert_eq!(issued.len(), 2);
    let next_node: i64 = sqlx::query_scalar("SELECT nextval('singbox_external_nodes_id_seq')")
        .fetch_one(&pool)
        .await?;
    assert!(issued.iter().all(|value| *value < next_node));
    let (max_version, next_version): (i64, i64) = sqlx::query_as("SELECT (SELECT MAX(public_id) FROM singbox_ordered_external_node_versions),nextval('singbox_external_node_versions_id_seq')").fetch_one(&pool).await?;
    assert!(max_version < next_version);
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn existing_ordered_sources_keep_their_behaviour_after_the_upgrade(
    pool: PgPool,
) -> Result<()> {
    let migrations = sqlx::migrate!();
    let old = Migrator {
        migrations: Cow::Owned(
            migrations
                .iter()
                .filter(|migration| migration.version <= 60)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    old.run(&pool).await?;
    let source: i64 = sqlx::query_scalar("INSERT INTO singbox_ordered_subscription_sources(name,kind,input_config,refresh_interval_secs,next_refresh_at,created_at,updated_at) VALUES('TEST_ONLY existing','url','{\"kind\":\"url\",\"url\":\"https://source.example.com/a\"}',86400,5,1,1) RETURNING id")
        .fetch_one(&pool).await?;
    let job = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_subscription_source_jobs(id,source_id,settings_revision,identity_epoch,parser_version,status,stage,created_at,finished_at) VALUES($1,$2,1,1,'apsp-subscription-v1','succeeded','done',1,1)")
        .bind(job).bind(source).execute(&pool).await?;
    let revision = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_subscription_source_revisions(id,source_id,job_id,settings_revision,identity_epoch,parser_version,format,raw_digest,parsed_at,counts,warnings) VALUES($1,$2,$3,1,1,'apsp-subscription-v1','sing_box_json',repeat('0',64),1,'{}','[]')")
        .bind(revision).bind(source).bind(job).execute(&pool).await?;
    let node = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_ordered_external_nodes(id,source_id,identity_epoch,identity_key,identity_state,created_at) VALUES($1,$2,1,'fingerprint:TEST_ONLY',  'unique',1)")
        .bind(node).bind(source).execute(&pool).await?;
    let version = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_ordered_external_node_versions(id,node_id,source_revision_id,public_preview,capabilities,supported,reasons) VALUES($1,$2,$3,'{}','{}',FALSE,'[]')")
        .bind(version).bind(node).bind(revision).execute(&pool).await?;
    migrations.run(&pool).await?;
    let (agent, auto_refresh, next, changes): (Option<String>, bool, Option<i64>, Value) = sqlx::query_as("SELECT user_agent,auto_refresh,next_refresh_at,changes FROM singbox_ordered_subscription_sources WHERE id=$1")
        .bind(source).fetch_one(&pool).await?;
    assert!(agent.is_none(), "existing sources still send no User-Agent");
    assert!(auto_refresh);
    assert_eq!(next, Some(5));
    assert_eq!(
        changes,
        json!({"added":0,"updated":0,"missing":0,"unsupported":0})
    );
    let (adopted, node_public): (bool, i64) =
        sqlx::query_as("SELECT adopted,public_id FROM singbox_ordered_external_nodes WHERE id=$1")
            .bind(node)
            .fetch_one(&pool)
            .await?;
    assert!(!adopted, "existing nodes stay out of the catalog");
    let (version_public, legacy): (i64, Option<Value>) = sqlx::query_as(
        "SELECT public_id,legacy_config FROM singbox_ordered_external_node_versions WHERE id=$1",
    )
    .bind(version)
    .fetch_one(&pool)
    .await?;
    assert!(node_public > 0 && version_public > 0 && legacy.is_none());
    // Wider refresh periods are accepted by the new constraint.
    sqlx::query(
        "UPDATE singbox_ordered_subscription_sources SET refresh_interval_secs=300 WHERE id=$1",
    )
    .bind(source)
    .execute(&pool)
    .await?;
    assert!(
        sqlx::query(
            "UPDATE singbox_ordered_subscription_sources SET refresh_interval_secs=299 WHERE id=$1"
        )
        .bind(source)
        .execute(&pool)
        .await
        .is_err()
    );
    // Version history stays immutable after the new columns were added.
    assert!(
        sqlx::query("UPDATE singbox_ordered_external_node_versions SET supported=TRUE WHERE id=$1")
            .bind(version)
            .execute(&pool)
            .await
            .is_err()
    );
    Ok(())
}
