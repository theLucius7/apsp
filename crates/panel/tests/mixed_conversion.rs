#![forbid(unsafe_code)]

//! Mixed chains converted into ordered chains (ADR 0079 phase 3, step S1d).
//! Device observations are explicit TEST_ONLY facts; no native process runs.

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
#[allow(dead_code)]
#[path = "ordered_paths/support.rs"]
mod support;

use anyhow::{Context, Result, ensure};
use apsp_panel::plugins::singbox::{mixed_paths, ordered_paths};
use apsp_panel::{AppState, agent_api, runtime_control};
use apsp_protocol::*;
use business_support::{TestPanel, id, release_fixture};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::BTreeMap;
use support::*;
use uuid::Uuid;

/// Two servers with every runtime capability; one mixed chain from a new entry
/// on the first server through a managed node on the second.
async fn mixed_fixture(pool: PgPool) -> Result<(Fixture, i64, i64)> {
    let panel = TestPanel::start_with_public_url(pool, Some("https://panel.example")).await?;
    let archive = release_fixture::archive("sing-box", b"TEST_ONLY native binary never executed")?;
    release_fixture::write(
        &panel.state.config.data_dir,
        "sing-box",
        "1.14.2",
        "sing-box",
        &archive,
        b"TEST_ONLY native binary never executed",
        "tar.gz",
    )?;
    let cookie = panel.admin_cookie().await?;
    let mut servers = Vec::new();
    for name in ["入口", "出口"] {
        let server = panel.create_server(&cookie, name).await?;
        panel.enable_plugin(&cookie, server).await?;
        sqlx::query("UPDATE servers SET capabilities=$2,static_info=$3,last_seen=$4 WHERE id=$1")
            .bind(server)
            .bind(json!(["singbox","runtime:dependency-validation:v1",RUNTIME_CHECKPOINT_CAPABILITY,RUNTIME_RECOVERY_BARRIER_CAPABILITY,RUNTIME_PATH_PROBE_CAPABILITY]))
            .bind(json!({"os":"linux","arch":apsp_protocol::release::native_arch()?,"libc":"gnu","runtime_libc":"gnu"}))
            .bind(now_timestamp())
            .execute(&panel.state.pool).await?;
        servers.push(server);
    }
    let hop = id(&api(&panel, &cookie, Method::POST, "/nodes", Some(json!({"name":"出口节点","server_id":servers[1],"public_host":"exit.example.net","sni":"www.example.com"})), StatusCode::CREATED).await?)?;
    let created = api(&panel, &cookie, Method::POST, "/chains/batch", Some(json!({"request_id":Uuid::new_v4(),"items":[{"name":"混合链路","entry":{"mode":"new","server_id":servers[0],"public_host":"entry.example.net","sni":"www.example.com"},"hops":[{"kind":"managed","node_id":hop}]}]})), StatusCode::CREATED).await?;
    let chain = created["chain_ids"][0].as_i64().context("chain")?;
    let entry = created["entry_node_ids"][0].as_i64().context("entry")?;
    let fixture = Fixture {
        panel,
        cookie,
        servers,
        nodes: vec![entry, hop],
        source: 0,
        external: Value::Null,
    };
    Ok((fixture, chain, entry))
}

async fn stage(fixture: &Fixture, chain: i64) -> Result<String> {
    Ok(sqlx::query_scalar("SELECT v.stage FROM singbox_chain_versions v JOIN singbox_chains c ON c.id=v.chain_id WHERE c.id=$1 AND v.generation=COALESCE(c.pending_generation,c.active_generation)").bind(chain).fetch_one(&fixture.panel.state.pool).await?)
}

