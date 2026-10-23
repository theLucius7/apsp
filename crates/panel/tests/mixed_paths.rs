#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use anyhow::{Context, Result, ensure};
use apsp_panel::plugins::singbox::mixed_paths;
use apsp_protocol::{Bundle, now_timestamp};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

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
async fn host(panel: &TestPanel, cookie: &str, name: &str) -> Result<i64> {
    let host = panel.create_server(cookie, name).await?;
    panel.enable_plugin(cookie, host).await?;
    sqlx::query("UPDATE servers SET capabilities='[\"singbox\",\"runtime:dependency-validation:v1\"]',last_seen=$2 WHERE id=$1").bind(host).bind(now_timestamp()).execute(&panel.state.pool).await?;
    Ok(host)
}
async fn managed(panel: &TestPanel, cookie: &str, host: i64) -> Result<i64> {
    let node=call(panel,cookie,Method::POST,"/nodes",Some(json!({"name":"内部节点","server_id":host,"public_host":format!("hop-{host}.example.com"),"sni":"www.example.com"}))).await?;
    id(&node)
}
fn chain(host: i64, node: i64) -> Value {
    json!({"name":"测试链路","entry":{"mode":"new","server_id":host,"public_host":"entry.example.com","sni":"www.example.com"},"hops":[{"kind":"managed","node_id":node}]})
}
async fn create(panel: &TestPanel, cookie: &str, item: Value) -> Result<(i64, i64)> {
    let value = call(
        panel,
        cookie,
        Method::POST,
        "/chains/batch",
        Some(json!({"request_id":Uuid::new_v4(),"items":[item]})),
    )
    .await?;
    Ok((
        value["chain_ids"][0].as_i64().context("chain")?,
        value["entry_node_ids"][0].as_i64().context("entry")?,
    ))
}
async fn stage(pool: &PgPool, id: i64) -> Result<String> {
    Ok(sqlx::query_scalar("SELECT stage FROM singbox_chain_versions v JOIN singbox_chains c ON c.id=v.chain_id WHERE c.id=$1 AND v.generation=COALESCE(c.pending_generation,c.active_generation)").bind(id).fetch_one(pool).await?)
}
async fn applied(panel: &TestPanel) -> Result<()> {
    panel.publish_now().await?;
    // Synthetic boundary acknowledgements test the panel state machine. The
    // real Agent authentication and native path execution have separate tests.
    sqlx::query("UPDATE server_module_status SET applied_rev=target_rev,last_result_rev=target_rev,healthy=TRUE,last_error=NULL,updated_at=$1 WHERE module='singbox'").bind(now_timestamp()).execute(&panel.state.pool).await?;
    sqlx::query("UPDATE servers SET last_seen=$1")
        .bind(now_timestamp())
        .execute(&panel.state.pool)
        .await?;
    Ok(())
}
async fn settle(panel: &TestPanel, id: i64) -> Result<()> {
    for _ in 0..30 {
        applied(panel).await?;
        mixed_paths::advance(&panel.state).await?;
        sqlx::query(
            "UPDATE runtime_validations SET result='{\"success\":true}' WHERE result IS NULL",
        )
        .execute(&panel.state.pool)
        .await?;
        if stage(&panel.state.pool, id).await? == "active" {
            return Ok(());
        }
    }
    anyhow::bail!(
        "path did not settle: {}",
        stage(&panel.state.pool, id).await?
    )
}
async fn configuration(pool: &PgPool, server: i64) -> Result<Value> {
    let text:String=sqlx::query_scalar("SELECT bundle FROM deployments WHERE server_id=$1 AND module='singbox' ORDER BY rev DESC LIMIT 1").bind(server).fetch_one(pool).await?;
    let bundle: Bundle = serde_json::from_str(&text)?;
    Ok(serde_json::from_str(&bundle.files["config.json"])?)
}
async fn assign(panel: &TestPanel, cookie: &str, chain: i64) -> Result<i64> {
    let user = id(&panel.create_user(cookie, "路径用户").await?)?;
    let policy = call(
        panel,
        cookie,
        Method::POST,
        "/policy-groups",
        Some(json!({"name":"测试授权","node_ids":[],"chain_ids":[chain]})),
    )
    .await?;
    call(
        panel,
        cookie,
        Method::PUT,
        &format!("/users/{user}/policy-groups"),
        Some(json!({"group_ids":[id(&policy)?]})),
    )
    .await?;
    Ok(user)
}

