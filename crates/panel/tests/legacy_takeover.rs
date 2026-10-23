#![forbid(unsafe_code)]

//! Legacy two-hop takeover (ADR 0079 phase 3, step S1c): a chain becomes an
//! ordered chain on the generation 0044 froze, without changing the bundles
//! devices run or the subscriptions users receive, and a rollback restores it.

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_panel::plugins::singbox::{
    legacy_takeover as takeover, ordered_paths,
    source_migration::{compare, snapshot},
};
use apsp_protocol::now_timestamp;
use business_support::{TestPanel, id};
use reqwest::Method;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const ROOT: &str = "/api/plugins/sing-box";

async fn ok(
    panel: &TestPanel,
    cookie: &str,
    method: Method,
    path: &str,
    body: Value,
) -> Result<Value> {
    let response = panel
        .admin(method, &format!("{ROOT}{path}"), cookie, Some(body))
        .await?;
    let status = response.status();
    let text = response.text().await?;
    ensure!(status.is_success(), "{path}: {status}: {text}");
    Ok(serde_json::from_str(&text)?)
}

async fn host(panel: &TestPanel, cookie: &str, name: &str) -> Result<i64> {
    let host = panel.create_server(cookie, name).await?;
    panel.enable_plugin(cookie, host).await?;
    sqlx::query("UPDATE servers SET capabilities='[\"singbox\",\"runtime:dependency-validation:v1\"]',last_seen=$2 WHERE id=$1").bind(host).bind(now_timestamp()).execute(&panel.state.pool).await?;
    Ok(host)
}

async fn node(panel: &TestPanel, cookie: &str, server: i64, host: &str) -> Result<i64> {
    id(&ok(
        panel,
        cookie,
        Method::POST,
        "/nodes",
        json!({"name":host,"server_id":server,"public_host":host,"sni":"www.example.com"}),
    )
    .await?)
}

/// Publishes and records that every device applied it, so the two-hop
/// subscription rule (both servers healthy and applied) holds.
async fn applied(panel: &TestPanel) -> Result<()> {
    panel.publish_now().await?;
    sqlx::query("UPDATE server_module_status SET applied_rev=target_rev,last_result_rev=target_rev,healthy=TRUE,last_error=NULL,updated_at=$1 WHERE module='singbox'").bind(now_timestamp()).execute(&panel.state.pool).await?;
    Ok(())
}

async fn grant_chain(panel: &TestPanel, cookie: &str, user: i64, chain: i64) -> Result<()> {
    let policy = ok(
        panel,
        cookie,
        Method::POST,
        "/policy-groups",
        json!({"name":"两跳授权","node_ids":[],"chain_ids":[chain]}),
    )
    .await?;
    ok(
        panel,
        cookie,
        Method::PUT,
        &format!("/users/{user}/policy-groups"),
        json!({"group_ids":[id(&policy)?]}),
    )
    .await?;
    Ok(())
}

fn outcome(report: &takeover::Report, chain: i64) -> Result<&takeover::ChainOutcome> {
    report
        .chains
        .iter()
        .find(|item| item.chain_id == chain)
        .with_context(|| format!("chain {chain}"))
}

async fn kind(pool: &PgPool, chain: i64) -> Result<(String, Option<i64>, Option<Uuid>)> {
    Ok(
        sqlx::query_as("SELECT path_kind,exit_node_id,relay_uuid FROM singbox_chains WHERE id=$1")
            .bind(chain)
            .fetch_one(pool)
            .await?,
    )
}