/// Drives the mixed path to an active, confirmed generation.
async fn settle_mixed(fixture: &Fixture, chain: i64) -> Result<()> {
    let pool = &fixture.panel.state.pool;
    for _ in 0..30 {
        fixture.panel.publish_now().await?;
        confirm_devices(&fixture.panel.state).await?;
        sqlx::query("UPDATE servers SET last_seen=$1")
            .bind(now_timestamp())
            .execute(pool)
            .await?;
        mixed_paths::advance(&fixture.panel.state).await?;
        sqlx::query(
            "UPDATE runtime_validations SET result='{\"success\":true}' WHERE result IS NULL",
        )
        .execute(pool)
        .await?;
        if stage(fixture, chain).await? == "active" {
            fixture.panel.publish_now().await?;
            confirm_devices(&fixture.panel.state).await?;
            return Ok(());
        }
    }
    anyhow::bail!(
        "mixed path did not settle: {}",
        stage(fixture, chain).await?
    )
}

async fn files(fixture: &Fixture, server: i64) -> Result<BTreeMap<String, String>> {
    let raw: String = sqlx::query_scalar("SELECT bundle FROM deployments WHERE server_id=$1 AND module='singbox' ORDER BY rev DESC LIMIT 1")
        .bind(server).fetch_one(&fixture.panel.state.pool).await?;
    Ok(serde_json::from_str::<Bundle>(&raw)?.files)
}

async fn config(fixture: &Fixture, server: i64) -> Result<Value> {
    Ok(serde_json::from_str(
        &files(fixture, server).await?["config.json"],
    )?)
}

/// Outbounds that user traffic of this entry is routed to.
fn routes(config: &Value, entry: i64) -> Vec<String> {
    config["route"]["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|rule| {
            rule["inbound"] == json!([format!("node-{entry}")]) && rule["action"] == "route"
        })
        .filter_map(|rule| rule["outbound"].as_str().map(str::to_owned))
        .collect()
}

fn has_outbound(config: &Value, tag: &str) -> bool {
    config["outbounds"]
        .as_array()
        .is_some_and(|outbounds| outbounds.iter().any(|outbound| outbound["tag"] == tag))
}

fn identities(config: &Value, node: i64) -> Vec<String> {
    config["inbounds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|inbound| inbound["tag"] == format!("node-{node}"))
        .flat_map(|inbound| inbound["users"].as_array().cloned().unwrap_or_default())
        .filter_map(|user| user["name"].as_str().map(str::to_owned))
        .collect()
}

async fn conversion_state(fixture: &Fixture, chain: i64) -> Result<Option<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT state,ordered_generation FROM singbox_mixed_conversions WHERE chain_id=$1",
    )
    .bind(chain)
    .fetch_optional(&fixture.panel.state.pool)
    .await?)
}

async fn path_kind(fixture: &Fixture, chain: i64) -> Result<String> {
    Ok(
        sqlx::query_scalar("SELECT path_kind FROM singbox_chains WHERE id=$1")
            .bind(chain)
            .fetch_one(&fixture.panel.state.pool)
            .await?,
    )
}

fn entry_uuid(output: &Value) -> Option<Value> {
    subscription_nodes(output)
        .into_iter()
        .find(|node| node["server"] == "entry.example.net")
        .map(|node| node["uuid"].clone())
}

async fn check(fixture: &Fixture, chain: i64) -> Result<Value> {
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::GET,
        &format!("/proxy-resources/chain/{chain}/conversion"),
        None,
        StatusCode::OK,
    )
    .await
}

async fn convert(fixture: &Fixture, chain: i64, generation: i64) -> Result<Value> {
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::POST,
        &format!("/proxy-resources/chain/{chain}/conversion"),
        Some(json!({"expected_generation":generation})),
        StatusCode::ACCEPTED,
    )
    .await
}

/// Any successful status, for endpoints whose status code is not under test.
async fn success(
    fixture: &Fixture,
    method: Method,
    path: String,
    body: Option<Value>,
) -> Result<Value> {
    let response = fixture
        .panel
        .admin(method, &format!("{ROOT}{path}"), &fixture.cookie, body)
        .await?;
    let status = response.status();
    let value: Value = response.json().await?;
    ensure!(status.is_success(), "{path}: {status}: {value}");
    Ok(value)
}

