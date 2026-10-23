#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_panel::plugins::singbox::sources::{
    latest_follow_version_on, load_version_on, refresh_due,
};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;

const ROOT: &str = "/api/plugins/sing-box";

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

fn content(password: &str) -> String {
    json!({"outbounds":[{"type":"shadowsocks","tag":"同一节点","server":"proxy.example.com","server_port":443,"method":"aes-128-gcm","password":password}]}).to_string()
}

async fn settled(panel: &TestPanel, cookie: &str, source: i64) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(20), async {
        // This HTTP-only fixture has no publisher. Drive its production cadence
        // so a best-effort API kick cannot strand jobs while permits are busy.
        let mut refresh = tokio::time::interval(Duration::from_secs(1));
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let value = call(
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

async fn nodes(panel: &TestPanel, cookie: &str, source: i64) -> Result<Vec<Value>> {
    Ok(call(
        panel,
        cookie,
        Method::GET,
        &format!("/subscription-sources/{source}/nodes"),
        None,
    )
    .await?
    .as_array()
    .context("node array")?
    .clone())
}

#[sqlx::test(migrations = "./migrations")]
async fn source_rotation_preserves_identity_versions_and_redacts_every_public_response(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let created = call(
        &panel,
        &cookie,
        Method::POST,
        "/subscription-sources",
        Some(json!({"name":"机场来源","kind":"inline","content":content("first-fixture-secret")})),
    )
    .await?;
    let source = id(&created)?;
    let initial = settled(&panel, &cookie, source).await?;
    let first = nodes(&panel, &cookie, source).await?;
    ensure!(first.len() == 1 && first[0]["selectable"] == true);
    let node = first[0]["id"].as_i64().context("node id")?;
    let first_version = first[0]["node_version_id"].as_i64().context("version id")?;
    let patched = call(&panel, &cookie, Method::PATCH, &format!("/subscription-sources/{source}"), Some(json!({"settings_revision":initial["settings_revision"],"content":content("rotated-fixture-secret")}))).await?;
    let updated = settled(&panel, &cookie, source).await?;
    let second = nodes(&panel, &cookie, source).await?;
    ensure!(second[0]["id"] == node && second[0]["node_version_id"] != first_version);
    let mut connection = pool.acquire().await?;
    let pinned = load_version_on(&mut connection, source, node, first_version, true).await?;
    let latest = latest_follow_version_on(&mut connection, source, node, 1)
        .await?
        .context("follow version")?;
    ensure!(pinned.outbound.0["password"] == "first-fixture-secret");
    ensure!(latest.outbound.0["password"] == "rotated-fixture-secret");
    ensure!(
        pinned.node_version_id != latest.node_version_id
            && pinned.config_sha256 != latest.config_sha256
    );
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM singbox_external_node_versions WHERE external_node_id=$1",
    )
    .bind(node)
    .fetch_one(&pool)
    .await?;
    ensure!(rows == 2);
    let stale = panel
        .admin(
            Method::PATCH,
            &format!("{ROOT}/subscription-sources/{source}"),
            &cookie,
            Some(json!({"settings_revision":1,"name":"过期修改"})),
        )
        .await?;
    ensure!(stale.status() == StatusCode::CONFLICT);
    for public in [
        created,
        initial,
        patched,
        updated,
        json!(first),
        json!(second),
    ] {
        let text = public.to_string();
        ensure!(
            !text.contains("fixture-secret")
                && !text.contains("config_json")
                && !text.contains("secret_content")
        );
    }
    ensure!(
        sqlx::query("UPDATE singbox_external_node_versions SET name='mutated' WHERE id=$1")
            .bind(first_version)
            .execute(&pool)
            .await
            .is_err()
    );
    ensure!(
        sqlx::query("DELETE FROM singbox_source_revisions WHERE source_id=$1")
            .bind(source)
            .execute(&pool)
            .await
            .is_err()
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn missing_ambiguous_and_replaced_sources_do_not_rebind_existing_nodes(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let created = call(
        &panel,
        &cookie,
        Method::POST,
        "/subscription-sources",
        Some(json!({"name":"来源","kind":"inline","content":content("original-fixture-secret")})),
    )
    .await?;
    let source = id(&created)?;
    let initial = settled(&panel, &cookie, source).await?;
    let first = nodes(&panel, &cookie, source).await?;
    let node = first[0]["id"].as_i64().context("node")?;
    let version = first[0]["node_version_id"].as_i64().context("version")?;
    let ambiguous = json!({"outbounds":[{"type":"shadowsocks","server":"proxy.example.com","server_port":443,"method":"aes-128-gcm","password":"first-account"},{"type":"shadowsocks","server":"proxy.example.com","server_port":443,"method":"aes-128-gcm","password":"second-account"}]}).to_string();
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/subscription-sources/{source}"),
        Some(json!({"settings_revision":initial["settings_revision"],"content":ambiguous})),
    )
    .await?;
    let ambiguous = settled(&panel, &cookie, source).await?;
    let previews = nodes(&panel, &cookie, source).await?;
    ensure!(
        previews
            .iter()
            .any(|v| v["id"] == node && v["reason"] == "ambiguous_node_identity")
    );
    let mut connection = pool.acquire().await?;
    ensure!(
        latest_follow_version_on(&mut connection, source, node, 1)
            .await?
            .is_none()
    );
    ensure!(
        load_version_on(&mut connection, source, node, version, false)
            .await?
            .node_version_id
            == version
    );
    ensure!(
        load_version_on(&mut connection, source, node, version, true)
            .await
            .is_err()
    );
    drop(connection);
    call(&panel, &cookie, Method::PATCH, &format!("/subscription-sources/{source}"), Some(json!({"settings_revision":ambiguous["settings_revision"],"content":"http://different.example.com:8080#other"}))).await?;
    let missing = settled(&panel, &cookie, source).await?;
    ensure!(
        nodes(&panel, &cookie, source)
            .await?
            .iter()
            .any(|v| v["id"] == node && v["reason"] == "node_missing")
    );
    call(&panel, &cookie, Method::PATCH, &format!("/subscription-sources/{source}"), Some(json!({"settings_revision":missing["settings_revision"],"content":content("replacement-fixture-secret"),"replace_source":true}))).await?;
    let replaced = settled(&panel, &cookie, source).await?;
    ensure!(replaced["identity_epoch"] == 2);
    let previews = nodes(&panel, &cookie, source).await?;
    ensure!(
        previews
            .iter()
            .any(|v| v["id"] == node && v["reason"] == "source_replaced")
    );
    ensure!(
        previews
            .iter()
            .any(|v| v["id"] != node && v["selectable"] == true && v["identity_epoch"] == 2)
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn archival_and_deletion_protect_all_path_generations_and_preserve_history(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let source = id(&call(
        &panel,
        &cookie,
        Method::POST,
        "/subscription-sources",
        Some(json!({"name":"来源","kind":"inline","content":content("fixture-password")})),
    )
    .await?)?;
    let initial = settled(&panel, &cookie, source).await?;
    let preview = nodes(&panel, &cookie, source).await?.remove(0);
    let node = preview["id"].as_i64().context("node")?;
    let version = preview["node_version_id"].as_i64().context("version")?;
    let server = panel.create_server(&cookie, "入口服务器").await?;
    let entry = id(&panel.create_node(&cookie, server, "入口").await?)?;
    let chain: i64 = sqlx::query_scalar("INSERT INTO singbox_chains(name,entry_node_id,path_kind) VALUES('source reference fixture',$1,'mixed') RETURNING id").bind(entry).fetch_one(&pool).await?;
    sqlx::query("INSERT INTO singbox_chain_versions(chain_id,generation,path_json,semantic_hash,networks,stage,created_at,updated_at) VALUES($1,1,'{}','fixture','{}','failed',0,0)").bind(chain).execute(&pool).await?;
    sqlx::query("INSERT INTO singbox_chain_hops(chain_id,generation,position,kind,source_id,external_node_id,external_version_id,update_mode) VALUES($1,1,0,'subscription',$2,$3,$4,'pinned')").bind(chain).bind(source).bind(node).bind(version).execute(&pool).await?;
    let response = panel
        .admin(
            Method::DELETE,
            &format!("{ROOT}/subscription-sources/{source}"),
            &cookie,
            None,
        )
        .await?;
    ensure!(response.status() == StatusCode::CONFLICT);
    let archived = call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/subscription-sources/{source}"),
        Some(json!({"settings_revision":initial["settings_revision"],"archived":true})),
    )
    .await?;
    ensure!(archived["dependency_ids"] == json!([chain]));
    ensure!(
        !nodes(&panel, &cookie, source)
            .await?
            .iter()
            .any(|v| v["selectable"] == true)
    );
    let mut connection = pool.acquire().await?;
    ensure!(
        load_version_on(&mut connection, source, node, version, false)
            .await?
            .node_version_id
            == version
    );
    ensure!(
        latest_follow_version_on(&mut connection, source, node, 1)
            .await?
            .is_none()
    );
    drop(connection);
    sqlx::query("UPDATE singbox_chains SET deleted_at=1 WHERE id=$1")
        .bind(chain)
        .execute(&pool)
        .await?;
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/subscription-sources/{source}"),
        None,
    )
    .await?;
    let retained: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM singbox_external_node_versions WHERE id=$1")
            .bind(version)
            .fetch_one(&pool)
            .await?;
    ensure!(retained == 1);
    Ok(())
}