fn imported(password: &str) -> String {
    json!({"outbounds":[{"type":"http","tag":"外部中间段","server":"airport.example.com","server_port":443,"username":"fixture","password":password}]}).to_string()
}

#[sqlx::test(migrations = "./migrations")]
async fn entry_eligibility_tracks_grants_and_live_path_references(pool: PgPool) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example.com")).await?;
    let cookie = panel.admin_cookie().await?;
    let entry_host = host(&panel, &cookie, "入口资格").await?;
    let hop_host = host(&panel, &cookie, "内部段").await?;
    let entry = managed(&panel, &cookie, entry_host).await?;
    let hop = managed(&panel, &cookie, hop_host).await?;
    async fn resource(panel: &TestPanel, cookie: &str, node: i64) -> Result<Value> {
        call(panel, cookie, Method::GET, "/proxy-resources", None)
            .await?
            .as_array()
            .context("resources")?
            .iter()
            .find(|value| value["kind"] == "direct" && value["id"] == node)
            .cloned()
            .context("direct resource")
    }
    ensure!(resource(&panel, &cookie, entry).await?["entry_eligible"] == true);
    let user = id(&panel.create_user(&cookie, "授权资格用户").await?)?;
    panel.grant(&cookie, user, entry).await?;
    let granted = resource(&panel, &cookie, entry).await?;
    ensure!(granted["entry_eligible"] == false);
    ensure!(granted["reference_count"] == 0);
    ensure!(granted.get("user_id").is_none() && granted.get("uuid").is_none());
    let existing = json!({"name":"资格拒绝", "entry":{"mode":"existing","node_id":entry}, "hops":[{"kind":"managed","node_id":hop}]});
    ensure!(
        panel
            .admin(
                Method::POST,
                &format!("{ROOT}/chains/batch"),
                &cookie,
                Some(json!({"request_id":Uuid::new_v4(),"items":[existing.clone()]}))
            )
            .await?
            .status()
            == StatusCode::CONFLICT
    );
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/users/{user}/accesses/{entry}"),
        None,
    )
    .await?;
    ensure!(resource(&panel, &cookie, entry).await?["entry_eligible"] == true);
    let policy = call(
        &panel,
        &cookie,
        Method::POST,
        "/policy-groups",
        Some(json!({"name":"未分配策略也占用入口", "node_ids":[entry], "chain_ids":[]})),
    )
    .await?;
    ensure!(resource(&panel, &cookie, entry).await?["entry_eligible"] == false);
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/policy-groups/{}", id(&policy)?),
        None,
    )
    .await?;
    ensure!(resource(&panel, &cookie, entry).await?["entry_eligible"] == true);
    sqlx::query("UPDATE nodes SET enabled=FALSE WHERE id=$1")
        .bind(entry)
        .execute(&pool)
        .await?;
    ensure!(resource(&panel, &cookie, entry).await?["entry_eligible"] == false);
    sqlx::query("UPDATE nodes SET enabled=TRUE WHERE id=$1")
        .bind(entry)
        .execute(&pool)
        .await?;
    let (chain_id, _) = create(&panel, &cookie, existing).await?;
    let internal = resource(&panel, &cookie, hop).await?;
    ensure!(
        internal["entry_eligible"] == false
            && internal["reference_count"]
                .as_i64()
                .is_some_and(|count| count > 0)
    );
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/proxy-resources/chain/{chain_id}"),
        None,
    )
    .await?;
    ensure!(resource(&panel, &cookie, hop).await?["entry_eligible"] == true);
    Ok(())
}
async fn source_settled(panel: &TestPanel, cookie: &str, source: i64) -> Result<Value> {
    for _ in 0..200 {
        let value = call(
            panel,
            cookie,
            Method::GET,
            &format!("/subscription-sources/{source}"),
            None,
        )
        .await?;
        if value["active_job_id"].is_null() {
            ensure!(value["last_error"].is_null(), "{value}");
            return Ok(value);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    anyhow::bail!("source import did not settle")
}

#[sqlx::test(migrations = "./migrations")]
async fn batches_are_atomic_idempotent_and_preserve_receipts_after_delete(
    pool: PgPool,
) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example.com")).await?;
    let cookie = panel.admin_cookie().await?;
    let a = host(&panel, &cookie, "入口").await?;
    let b = host(&panel, &cookie, "出口").await?;
    let node = managed(&panel, &cookie, b).await?;
    let request = json!({"request_id":Uuid::new_v4(),"items":[chain(a,node),chain(a,node)]});
    let first = call(
        &panel,
        &cookie,
        Method::POST,
        "/chains/batch",
        Some(request.clone()),
    )
    .await?;
    let second = call(
        &panel,
        &cookie,
        Method::POST,
        "/chains/batch",
        Some(request.clone()),
    )
    .await?;
    ensure!(first == second);
    let entries: Vec<i64> = serde_json::from_value(first["entry_node_ids"].clone())?;
    let ports: Vec<i32> =
        sqlx::query_scalar("SELECT port FROM nodes WHERE id=ANY($1) ORDER BY port")
            .bind(&entries)
            .fetch_all(&pool)
            .await?;
    ensure!(ports == [20000, 20001]);
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM accesses")
            .fetch_one(&pool)
            .await?
            == 0
    );
    let mut changed = request.clone();
    changed["items"][0]["name"] = json!("不同内容");
    ensure!(
        panel
            .admin(
                Method::POST,
                &format!("{ROOT}/chains/batch"),
                &cookie,
                Some(changed)
            )
            .await?
            .status()
            == StatusCode::CONFLICT
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes")
        .fetch_one(&pool)
        .await?;
    let mut bad = chain(a, node);
    bad["entry"]["port"] = json!(22000);
    let response = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/chains/batch"),
            &cookie,
            Some(json!({"request_id":Uuid::new_v4(),"items":[bad.clone(),bad]})),
        )
        .await?;
    ensure!(response.status() == StatusCode::CONFLICT);
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes")
            .fetch_one(&pool)
            .await?
            == count
    );
    for id in first["chain_ids"].as_array().context("ids")? {
        call(
            &panel,
            &cookie,
            Method::DELETE,
            &format!("/proxy-resources/chain/{}", id.as_i64().context("id")?),
            None,
        )
        .await?;
    }
    ensure!(
        call(
            &panel,
            &cookie,
            Method::POST,
            "/chains/batch",
            Some(request)
        )
        .await?
            == first
    );
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM singbox_live_chains")
            .fetch_one(&pool)
            .await?
            == 0
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn candidate_proof_barrier_and_cleanup_gate_subscription_and_endpoint_mutation(
    pool: PgPool,
) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example.com")).await?;
    let cookie = panel.admin_cookie().await?;
    let a = host(&panel, &cookie, "入口").await?;
    let b = host(&panel, &cookie, "内部").await?;
    let hop = managed(&panel, &cookie, b).await?;
    let (chain, entry) = create(&panel, &cookie, chain(a, hop)).await?;
    let user = assign(&panel, &cookie, chain).await?;
    applied(&panel).await?;
    let first = configuration(&pool, a).await?;
    ensure!(
        first["inbounds"]
            .as_array()
            .context("inbounds")?
            .iter()
            .all(|inbound| inbound["users"].as_array().is_none_or(Vec::is_empty))
    );
    ensure!(
        !sqlx::query_scalar::<_, bool>("SELECT singbox_path_ready($1)")
            .bind(chain)
            .fetch_one(&pool)
            .await?
    );
    settle(&panel, chain).await?;
    ensure!(
        sqlx::query_scalar::<_, bool>("SELECT singbox_path_ready($1)")
            .bind(chain)
            .fetch_one(&pool)
            .await?
    );
    let active = configuration(&pool, a).await?;
    let rules = active["route"]["rules"].as_array().context("rules")?;
    ensure!(
        rules
            .iter()
            .any(|r| r["inbound"] == json!([format!("node-{entry}")])
                && r["outbound"] == format!("path-{chain}-g1-h0"))
    );
    let internal = configuration(&pool, b).await?;
    ensure!(internal["experimental"]["v2ray_api"]["stats"]["users"] == json!([]));
    let preview = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/users/{user}/subscription"),
        None,
    )
    .await?;
    let text = preview.to_string();
    ensure!(
        !text.contains("relay_") && !text.contains("private_key") && !text.contains("path-checks")
    );
    for (method, body) in [
        (Method::PATCH, Some(json!({"port":22001}))),
        (Method::DELETE, None),
    ] {
        ensure!(
            panel
                .admin(method, &format!("{ROOT}/nodes/{hop}"), &cookie, body)
                .await?
                .status()
                == StatusCode::CONFLICT
        );
    }
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/nodes/{hop}"),
        Some(json!({"enabled":false})),
    )
    .await?;
    applied(&panel).await?;
    let paused = configuration(&pool, a).await?;
    ensure!(
        paused["inbounds"]
            .as_array()
            .context("inbounds")?
            .iter()
            .all(|inbound| inbound["users"].as_array().is_none_or(Vec::is_empty))
    );
    ensure!(
        !sqlx::query_scalar::<_, bool>("SELECT singbox_path_ready($1)")
            .bind(chain)
            .fetch_one(&pool)
            .await?
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn missing_capability_waits_and_failed_probe_never_exposes_a_direct_entry(
    pool: PgPool,
) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example.com")).await?;
    let cookie = panel.admin_cookie().await?;
    let a = host(&panel, &cookie, "入口").await?;
    let b = host(&panel, &cookie, "内部").await?;
    let hop = managed(&panel, &cookie, b).await?;
    let (chain, _) = create(&panel, &cookie, chain(a, hop)).await?;
    assign(&panel, &cookie, chain).await?;
    sqlx::query("UPDATE servers SET capabilities='[\"singbox\"]' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await?;
    applied(&panel).await?;
    mixed_paths::advance(&panel.state).await?;
    ensure!(stage(&pool, chain).await? == "waiting_dependencies");
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runtime_validations")
            .fetch_one(&pool)
            .await?
            == 0
    );
    sqlx::query(
        "UPDATE servers SET capabilities='[\"singbox\",\"runtime:dependency-validation:v1\"]'",
    )
    .execute(&pool)
    .await?;
    for _ in 0..10 {
        applied(&panel).await?;
        mixed_paths::advance(&panel.state).await?;
        if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runtime_validations")
            .fetch_one(&pool)
            .await?
            > 0
        {
            break;
        }
    }
    ensure!(stage(&pool, chain).await? == "checking_candidate");
    sqlx::query("UPDATE runtime_validations SET result='{\"success\":false}' WHERE result IS NULL")
        .execute(&pool)
        .await?;
    mixed_paths::advance(&panel.state).await?;
    ensure!(stage(&pool, chain).await? == "rolling_back");
    applied(&panel).await?;
    mixed_paths::advance(&panel.state).await?;
    applied(&panel).await?;
    ensure!(stage(&pool, chain).await? == "failed");
    let config = configuration(&pool, a).await?;
    ensure!(
        config["inbounds"]
            .as_array()
            .context("inbounds")?
            .iter()
            .all(|i| i["users"].as_array().is_none_or(Vec::is_empty))
    );
    ensure!(
        !sqlx::query_scalar::<_, bool>("SELECT singbox_path_ready($1)")
            .bind(chain)
            .fetch_one(&pool)
            .await?
    );
    let detail = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/proxy-resources/chain/{chain}"),
        None,
    )
    .await?;
    ensure!(detail["resource"]["last_error"].as_str().is_some());
    ensure!(!detail.to_string().contains("private_key"));
    let retry = call(
        &panel,
        &cookie,
        Method::POST,
        &format!("/proxy-resources/chain/{chain}/apply-node-versions"),
        Some(json!({"expected_generation":1,"versions":[]})),
    )
    .await?;
    ensure!(retry["generation"] == 2);
    settle(&panel, chain).await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn rotating_external_middle_freezes_versions_and_failed_candidate_keeps_old_path(
    pool: PgPool,
) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example.com")).await?;
    let cookie = panel.admin_cookie().await?;
    let a = host(&panel, &cookie, "入口").await?;
    let b = host(&panel, &cookie, "最终出口").await?;
    let managed = managed(&panel, &cookie, b).await?;
    let source = id(&call(
        &panel,
        &cookie,
        Method::POST,
        "/subscription-sources",
        Some(json!({"name":"机场","kind":"inline","content":imported("first-fixture-secret")})),
    )
    .await?)?;
    source_settled(&panel, &cookie, source).await?;
    let imported_nodes = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{source}/nodes"),
        None,
    )
    .await?;
    let external = imported_nodes[0]["id"].as_i64().context("external")?;
    let version = imported_nodes[0]["node_version_id"]
        .as_i64()
        .context("version")?;
    let mut input = chain(a, managed);
    input["hops"] = json!([{"kind":"subscription","source_id":source,"external_node_id":external,"node_version_id":version,"update_mode":"follow_node"},{"kind":"managed","node_id":managed}]);
    let (chain, _) = create(&panel, &cookie, input).await?;
    assign(&panel, &cookie, chain).await?;
    settle(&panel, chain).await?;
    let original: Vec<(i64, String, Value)> = sqlx::query_as(
        "SELECT rev,bundle_sha256,source_json FROM deployments WHERE server_id=$1 ORDER BY rev",
    )
    .bind(a)
    .fetch_all(&pool)
    .await?;
    let before = configuration(&pool, a).await?;
    ensure!(
        before["outbounds"]
            .as_array()
            .context("outbounds")?
            .iter()
            .any(|o| o["detour"] == format!("path-{chain}-g1-h0")
                && o["tag"] == format!("path-{chain}-g1-h1"))
    );
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/subscription-sources/{source}"),
        Some(json!({"settings_revision":1,"content":imported("second-fixture-secret")})),
    )
    .await?;
    source_settled(&panel, &cookie, source).await?;
    mixed_paths::follow_updates(&panel.state).await?;
    let selected: Value = sqlx::query_scalar(
        "SELECT path_json FROM singbox_chain_versions WHERE chain_id=$1 AND generation=2",
    )
    .bind(chain)
    .fetch_one(&pool)
    .await?;
    ensure!(selected["hops"][0]["outbound"]["password"] == "second-fixture-secret");
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/subscription-sources/{source}"),
        Some(json!({"settings_revision":2,"content":imported("third-fixture-secret")})),
    )
    .await?;
    source_settled(&panel, &cookie, source).await?;
    mixed_paths::follow_updates(&panel.state).await?;
    ensure!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM singbox_chain_versions WHERE chain_id=$1"
        )
        .bind(chain)
        .fetch_one(&pool)
        .await?
            == 2
    );
    for _ in 0..15 {
        applied(&panel).await?;
        mixed_paths::advance(&panel.state).await?;
        if sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM runtime_validations WHERE generation=2",
        )
        .fetch_one(&pool)
        .await?
            > 0
        {
            break;
        }
    }
    sqlx::query("UPDATE runtime_validations SET result='{\"success\":false}' WHERE generation=2 AND result IS NULL").execute(&pool).await?;
    mixed_paths::advance(&panel.state).await?;
    ensure!(stage(&pool, chain).await? == "rolling_back");
    applied(&panel).await?;
    mixed_paths::advance(&panel.state).await?;
    applied(&panel).await?;
    ensure!(stage(&pool, chain).await? == "failed");
    let preserved = configuration(&pool, a).await?;
    ensure!(
        preserved["route"]["rules"]
            .as_array()
            .context("rules")?
            .iter()
            .any(|r| r["outbound"] == format!("path-{chain}-g1-h1"))
    );
    ensure!(
        !preserved.to_string().contains("second-fixture-secret")
            && !preserved.to_string().contains("third-fixture-secret")
    );
    let third = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscription-sources/{source}/nodes"),
        None,
    )
    .await?;
    call(&panel,&cookie,Method::POST,&format!("/proxy-resources/chain/{chain}/apply-node-versions"),Some(json!({"expected_generation":2,"versions":[{"position":0,"node_version_id":third[0]["node_version_id"]}]}))).await?;
    settle(&panel, chain).await?;
    let state: (i64, i64) = sqlx::query_as(
        "SELECT active_generation,minimum_generation FROM singbox_chains WHERE id=$1",
    )
    .bind(chain)
    .fetch_one(&pool)
    .await?;
    ensure!(state == (3, 3));
    let internal = configuration(&pool, b).await?;
    let identities = internal["inbounds"]
        .as_array()
        .context("inbounds")?
        .iter()
        .flat_map(|i| i["users"].as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    ensure!(identities.len() == 1 && identities[0]["name"] == format!("relay_{chain}_g3_h1"));
    let current: Vec<(i64, String, Value)> = sqlx::query_as(
        "SELECT rev,bundle_sha256,source_json FROM deployments WHERE server_id=$1 ORDER BY rev",
    )
    .bind(a)
    .fetch_all(&pool)
    .await?;
    ensure!(current.starts_with(&original));
    let detail = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/proxy-resources/chain/{chain}"),
        None,
    )
    .await?;
    ensure!(!detail.to_string().contains("fixture-secret"));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn partial_barrier_preserves_candidate_on_capability_loss_and_revocation_covers_issued_floor(
    pool: PgPool,
) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example.com")).await?;
    let cookie = panel.admin_cookie().await?;
    let a = host(&panel, &cookie, "入口").await?;
    let b = host(&panel, &cookie, "内部").await?;
    let hop = managed(&panel, &cookie, b).await?;
    let (chain, _) = create(&panel, &cookie, chain(a, hop)).await?;
    assign(&panel, &cookie, chain).await?;
    for _ in 0..15 {
        applied(&panel).await?;
        mixed_paths::advance(&panel.state).await?;
        sqlx::query("UPDATE runtime_validations SET result='{\"success\":true}' WHERE operation='probe' AND result IS NULL").execute(&pool).await?;
        if stage(&pool, chain).await? == "establishing_barrier" {
            break;
        }
    }
    ensure!(stage(&pool, chain).await? == "establishing_barrier");
    mixed_paths::advance(&panel.state).await?;
    sqlx::query("UPDATE runtime_validations SET result='{\"success\":true}' WHERE operation='barrier' AND server_id=$1").bind(a).execute(&pool).await?;
    sqlx::query("UPDATE servers SET capabilities='[\"singbox\"]' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    applied(&panel).await?;
    mixed_paths::advance(&panel.state).await?;
    ensure!(stage(&pool, chain).await? == "establishing_barrier");
    let entry = configuration(&pool, a).await?;
    ensure!(
        entry["route"]["rules"]
            .as_array()
            .context("rules")?
            .iter()
            .any(|r| r["outbound"] == format!("path-{chain}-g1-h0"))
    );
    ensure!(
        configuration(&pool, b)
            .await?
            .to_string()
            .contains(&format!("relay_{chain}_g1_h0"))
    );
    ensure!(
        sqlx::query_scalar::<_, i64>("SELECT minimum_generation FROM singbox_chains WHERE id=$1")
            .bind(chain)
            .fetch_one(&pool)
            .await?
            == 0
    );
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/nodes/{hop}"),
        Some(json!({"enabled":false})),
    )
    .await?;
    applied(&panel).await?;
    let bundle: String = sqlx::query_scalar(
        "SELECT bundle FROM deployments WHERE server_id=$1 ORDER BY rev DESC LIMIT 1",
    )
    .bind(a)
    .fetch_one(&pool)
    .await?;
    let bundle: Bundle = serde_json::from_str(&bundle)?;
    let constraints: Value = serde_json::from_str(&bundle.files["runtime-constraints.json"])?;
    ensure!(
        constraints["retired"][format!("path-{chain}")] == 1 && constraints["active"] == json!({})
    );
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/nodes/{hop}"),
        Some(json!({"enabled":true})),
    )
    .await?;
    sqlx::query("UPDATE servers SET capabilities='[\"singbox\",\"runtime:dependency-validation:v1\"]' WHERE id=$1").bind(b).execute(&pool).await?;
    settle(&panel, chain).await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn names_do_not_mutate_evidence_and_retired_entry_can_still_be_inspected_and_removed(
    pool: PgPool,
) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example.com")).await?;
    let cookie = panel.admin_cookie().await?;
    let a = host(&panel, &cookie, "入口").await?;
    let b = host(&panel, &cookie, "内部").await?;
    let hop = managed(&panel, &cookie, b).await?;
    let (chain, entry) = create(&panel, &cookie, chain(a, hop)).await?;
    settle(&panel, chain).await?;
    let original: (i64, Value) = sqlx::query_as(
        "SELECT rev,source_json FROM deployments WHERE server_id=$1 ORDER BY rev DESC LIMIT 1",
    )
    .bind(a)
    .fetch_one(&pool)
    .await?;
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/proxy-resources/chain/{chain}"),
        Some(json!({"name":"管理名称"})),
    )
    .await?;
    ensure!(
        sqlx::query_scalar::<_, String>("SELECT name FROM nodes WHERE id=$1")
            .bind(entry)
            .fetch_one(&pool)
            .await?
            == "测试链路"
    );
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/proxy-resources/chain/{chain}"),
        Some(json!({"name":"管理名称","subscription_name":"订阅显示"})),
    )
    .await?;
    applied(&panel).await?;
    let unchanged: (i64, Value) = sqlx::query_as(
        "SELECT rev,source_json FROM deployments WHERE server_id=$1 ORDER BY rev DESC LIMIT 1",
    )
    .bind(a)
    .fetch_one(&pool)
    .await?;
    ensure!(unchanged == original);
    let projection: Value = sqlx::query_scalar(
        "SELECT source_json FROM singbox_deployment_projections WHERE server_id=$1 AND rev=$2",
    )
    .bind(a)
    .bind(original.0)
    .fetch_one(&pool)
    .await?;
    ensure!(projection[0]["name"] == "订阅显示");
    sqlx::query("UPDATE servers SET deleted_at=$2 WHERE id=$1")
        .bind(a)
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    apsp_panel::plugins::singbox::entitlements::refresh(&pool, now_timestamp()).await?;
    let detail = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/proxy-resources/chain/{chain}"),
        None,
    )
    .await?;
    ensure!(detail["resource"]["available"] == false && detail["node"]["id"] == entry);
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/proxy-resources/chain/{chain}"),
        None,
    )
    .await?;
    applied(&panel).await?;
    ensure!(
        !configuration(&pool, b)
            .await?
            .to_string()
            .contains(&format!("relay_{chain}_"))
    );
    ensure!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM singbox_chain_versions WHERE chain_id=$1"
        )
        .bind(chain)
        .fetch_one(&pool)
        .await?
            == 1
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn batch_probes_are_bounded_per_device_instead_of_expiring_in_a_long_queue(
    pool: PgPool,
) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example.com")).await?;
    let cookie = panel.admin_cookie().await?;
    let a = host(&panel, &cookie, "入口").await?;
    let b = host(&panel, &cookie, "内部").await?;
    let hop = managed(&panel, &cookie, b).await?;
    call(&panel,&cookie,Method::POST,"/chains/batch",Some(json!({"request_id":Uuid::new_v4(),"items":[chain(a,hop),chain(a,hop),chain(a,hop),chain(a,hop),chain(a,hop),chain(a,hop)]}))).await?;
    for _ in 0..8 {
        applied(&panel).await?;
        mixed_paths::advance(&panel.state).await?;
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_validations WHERE result IS NULL AND server_id=$1",
    )
    .bind(a)
    .fetch_one(&pool)
    .await?;
    ensure!(count == 4);
    let shortest: i64 =
        sqlx::query_scalar("SELECT MIN(expires_at-requested_at) FROM runtime_validations")
            .fetch_one(&pool)
            .await?;
    ensure!(shortest == 300);
    Ok(())
}