/// One round of publication, device confirmations and both lifecycles.
async fn round(fixture: &Fixture, probes: bool) -> Result<()> {
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    finish_controls(&fixture.panel.state, probes).await?;
    ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn conversion_switches_the_entry_after_the_candidate_probe_and_keeps_the_floors(
    pool: PgPool,
) -> Result<()> {
    let (fixture, chain, entry) = mixed_fixture(pool).await?;
    let (entry_server, exit_server, hop) =
        (fixture.servers[0], fixture.servers[1], fixture.nodes[1]);
    let user = authorize(&fixture, chain).await?;
    settle_mixed(&fixture, chain).await?;
    let before = subscription(&fixture, &user).await?;
    let credential = entry_uuid(&before).context("the mixed entry is in the subscription")?;
    let mixed_route = format!("path-{chain}-g1-h0");
    ensure!(routes(&config(&fixture, entry_server).await?, entry) == [mixed_route.as_str()]);

    let precheck = check(&fixture, chain).await?;
    ensure!(precheck["ready"] == true, "{precheck}");
    ensure!(precheck["mixed_generation"] == 1);
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM singbox_ordered_chain_versions")
            .fetch_one(&fixture.panel.state.pool)
            .await?
            == 0,
        "the precheck writes nothing"
    );
    let started = convert(&fixture, chain, 1).await?;
    ensure!(started["ordered_generation"] == 2, "{started}");
    ensure!(path_kind(&fixture, chain).await? == "mixed");
    ensure!(conversion_state(&fixture, chain).await? == Some(("preparing".into(), 2)));
    let tombstones: Vec<(i64, String, i64)> = sqlx::query_as("SELECT server_id,scope,floor FROM singbox_retired_path_scopes WHERE chain_id=$1 ORDER BY server_id")
        .bind(chain).fetch_all(&fixture.panel.state.pool).await?;
    ensure!(
        tombstones
            == [
                (entry_server, format!("path-{chain}"), 1),
                (exit_server, format!("path-{chain}"), 1)
            ],
        "{tombstones:?}"
    );
    // A second start is refused while the first runs.
    let busy = check(&fixture, chain).await?;
    ensure!(
        busy["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.contains(&json!("conversion_in_progress")))
    );

    // Before the switch the mixed route carries the users; the candidate is
    // compiled next to it without a route of its own.
    let candidate_route = format!("chain-{chain}-g2-h1");
    let mut prepared_side_by_side = false;
    let mut switched = false;
    for _ in 0..40 {
        round(&fixture, true).await?;
        // Lifecycle steps leave servers pending; publish and confirm before reading.
        fixture.panel.publish_now().await?;
        confirm_devices(&fixture.panel.state).await?;
        let state = conversion_state(&fixture, chain)
            .await?
            .context("conversion")?
            .0;
        let entry_config = config(&fixture, entry_server).await?;
        if state == "preparing" {
            ensure!(routes(&entry_config, entry) == [mixed_route.as_str()]);
            prepared_side_by_side |= has_outbound(&entry_config, &candidate_route);
            ensure!(
                entry_uuid(&subscription(&fixture, &user).await?).as_ref() == Some(&credential),
                "the mixed entry stays in the subscription while the candidate is prepared"
            );
        } else {
            switched = true;
            ensure!(!routes(&entry_config, entry).contains(&mixed_route));
        }
        if state == "switched" {
            // The old identities stay on the dependency until the barrier.
            ensure!(
                identities(&config(&fixture, exit_server).await?, hop)
                    .contains(&format!("relay_{chain}_g1_h0"))
            );
        }
        if state == "completed" && phase(&fixture, chain).await? == "applied" {
            break;
        }
    }
    ensure!(prepared_side_by_side && switched);
    round(&fixture, true).await?;
    ensure!(conversion_state(&fixture, chain).await? == Some(("completed".into(), 2)));
    ensure!(phase(&fixture, chain).await? == "applied");
    ensure!(path_kind(&fixture, chain).await? == "ordered");
    let view = resource(&fixture, chain).await?;
    ensure!(view["path_state"]["applied_generation"] == 2, "{view}");

    // The entry routes the ordered generation; the mixed generation is gone
    // from every device, and its floors stay in the runtime constraints.
    let entry_files = files(&fixture, entry_server).await?;
    let entry_config: Value = serde_json::from_str(&entry_files["config.json"])?;
    ensure!(routes(&entry_config, entry) == [candidate_route.as_str()]);
    ensure!(!has_outbound(&entry_config, &mixed_route));
    let exit_files = files(&fixture, exit_server).await?;
    let exit_config: Value = serde_json::from_str(&exit_files["config.json"])?;
    let relays = identities(&exit_config, hop);
    ensure!(
        relays.contains(&format!("relay_{chain}_g2_h1")),
        "{relays:?}"
    );
    ensure!(
        !relays.contains(&format!("relay_{chain}_g1_h0")),
        "{relays:?}"
    );
    for files in [&entry_files, &exit_files] {
        let constraints: Value = serde_json::from_str(
            files
                .get("runtime-constraints.json")
                .context("tombstone floors are still published")?,
        )?;
        ensure!(
            constraints["retired"][format!("path-{chain}")] == 1
                && constraints["active"].get(format!("path-{chain}")).is_none(),
            "{constraints}"
        );
    }
    // Users keep the entry, its credential and its accounting identity.
    let after = subscription(&fixture, &user).await?;
    ensure!(entry_uuid(&after).as_ref() == Some(&credential), "{after}");
    let mixed_detail = fixture
        .panel
        .admin(
            Method::GET,
            &format!("{ROOT}/proxy-resources/chain/{chain}"),
            &fixture.cookie,
            None,
        )
        .await?;
    ensure!(mixed_detail.status() == StatusCode::NOT_FOUND);
    let converted = check(&fixture, chain).await?;
    ensure!(
        converted["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.contains(&json!("not_mixed")))
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn conversion_failing_before_the_switch_keeps_the_mixed_route_and_can_be_retried(
    pool: PgPool,
) -> Result<()> {
    let (fixture, chain, entry) = mixed_fixture(pool).await?;
    let entry_server = fixture.servers[0];
    let user = authorize(&fixture, chain).await?;
    settle_mixed(&fixture, chain).await?;
    let credential = entry_uuid(&subscription(&fixture, &user).await?).context("entry")?;
    convert(&fixture, chain, 1).await?;
    // Prepare up to the candidate probe, then fail it.
    for _ in 0..20 {
        if phase(&fixture, chain).await? == "probing_candidate" {
            break;
        }
        fixture.panel.publish_now().await?;
        confirm_devices(&fixture.panel.state).await?;
        ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    }
    ensure!(phase(&fixture, chain).await? == "probing_candidate");
    // The phase change leaves the servers pending; the probe waits for them.
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    ordered_paths::reconcile_pending(&fixture.panel.state).await?;
    finish_controls(&fixture.panel.state, false).await?;
    ordered_paths::reconcile_pending(&fixture.panel.state).await?;

    ensure!(path_kind(&fixture, chain).await? == "mixed");
    ensure!(conversion_state(&fixture, chain).await? == Some(("reverted".into(), 2)));
    let (candidate, error): (Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT c.candidate_generation,m.last_error FROM singbox_chains c JOIN singbox_mixed_conversions m ON m.chain_id=c.id WHERE c.id=$1",
    )
    .bind(chain)
    .fetch_one(&fixture.panel.state.pool)
    .await?;
    ensure!(candidate.is_none() && error.is_some());
    fixture.panel.publish_now().await?;
    confirm_devices(&fixture.panel.state).await?;
    let entry_config = config(&fixture, entry_server).await?;
    ensure!(routes(&entry_config, entry) == [format!("path-{chain}-g1-h0")]);
    ensure!(!has_outbound(
        &entry_config,
        &format!("chain-{chain}-g2-h1")
    ));
    ensure!(entry_uuid(&subscription(&fixture, &user).await?) == Some(credential));
    let detail = api(
        &fixture.panel,
        &fixture.cookie,
        Method::GET,
        &format!("/proxy-resources/chain/{chain}"),
        None,
        StatusCode::OK,
    )
    .await?;
    ensure!(detail["conversion"]["state"] == "reverted", "{detail}");

    // A retry continues the generation numbers.
    ensure!(check(&fixture, chain).await?["ready"] == true);
    let retried = convert(&fixture, chain, 1).await?;
    ensure!(retried["ordered_generation"] == 3, "{retried}");
    let attempts: i32 =
        sqlx::query_scalar("SELECT attempts FROM singbox_mixed_conversions WHERE chain_id=$1")
            .bind(chain)
            .fetch_one(&fixture.panel.state.pool)
            .await?;
    ensure!(attempts == 2);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn conversion_reports_blockers_and_old_creation_entries_are_closed(
    pool: PgPool,
) -> Result<()> {
    let (fixture, chain, entry) = mixed_fixture(pool).await?;
    let pool = fixture.panel.state.pool.clone();
    let reasons = |value: Value| -> Vec<String> {
        value["reasons"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|reason| reason.as_str().map(str::to_owned))
            .collect()
    };
    // Not yet confirmed on the devices.
    ensure!(reasons(check(&fixture, chain).await?).contains(&"mixed_not_ready".to_owned()));
    settle_mixed(&fixture, chain).await?;
    ensure!(check(&fixture, chain).await?["ready"] == true);

    // A node-policy grant on the entry would be dropped by an ordered entry.
    let group = api(
        &fixture.panel,
        &fixture.cookie,
        Method::POST,
        "/policy-groups",
        Some(json!({"name":"节点授权","node_ids":[],"chain_ids":[]})),
        StatusCode::CREATED,
    )
    .await?;
    sqlx::query("INSERT INTO singbox_policy_nodes(group_id,node_id) VALUES($1,$2)")
        .bind(id(&group)?)
        .bind(entry)
        .execute(&pool)
        .await?;
    let blocked = check(&fixture, chain).await?;
    ensure!(reasons(blocked.clone()).contains(&"entry_policy_node_grants".to_owned()));
    ensure!(
        blocked["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().all(Value::is_string))
    );
    sqlx::query("DELETE FROM singbox_policy_nodes WHERE node_id=$1")
        .bind(entry)
        .execute(&pool)
        .await?;

    // Every participating device must support checkpoints, barriers and probes.
    sqlx::query("UPDATE servers SET capabilities='[\"singbox\",\"runtime:dependency-validation:v1\"]' WHERE id=$1")
        .bind(fixture.servers[1]).execute(&pool).await?;
    ensure!(reasons(check(&fixture, chain).await?) == ["capabilities_missing"]);
    sqlx::query("UPDATE servers SET capabilities=$2 WHERE id=$1")
        .bind(fixture.servers[1])
        .bind(json!([
            "singbox",
            "runtime:dependency-validation:v1",
            RUNTIME_CHECKPOINT_CAPABILITY,
            RUNTIME_RECOVERY_BARRIER_CAPABILITY,
            RUNTIME_PATH_PROBE_CAPABILITY
        ]))
        .execute(&pool)
        .await?;

    // A start must name the generation the administrator saw.
    let stale = fixture
        .panel
        .admin(
            Method::POST,
            &format!("{ROOT}/proxy-resources/chain/{chain}/conversion"),
            &fixture.cookie,
            Some(json!({"expected_generation":2})),
        )
        .await?;
    ensure!(stale.status() == StatusCode::CONFLICT);
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM singbox_mixed_conversions")
            .fetch_one(&pool)
            .await?
            == 0
    );

    // Two-hop chains are neither created nor converted here.
    let legacy = fixture
        .panel
        .admin(
            Method::POST,
            &format!("{ROOT}/chains"),
            &fixture.cookie,
            Some(json!({"name":"两跳","entry_node_id":entry,"exit_node_id":fixture.nodes[1]})),
        )
        .await?;
    ensure!(legacy.status() == StatusCode::CONFLICT);
    // After the source migration new mixed chains are refused too.
    sqlx::query("UPDATE singbox_source_migration SET migrated_at=$1")
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    let mixed = fixture.panel.admin(Method::POST, &format!("{ROOT}/chains/batch"), &fixture.cookie, Some(json!({"request_id":Uuid::new_v4(),"items":[{"name":"新混合链路","entry":{"mode":"new","server_id":fixture.servers[0],"public_host":"entry2.example.net","sni":"www.example.com"},"hops":[{"kind":"managed","node_id":fixture.nodes[1]}]}]}))).await?;
    ensure!(mixed.status() == StatusCode::CONFLICT);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn subscription_hops_convert_to_the_migrated_nodes_by_public_id(pool: PgPool) -> Result<()> {
    let (fixture, _, _) = mixed_fixture(pool).await?;
    let panel = &fixture.panel;
    let send =
        |method: Method, path: String, body: Option<Value>| success(&fixture, method, path, body);
    // A numbered source with one node, imported before the migration.
    let content = json!({"outbounds":[{"type":"shadowsocks","tag":"甲","server":"a.example.com","server_port":443,"method":"aes-128-gcm","password":"TEST_ONLY-first"}]}).to_string();
    let source = id(&send(
        Method::POST,
        "/subscription-sources".into(),
        Some(json!({"name":"机场","kind":"inline","content":content})),
    )
    .await?)?;
    for _ in 0..500 {
        let value = send(Method::GET, format!("/subscription-sources/{source}"), None).await?;
        if value["active_job_id"].is_null() {
            ensure!(value["last_error"].is_null(), "{value}");
            break;
        }
        apsp_panel::plugins::singbox::sources::refresh_due(&panel.state).await?;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let nodes = send(
        Method::GET,
        format!("/subscription-sources/{source}/nodes"),
        None,
    )
    .await?;
    let node = nodes
        .as_array()
        .and_then(|nodes| nodes.first())
        .context("imported node")?
        .clone();
    let created = send(Method::POST, "/chains/batch".into(), Some(json!({"request_id":Uuid::new_v4(),"items":[{"name":"订阅链路","entry":{"mode":"new","server_id":fixture.servers[0],"public_host":"entry2.example.net","sni":"www.example.com"},"hops":[{"kind":"subscription","source_id":source,"external_node_id":node["id"],"node_version_id":node["node_version_id"],"update_mode":"pinned"},{"kind":"managed","node_id":fixture.nodes[1]}]}]}))).await?;
    let chain = created["chain_ids"][0].as_i64().context("chain")?;
    settle_mixed(&fixture, chain).await?;
    let blocked = check(&fixture, chain).await?;
    ensure!(
        blocked["reasons"] == json!(["sources_not_migrated"]),
        "{blocked}"
    );

    let report = apsp_panel::plugins::singbox::source_migration::apply(&panel.state.pool).await?;
    ensure!(report.migrated);
    panel.publish_now().await?;
    confirm_devices(&panel.state).await?;
    let ready = check(&fixture, chain).await?;
    ensure!(ready["ready"] == true, "{ready}");
    convert(&fixture, chain, 1).await?;
    // The ordered hop is the imported node and version under the same public
    // ids, in the source that replaced the numbered one; the mode is kept.
    let (node_id, version_id, mode, mapped): (i64, i64, String, bool) = sqlx::query_as("SELECT n.public_id,v.public_id,h.update_mode,EXISTS(SELECT 1 FROM singbox_source_id_map m WHERE m.a_source_id=$2 AND m.b_source_id=h.source_id) FROM singbox_ordered_chain_hops h JOIN singbox_ordered_external_nodes n ON n.id=h.external_node_id JOIN singbox_ordered_external_node_versions v ON v.id=h.node_version_id WHERE h.chain_id=$1 AND h.generation=2 AND h.position=1")
        .bind(chain).bind(source).fetch_one(&panel.state.pool).await?;
    ensure!(json!(node_id) == node["id"] && json!(version_id) == node["node_version_id"]);
    ensure!(mode == "pinned" && mapped);
    let managed: i64 = sqlx::query_scalar("SELECT managed_node_id FROM singbox_ordered_chain_hops WHERE chain_id=$1 AND generation=2 AND position=2")
        .bind(chain).fetch_one(&panel.state.pool).await?;
    ensure!(managed == fixture.nodes[1]);
    Ok(())
}