async fn subscription(panel: &TestPanel, user: &Value) -> Result<String> {
    let url = user["subscription_url"]
        .as_str()
        .context("subscription url")?;
    Ok(panel
        .client
        .get(format!("{url}?format=singbox"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

async fn revisions(pool: &PgPool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE module='singbox'")
            .fetch_one(pool)
            .await?,
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn takeover_keeps_every_byte_and_rollback_restores_the_chain(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (entry_host, exit_host) = (
        host(&panel, &cookie, "入口").await?,
        host(&panel, &cookie, "出口").await?,
    );
    let granted_entry = node(&panel, &cookie, entry_host, "entry-a.example.com").await?;
    let idle_entry = node(&panel, &cookie, entry_host, "entry-b.example.com").await?;
    let exit = node(&panel, &cookie, exit_host, "exit.example.com").await?;
    // Two chains share the exit; only the first has users. Before the
    // publication fix, the idle chain's relay appeared on the exit server.
    let granted = id(&panel
        .import_legacy_chain(&cookie, "已授权两跳", granted_entry, exit)
        .await?)?;
    let idle = id(&panel
        .import_legacy_chain(&cookie, "未授权两跳", idle_entry, exit)
        .await?)?;
    let user = panel.create_user(&cookie, "两跳用户").await?;
    grant_chain(&panel, &cookie, id(&user)?, granted).await?;
    applied(&panel).await?;
    let rendered = subscription(&panel, &user).await?;
    ensure!(
        rendered.contains("entry-a.example.com"),
        "the granted entry is in the subscription"
    );
    let deployed = revisions(&pool).await?;

    let at = now_timestamp();
    let before = snapshot(&panel.state, at).await?;
    let report = takeover::precheck(&panel.state, None).await?;
    ensure!(
        report.succeeded() && report.chains.len() == 2,
        "{}",
        serde_json::to_string(&report)?
    );
    for chain in [granted, idle] {
        ensure!(outcome(&report, chain)?.outcome == "ready");
        ensure!(
            kind(&pool, chain).await?.0 == "legacy",
            "a precheck changes nothing"
        );
    }

    let report = takeover::apply(&panel.state, None).await?;
    ensure!(report.succeeded(), "{}", serde_json::to_string(&report)?);
    for chain in [granted, idle] {
        ensure!(outcome(&report, chain)?.outcome == "taken_over");
        let (path_kind, exit_node, relay) = kind(&pool, chain).await?;
        ensure!(path_kind == "ordered" && exit_node.is_none() && relay.is_none());
        let (saved_exit, phase, generation): (i64, String, i64) = sqlx::query_as("SELECT t.exit_node_id,c.phase,c.applied_generation FROM singbox_legacy_takeovers t JOIN singbox_chains c ON c.id=t.chain_id WHERE t.chain_id=$1").bind(chain).fetch_one(&pool).await?;
        ensure!(saved_exit == exit && phase == "applied" && generation == 1);
    }
    let after = snapshot(&panel.state, at).await?;
    let differences = compare(&before, &after);
    ensure!(
        differences.iter().all(|key| key.starts_with("catalog/")),
        "takeover changed {differences:?}"
    );
    ensure!(subscription(&panel, &user).await? == rendered);
    // The lifecycle accepts the taken-over generation as it is, and a
    // publication finds the same bytes, so devices get no new revision.
    ordered_paths::reconcile_pending(&panel.state).await?;
    let (phase, desired): (String, i64) =
        sqlx::query_as("SELECT phase,desired_generation FROM singbox_chains WHERE id=$1")
            .bind(granted)
            .fetch_one(&pool)
            .await?;
    ensure!(phase == "applied" && desired == 1, "{phase} {desired}");
    sqlx::query("UPDATE servers SET dirty_at=$1")
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    applied(&panel).await?;
    ensure!(
        revisions(&pool).await? == deployed,
        "no new device revision"
    );
    ensure!(subscription(&panel, &user).await? == rendered);
    ensure!(takeover::apply(&panel.state, None).await?.chains.is_empty());

    let report = takeover::rollback(&panel.state, Some(vec![granted])).await?;
    ensure!(
        outcome(&report, granted)?.outcome == "rolled_back",
        "{}",
        serde_json::to_string(&report)?
    );
    let (path_kind, exit_node, relay) = kind(&pool, granted).await?;
    ensure!(path_kind == "legacy" && exit_node == Some(exit) && relay.is_some());
    let restored = snapshot(&panel.state, at).await?;
    let differences = compare(&before, &restored);
    ensure!(
        differences.iter().all(|key| key.starts_with("catalog/")),
        "rollback left {differences:?}"
    );
    let report = takeover::rollback(&panel.state, Some(vec![granted])).await?;
    ensure!(outcome(&report, granted)?.reasons == ["not_taken_over"]);
    ensure!(!report.succeeded());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn takeover_skips_chains_it_cannot_keep_byte_for_byte(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (entry_host, exit_host) = (
        host(&panel, &cookie, "入口").await?,
        host(&panel, &cookie, "出口").await?,
    );
    let mut chains = Vec::new();
    for index in 0..5 {
        let entry = node(
            &panel,
            &cookie,
            entry_host,
            &format!("entry-{index}.example.com"),
        )
        .await?;
        let exit = node(
            &panel,
            &cookie,
            exit_host,
            &format!("exit-{index}.example.com"),
        )
        .await?;
        chains.push((
            id(&panel
                .import_legacy_chain(&cookie, &format!("两跳 {index}"), entry, exit)
                .await?)?,
            entry,
            exit,
        ));
    }
    let user = id(&panel.create_user(&cookie, "直授用户").await?)?;
    // A direct grant on an entry would be dropped once the entry is ordered.
    sqlx::query("INSERT INTO accesses(user_id,node_id,uuid,stat_name,direct_grant) VALUES($1,$2,$3,$4,TRUE)")
        .bind(user).bind(chains[0].1).bind(Uuid::new_v4()).bind(format!("u{user}_n{}", chains[0].1)).execute(&pool).await?;
    let group: i64 =
        sqlx::query_scalar("INSERT INTO singbox_policy_groups(name) VALUES('节点组') RETURNING id")
            .fetch_one(&pool)
            .await?;
    sqlx::query("INSERT INTO singbox_policy_nodes(group_id,node_id) VALUES($1,$2)")
        .bind(group)
        .bind(chains[1].1)
        .execute(&pool)
        .await?;
    // A disabled exit, a changed entry and a deleted chain.
    sqlx::query("UPDATE nodes SET enabled=FALSE WHERE id=$1")
        .bind(chains[2].2)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE nodes SET sni='changed.example.com' WHERE id=$1")
        .bind(chains[3].1)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE singbox_chains SET deleted_at=1 WHERE id=$1")
        .bind(chains[4].0)
        .execute(&pool)
        .await?;

    let report = takeover::apply(
        &panel.state,
        Some(
            chains
                .iter()
                .map(|chain| chain.0)
                .chain([999_999])
                .collect(),
        ),
    )
    .await?;
    ensure!(!report.succeeded());
    let reasons =
        |chain: i64| -> Result<Vec<String>> { Ok(outcome(&report, chain)?.reasons.clone()) };
    ensure!(
        reasons(chains[0].0)?.contains(&"entry_direct_grants".to_owned()),
        "{:?}",
        reasons(chains[0].0)?
    );
    ensure!(reasons(chains[1].0)?.contains(&"entry_policy_node_grants".to_owned()));
    let disabled = reasons(chains[2].0)?;
    ensure!(
        disabled.contains(&"exit_disabled".to_owned())
            && disabled.contains(&"not_available".to_owned()),
        "{disabled:?}"
    );
    ensure!(
        reasons(chains[3].0)? == ["entry_changed"],
        "{:?}",
        reasons(chains[3].0)?
    );
    ensure!(reasons(chains[4].0)? == ["deleted"]);
    ensure!(reasons(999_999)? == ["not_found"]);
    for chain in &chains {
        ensure!(outcome(&report, chain.0)?.outcome == "skipped");
        ensure!(
            kind(&pool, chain.0).await?.0 == "legacy",
            "skipped chains stay legacy"
        );
    }
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM singbox_legacy_takeovers")
        .fetch_one(&pool)
        .await?;
    ensure!(rows == 0);

    // Once the chain leaves the taken-over generation, it cannot simply return.
    sqlx::query("DELETE FROM accesses WHERE node_id=$1")
        .bind(chains[0].1)
        .execute(&pool)
        .await?;
    ensure!(
        outcome(
            &takeover::apply(&panel.state, Some(vec![chains[0].0])).await?,
            chains[0].0
        )?
        .outcome
            == "taken_over"
    );
    sqlx::query("UPDATE singbox_chains SET phase='preparing_dependencies' WHERE id=$1")
        .bind(chains[0].0)
        .execute(&pool)
        .await?;
    let report = takeover::rollback(&panel.state, Some(vec![chains[0].0])).await?;
    ensure!(outcome(&report, chains[0].0)?.reasons == ["changed_since_takeover"]);
    ensure!(kind(&pool, chains[0].0).await?.0 == "ordered");
    Ok(())
}
