#![forbid(unsafe_code)]
mod business_support;
use business_support::release_fixture;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use anyhow::Result;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn ready(panel: &TestPanel, server: i64) -> Result<release_fixture::SignedTestAgent> {
    let names = ["nodequality", "ipquality", "network-workbench"];
    let components = release_fixture::pinned_components(&names)?;
    let mut agents = release_fixture::write_signed_agents(
        &panel.state.config.data_dir,
        "0.4.0",
        &["linux-musl-amd64", "linux-musl-arm64"],
        &components,
    )?;
    let signed = agents.swap_remove(0);
    panel
        .observe_signed_agent(
            server,
            &signed,
            &[
                "diagnostic:nodequality",
                "diagnostic:nodequality-modes",
                apsp_observability::diagnostic_tools::ipquality::CAPABILITY,
                apsp_protocol::DIAGNOSTIC_SECTIONS_CAPABILITY,
                apsp_protocol::DIAGNOSTIC_SERVICE_CAPABILITY,
                apsp_protocol::DIAGNOSTIC_CPU_CEILING_CAPABILITY,
                apsp_protocol::DIAGNOSTIC_COMPLETION_CAPABILITY,
                apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY,
            ],
        )
        .await?;
    Ok(signed)
}

fn workbench_budget() -> Value {
    json!({"duration_secs":60,"memory_bytes":67108864,"disk_bytes":134217728,
        "traffic_bytes":268435456,"rate_bps":20000000,"concurrency":1,
        "cpu_percent":20,"cpu_weight":20,"pause_on_service_error":true})
}

#[sqlx::test(migrations = "./migrations")]
async fn throughput_enqueue_freezes_the_same_exact_latency_identity_for_source_and_listener(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let source: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('TEST_ONLY source') RETURNING id")
            .fetch_one(&panel.state.pool)
            .await?;
    let receiver: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name,static_info) VALUES('TEST_ONLY receiver',$1) RETURNING id",
    )
    .bind(json!({"ip_addresses":["127.0.0.1"]}))
    .fetch_one(&panel.state.pool)
    .await?;
    let target = Uuid::new_v4();
    let end = apsp_protocol::now_timestamp() + 3600;
    sqlx::query("INSERT INTO network_workbench_targets(id,name,host,purpose,authorization_snapshot,authorized_until,created_at,updated_at) VALUES($1,'TEST_ONLY latency','latency.example.test','isolated','original authorization',$2,0,0)")
        .bind(target).bind(end).execute(&panel.state.pool).await?;
    sqlx::query("INSERT INTO network_workbench_tools(id,version,license,source_url,licensed,updated_at) VALUES('iperf3','TEST_ONLY','TEST_ONLY','https://tools.example.test/iperf3',TRUE,0)")
        .execute(&panel.state.pool).await?;
    let plan = json!({"name":"TEST_ONLY exact latency","budget":workbench_budget(),"schedule":null,
        "steps":[{"name":"TEST_ONLY throughput","source":{"kind":"server","server_id":source},"stop_on_failure":true,
        "check":{"kind":"throughput","client_mode":"managed","receiver_server":receiver,"receiver_host":"127.0.0.1",
            "port":5201,"family":"ipv4","direction":"forward","protocol":"tcp","streams":1,
            "duration_secs":1,"rate_bps":1000,"tool_version":"TEST_ONLY","latency_target":"latency.example.test"}}]});
    let run: Value = panel
        .admin(
            Method::POST,
            "/api/network-workbench/runs",
            &cookie,
            Some(plan.clone()),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let snapshot: Value =
        sqlx::query_scalar("SELECT snapshot FROM network_workbench_runs WHERE id=$1")
            .bind(Uuid::parse_str(run["id"].as_str().unwrap())?)
            .fetch_one(&panel.state.pool)
            .await?;
    let entries = snapshot["executions"][0].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["role"], format!("listener:{receiver}"));
    assert_eq!(entries[1]["role"], format!("source:{source}"));
    for execution in entries {
        assert_eq!(execution["latency_target"]["id"], target.to_string());
        assert_eq!(
            execution["latency_target"]["authorization"],
            "original authorization"
        );
        assert_eq!(execution["latency_target"]["authorized_until"], end);
    }
    assert_eq!(entries[0]["latency_target"], entries[1]["latency_target"]);
    let alias = Uuid::new_v4();
    sqlx::query("INSERT INTO network_workbench_targets(id,name,host,purpose,authorization_snapshot,created_at,updated_at) VALUES($1,'TEST_ONLY alias','latency.example.test','isolated','different authorization',0,0)")
        .bind(alias).execute(&panel.state.pool).await?;
    assert_eq!(
        panel
            .admin(
                Method::POST,
                "/api/network-workbench/runs",
                &cookie,
                Some(plan)
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let saved: Value =
        sqlx::query_scalar("SELECT snapshot FROM network_workbench_runs WHERE id=$1")
            .bind(Uuid::parse_str(run["id"].as_str().unwrap())?)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(saved, snapshot);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM network_workbench_runs")
            .fetch_one(&panel.state.pool)
            .await?,
        1
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn revoked_workbench_delivery_blocks_the_entire_same_server_mixed_response_until_cleanup_confirmation(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "混合队列撤销夹具")
        .await?;
    ready(&panel, server).await?;
    sqlx::query("UPDATE servers SET capabilities=capabilities || $2::jsonb WHERE id=$1")
        .bind(server)
        .bind(json!([
            "diagnostic:network-workbench-v1",
            apsp_protocol::DIAGNOSTIC_CANCEL_CAPABILITY
        ]))
        .execute(&panel.state.pool)
        .await?;
    let signed = apsp_governance::releases::signed_agents(&panel.state).await?;
    assert_eq!(signed.len(), 2);
    for target in ["linux-musl-amd64", "linux-musl-arm64"] {
        assert!(signed.iter().any(|entry| {
            entry.name == "agent" && entry.version == "0.4.0" && entry.arch == target
        }));
    }
    let target = Uuid::new_v4();
    sqlx::query("INSERT INTO network_workbench_targets(id,name,host,purpose,authorization_snapshot,created_at,updated_at) VALUES($1,'TEST_ONLY TCP','127.0.0.1','isolated','original authorization',0,0)")
        .bind(target).execute(&panel.state.pool).await?;
    let plan = json!({"name":"TEST_ONLY mixed queue","budget":workbench_budget(),"schedule":null,
        "steps":[{"name":"TEST_ONLY TCP","source":{"kind":"server","server_id":server},"stop_on_failure":true,
        "check":{"kind":"tcp","target_id":target,"port":12345,"family":"ipv4","samples":1}}]});
    let run: Value = panel
        .admin(
            Method::POST,
            "/api/network-workbench/runs",
            &cookie,
            Some(plan),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let workbench: Value = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/diagnostics/network-workbench"),
            &cookie,
            Some(json!({"run_id":run["id"],"step_index":0,"role":format!("source:{server}")})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let revoked = Uuid::parse_str(workbench["id"].as_str().unwrap())?;
    sqlx::query("UPDATE diagnostic_jobs SET status='succeeded',agent_completed=TRUE WHERE id=$1")
        .bind(revoked)
        .execute(&panel.state.pool)
        .await?;
    let response = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/diagnostics/nodequality"),
            &cookie,
            Some(json!({"mode":"daily"})),
        )
        .await?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(
        status == StatusCode::CREATED,
        "mixed queue NodeQuality creation failed ({status}): {}",
        body.chars().take(2048).collect::<String>()
    );
    let nodequality: Value = serde_json::from_str(&body)?;
    assert_eq!(workbench["job"]["tool"], "network-workbench");
    assert!(workbench["job"].get("plugin").is_none());
    assert_eq!(nodequality["job"]["tool"], "nodequality");
    assert!(nodequality["job"].get("plugin").is_none());
    let first = Uuid::parse_str(nodequality["id"].as_str().unwrap())?;
    let revoked_job: Value = sqlx::query_scalar("SELECT job FROM diagnostic_jobs WHERE id=$1")
        .bind(revoked)
        .fetch_one(&panel.state.pool)
        .await?;
    let first_job: Value = sqlx::query_scalar("SELECT job FROM diagnostic_jobs WHERE id=$1")
        .bind(first)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(revoked_job["plugin"], "network-workbench");
    assert_eq!(first_job["plugin"], "nodequality");
    // This isolated fixture represents a legacy/restored inconsistent queue.
    // Normal creation retains the production one-active-job index and mutex.
    sqlx::query("DROP INDEX diagnostic_active_server_idx")
        .execute(&panel.state.pool)
        .await?;
    let last = Uuid::new_v4();
    let mut last_job = first_job.clone();
    last_job["id"] = json!(last);
    let now = apsp_protocol::now_timestamp();
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,status,job,created_at,updated_at,expires_at) VALUES($1,$2,'queued',$3,$4,$4,$5)")
        .bind(last).bind(server).bind(&last_job).bind(now).bind(now+300).execute(&panel.state.pool).await?;
    let preserved = json!({"text":"TEST_ONLY original unknown partial report"});
    sqlx::query("DELETE FROM network_workbench_targets WHERE id=$1")
        .bind(target)
        .execute(&panel.state.pool)
        .await?;
    // Model a recovered mixed queue with the revoked task first, between two
    // valid tasks, and last. Every response must discard earlier starts too.
    for position in 0..3 {
        for id in [first, last, revoked] {
            sqlx::query("UPDATE diagnostic_jobs SET status='queued',agent_completed=FALSE,cancel_requested_at=NULL,cancel_error=NULL WHERE id=$1")
                .bind(id).execute(&panel.state.pool).await?;
        }
        sqlx::query("UPDATE diagnostic_jobs SET report=$2 WHERE id=$1")
            .bind(revoked)
            .bind(&preserved)
            .execute(&panel.state.pool)
            .await?;
        let order = match position {
            0 => [revoked, first, last],
            1 => [first, revoked, last],
            _ => [first, last, revoked],
        };
        for (index, id) in order.into_iter().enumerate() {
            sqlx::query("UPDATE diagnostic_jobs SET created_at=$2 WHERE id=$1")
                .bind(id)
                .bind(now - 10 + index as i64)
                .execute(&panel.state.pool)
                .await?;
        }
        for _ in 0..2 {
            let pending: Value = panel
                .client
                .get(format!("{}/api/agent/v1/diagnostics", panel.base))
                .bearer_auth(&ack.session_token)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            assert_eq!(pending, json!([]));
        }
        let saved: Value =
            sqlx::query_scalar("SELECT to_jsonb(j) FROM diagnostic_jobs j WHERE id=$1")
                .bind(revoked)
                .fetch_one(&panel.state.pool)
                .await?;
        assert_eq!(saved["status"], "cancel_requested");
        assert_eq!(saved["agent_completed"], false);
        assert!(saved["cancel_requested_at"].is_i64());
        assert_eq!(saved["report"], preserved);
        assert_eq!(saved["job"], revoked_job);
        for (id, job) in [(first, &first_job), (last, &last_job)] {
            let saved: Value =
                sqlx::query_scalar("SELECT to_jsonb(j) FROM diagnostic_jobs j WHERE id=$1")
                    .bind(id)
                    .fetch_one(&panel.state.pool)
                    .await?;
            assert_eq!(saved["status"], "queued");
            assert_eq!(saved["job"], *job);
            assert_eq!(saved["agent_completed"], false);
        }
        let cancellations: Value = panel
            .client
            .get(format!(
                "{}/api/agent/v1/diagnostics/cancellations",
                panel.base
            ))
            .bearer_auth(&ack.session_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(cancellations.as_array().unwrap().len(), 1);
        assert_eq!(cancellations[0]["job"]["id"], revoked.to_string());
        assert_eq!(panel.client.post(format!("{}/api/agent/v1/diagnostics/{revoked}/cancel-confirmation",panel.base))
            .bearer_auth(&ack.session_token).json(&json!({"id":revoked,"server_id":server,"plugin":"network-workbench","confirmed":true}))
            .send().await?.status(), StatusCode::NO_CONTENT);
        let pending: Value = panel
            .client
            .get(format!("{}/api/agent/v1/diagnostics", panel.base))
            .bearer_auth(&ack.session_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let ids: Vec<_> = pending
            .as_array()
            .unwrap()
            .iter()
            .map(|job| job["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![first.to_string(), last.to_string()]);
        assert_eq!(
            sqlx::query_scalar::<_, Value>("SELECT report FROM diagnostic_jobs WHERE id=$1")
                .bind(revoked)
                .fetch_one(&panel.state.pool)
                .await?,
            preserved
        );
    }
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn queued_jobs_wait_for_completion_capability_without_erasing_history(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "等待清理能力设备")
        .await?;
    ready(&panel, server).await?;
    let created: Value = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/diagnostics/nodequality"),
            &cookie,
            Some(json!({"mode":"daily"})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(created["job"]["tool"], "nodequality");
    assert!(created["job"].get("plugin").is_none());
    let created_id = Uuid::parse_str(created["id"].as_str().unwrap())?;
    let initial_job: Value = sqlx::query_scalar("SELECT job FROM diagnostic_jobs WHERE id=$1")
        .bind(created_id)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(initial_job["plugin"], "nodequality");
    sqlx::query("UPDATE servers SET capabilities=capabilities-$2 WHERE id=$1")
        .bind(server)
        .bind(apsp_protocol::DIAGNOSTIC_COMPLETION_CAPABILITY)
        .execute(&panel.state.pool)
        .await?;
    let pending: Value = panel
        .client
        .get(format!("{}/api/agent/v1/diagnostics", panel.base))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending, json!([]));
    let saved: Value = sqlx::query_scalar("SELECT to_jsonb(j) FROM diagnostic_jobs j WHERE id=$1")
        .bind(created_id)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(saved["status"], "queued");
    assert_eq!(saved["job"], initial_job);
    ready(&panel, server).await?;
    let pending: Value = panel
        .client
        .get(format!("{}/api/agent/v1/diagnostics", panel.base))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending.as_array().unwrap().len(), 1);
    assert_eq!(pending[0]["id"], created["id"]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn shared_service_keeps_legacy_history_and_serializes_both_creation_routes(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "共用诊断夹具").await?;
    ready(&panel, server).await?;
    let generic = format!("/api/servers/{server}/diagnostics/nodequality");
    let legacy = format!("/api/servers/{server}/node-quality/reports");
    let request = json!({"mode":"daily"});
    let (first, second) = tokio::join!(
        panel.admin(Method::POST, &generic, &cookie, Some(request.clone())),
        panel.admin(Method::POST, &legacy, &cookie, Some(request))
    );
    let (first, second) = (first?, second?);
    let created = if first.status() == StatusCode::CREATED {
        assert_eq!(second.status(), StatusCode::CONFLICT);
        first
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(second.status(), StatusCode::CREATED);
        second
    };
    let record: Value = created.json().await?;
    assert_eq!(
        record["job"]["resource_budget"],
        json!({"memory_max":64*1024*1024,"tasks_max":32,"cpu_weight":10,"io_weight":10,"oom_score_adjust":500})
    );
    assert_eq!(
        record["expected_sections"],
        json!(["net_quality", "environment"])
    );
    let id = record["id"].as_str().unwrap();
    let response=panel.client.post(format!("{}/api/agent/v1/diagnostics/{id}/sections",panel.base)).bearer_auth(&ack.session_token).json(&json!({"id":id,"name":"net_quality","text":"已保存的网络章节","complete":true,"revision":1,"collected_at":apsp_protocol::now_timestamp()})).send().await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = panel
        .client
        .post(format!("{}/api/agent/v1/diagnostics/{id}", panel.base))
        .bearer_auth(&ack.session_token)
        .json(&json!({"id":id,"status":"failed","error":"夹具取消连接，部分报告保留"}))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(record["job"]["tool"], "nodequality");
    assert!(record["job"].get("plugin").is_none());
    let mut old_job: Value = sqlx::query_scalar("SELECT job FROM diagnostic_jobs WHERE id=$1")
        .bind(Uuid::parse_str(id)?)
        .fetch_one(&panel.state.pool)
        .await?;
    let old_id = Uuid::new_v4();
    old_job["id"] = json!(old_id);
    old_job["version"] = json!("a92fca6c0067df29ddd03fdc2fee6f3000f64545-r2");
    old_job.as_object_mut().unwrap().remove("resource_budget");
    old_job.as_object_mut().unwrap().remove("plugin");
    let historical_job = old_job.clone();
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,status,job,report,created_at,updated_at,expires_at,agent_completed,report_completeness) VALUES($1,$2,'succeeded',$3,$4,$5,$5,$5+300,TRUE,'legacy')")
        .bind(old_id).bind(server).bind(old_job.clone()).bind(json!({"text":"迁移前的完整历史原文"})).bind(apsp_protocol::now_timestamp()-1).execute(&panel.state.pool).await?;
    let other_id = Uuid::new_v4();
    old_job["id"] = json!(other_id);
    old_job["plugin"] = json!("other-registered-history");
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,status,job,created_at,updated_at,expires_at) VALUES($1,$2,'failed',$3,$4,$4,$4)").bind(other_id).bind(server).bind(old_job).bind(apsp_protocol::now_timestamp()).execute(&panel.state.pool).await?;
    let view: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/diagnostics"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(view["tools"][0]["tool"], "nodequality");
    assert_eq!(view["tools"][0]["ready"], true);
    assert_eq!(view["reports"].as_array().unwrap().len(), 3);
    let report = view["reports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap();
    assert_eq!(report["job"]["tool"], "nodequality");
    assert!(report["job"].get("plugin").is_none());
    assert_eq!(report["status"], "failed");
    assert_eq!(report["report_completeness"], "partial");
    assert_eq!(report["sections"][0]["text"], "已保存的网络章节");
    let other_report = view["reports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == other_id.to_string())
        .unwrap();
    assert_eq!(other_report["job"]["tool"], "other-registered-history");
    assert!(other_report["job"].get("plugin").is_none());
    let legacy_view: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/node-quality"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(legacy_view["reports"].as_array().unwrap().len(), 2);
    let historical = legacy_view["reports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == old_id.to_string())
        .unwrap();
    assert_eq!(historical["job"], historical_job);
    assert!(historical["job"].get("plugin").is_none());
    assert!(historical["job"].get("tool").is_none());
    assert!(historical["job"].get("resource_budget").is_none());
    assert_eq!(historical["report"]["text"], "迁移前的完整历史原文");
    assert_eq!(historical["report_completeness"], "legacy");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn registered_tools_use_one_server_mutex_and_require_budget_aware_agents(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "互斥与权限夹具")
        .await?;
    ready(&panel, server).await?;
    let path = format!("/api/servers/{server}/diagnostics/nodequality");
    assert_eq!(
        panel
            .client
            .post(format!("{}{}", panel.base, path))
            .bearer_auth(&ack.session_token)
            .json(&json!({"mode":"daily"}))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("/api/servers/{server}/diagnostics/unregistered"),
                &cookie,
                Some(json!({}))
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &path,
                &cookie,
                Some(json!({"mode":"daily","resource_budget":{"memory_max":1}}))
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    sqlx::query("UPDATE servers SET capabilities=capabilities-$2 WHERE id=$1")
        .bind(server)
        .bind(apsp_protocol::DIAGNOSTIC_SERVICE_CAPABILITY)
        .execute(&panel.state.pool)
        .await?;
    assert_eq!(
        panel
            .admin(Method::POST, &path, &cookie, Some(json!({"mode":"daily"})))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    ready(&panel, server).await?;
    sqlx::query("UPDATE servers SET capabilities=capabilities-$2 WHERE id=$1")
        .bind(server)
        .bind(apsp_protocol::DIAGNOSTIC_COMPLETION_CAPABILITY)
        .execute(&panel.state.pool)
        .await?;
    assert_eq!(
        panel
            .admin(Method::POST, &path, &cookie, Some(json!({"mode":"daily"})))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    ready(&panel, server).await?;
    let existing = Uuid::new_v4();
    let now = apsp_protocol::now_timestamp();
    let existing_job = json!({
        "id": existing,
        "plugin": "different-diagnostic-plugin",
        "version": "1.0.0",
        "agent_bundle": {
            "schema": 1,
            "agent_version": "0.4.0",
            "release_tag": "agent-v0.4.0",
            "target": "linux-musl-amd64",
            "source_repo": "theLucius7/apsp",
            "agent_sha256": "1".repeat(64),
            "agent_bytes": 1024,
            "component": "different-diagnostic-plugin",
            "component_version": "1.0.0",
            "component_sha256": "2".repeat(64)
        }
    });
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,job,created_at,updated_at,expires_at) VALUES($1,$2,$3,$4,$4,$4+300)").bind(existing).bind(server).bind(existing_job).bind(now).execute(&panel.state.pool).await?;
    for status in ["queued", "running", "cleaning", "cancel_requested"] {
        let expiry = if matches!(status, "cleaning" | "cancel_requested") {
            now - 1
        } else {
            now + 300
        };
        sqlx::query("UPDATE diagnostic_jobs SET status=$2,expires_at=$3 WHERE id=$1")
            .bind(existing)
            .bind(status)
            .bind(expiry)
            .execute(&panel.state.pool)
            .await?;
        for route in [
            &path,
            &format!("/api/servers/{server}/node-quality/reports"),
        ] {
            assert_eq!(
                panel
                    .admin(Method::POST, route, &cookie, Some(json!({"mode":"daily"})))
                    .await?
                    .status(),
                StatusCode::CONFLICT
            );
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM diagnostic_jobs WHERE server_id=$1")
        .bind(server)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn ipquality_dispatch_freezes_signed_bundle_verifies_receipts_and_keeps_legacy_readonly(
    pool: PgPool,
) -> Result<()> {
    use apsp_observability::diagnostic_tools::ipquality::{
        SOURCE_COMMIT, SOURCE_SHA256, TOOL_VERSION,
    };

    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "IPQuality 签名整包派单夹具")
        .await?;
    let signed = ready(&panel, server).await?;
    let agent_sha = signed.agent_sha256;
    let digests = signed.component_digests;
    let ipquality_sha = digests["ipquality"].clone();

    let created: Value = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/diagnostics/ipquality"),
            &cookie,
            Some(json!({"ip_version": "6"})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = Uuid::parse_str(created["id"].as_str().unwrap())?;
    assert_eq!(created["job"]["tool"], "ipquality");
    assert!(created["job"].get("artifact").is_none_or(Value::is_null));
    assert_eq!(created["job"]["agent_bundle"]["agent_sha256"], agent_sha);
    assert_eq!(
        created["job"]["agent_bundle"]["component_sha256"],
        ipquality_sha
    );
    assert_eq!(
        created["job"]["options"]["expected_component_sha256"],
        ipquality_sha
    );

    let pending: Value = panel
        .client
        .get(format!("{}/api/agent/v1/diagnostics", panel.base))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending.as_array().unwrap().len(), 1);
    assert_eq!(pending[0]["id"], id.to_string());
    assert_eq!(
        pending[0]["options"]["expected_component_sha256"],
        ipquality_sha
    );

    let at = created["created_at"].as_i64().unwrap();
    let egress_ip = "2001:db9::9";
    let receipt_body = |artifact_sha: &str| {
        json!({
            "schema": 1,
            "plugin": "ipquality",
            "version": TOOL_VERSION,
            "job_id": id,
            "ip_version": "6",
            "artifact_sha256": artifact_sha,
            "source_commit": SOURCE_COMMIT,
            "source_sha256": SOURCE_SHA256,
            "started_at": at,
            "finished_at": at + 1,
            "egress_ip": egress_ip,
            "upstream": {
                "Head": {"IP": egress_ip, "Version": "v2026-09-16"},
                "Type": {"Usage": {"ipapi": "hosting"}},
                "Factor": {"Proxy": {"ipapi": false}},
                "Score": {"ipapi": "0"}
            },
            "attempts": [
                {"seq": 1, "provider": "egress-discovery", "dataset": "egress", "target_ip": egress_ip, "url": "https://api64.ipify.org", "status": "succeeded", "attempted_at": at, "elapsed_ms": 1, "http_status": 200, "curl_exit": 0, "response_bytes": 12, "error_kind": null, "error_message": null},
                {"seq": 2, "provider": "check-place-aggregator", "dataset": "ipapi", "target_ip": egress_ip, "url": "https://ipinfo.check.place/fixture", "status": "succeeded", "attempted_at": at, "elapsed_ms": 2, "http_status": 200, "curl_exit": 0, "response_bytes": 128, "error_kind": null, "error_message": null}
            ]
        })
    };

    // Wrong component SHA256 (even if it belongs to another signed component in the same Agent!) must fail.
    let wrong_section = json!({
        "id": id,
        "name": "ipquality_result",
        "text": receipt_body(&digests["nodequality"]).to_string(),
        "complete": true,
        "revision": 1,
        "collected_at": at + 2
    });
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/diagnostics/{id}/sections",
                panel.base
            ))
            .bearer_auth(&ack.session_token)
            .json(&wrong_section)
            .send()
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );

    let valid_section = json!({
        "id": id,
        "name": "ipquality_result",
        "text": receipt_body(&ipquality_sha).to_string(),
        "complete": true,
        "revision": 1,
        "collected_at": at + 2
    });
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/diagnostics/{id}/sections",
                panel.base
            ))
            .bearer_auth(&ack.session_token)
            .json(&valid_section)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/agent/v1/diagnostics/{id}", panel.base))
            .bearer_auth(&ack.session_token)
            .json(
                &json!({"id": id, "status": "succeeded", "report": {"text": "IPQuality completed"}})
            )
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );

    // Historical legacy artifact-proof job accepts validated read-only chapter receipts without overwriting current egress projections.
    let legacy_id = Uuid::new_v4();
    let legacy_job = json!({
        "id": legacy_id,
        "plugin": "ipquality",
        "version": TOOL_VERSION,
        "timeout_secs": 300,
        "artifact": {"sha256": ipquality_sha, "proof": {"TEST_ONLY": "legacy"}},
        "options": {"ip_version": "6"}
    });
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,status,job,created_at,updated_at,expires_at,expected_sections,report_completeness,agent_completed) VALUES($1,$2,'succeeded',$3,$4,$4,$5,ARRAY['ipquality_result','environment'],'complete',TRUE)")
        .bind(legacy_id)
        .bind(server)
        .bind(legacy_job)
        .bind(at - 60)
        .bind(at + 600)
        .execute(&panel.state.pool)
        .await?;
    let tampered_text = {
        let mut v = receipt_body(&digests["nodequality"]);
        v["job_id"] = json!(legacy_id);
        v.to_string()
    };
    let tampered_legacy_upload = json!({
        "id": legacy_id,
        "name": "ipquality_result",
        "text": tampered_text,
        "complete": true,
        "revision": 2,
        "collected_at": at + 3
    });
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/diagnostics/{legacy_id}/sections",
                panel.base
            ))
            .bearer_auth(&ack.session_token)
            .json(&tampered_legacy_upload)
            .send()
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let legacy_egress = "2001:db9::dead";
    let mut valid_legacy_body = receipt_body(&ipquality_sha);
    valid_legacy_body["job_id"] = json!(legacy_id);
    valid_legacy_body["egress_ip"] = json!(legacy_egress);
    valid_legacy_body["upstream"]["Head"]["IP"] = json!(legacy_egress);
    valid_legacy_body["attempts"][0]["target_ip"] = json!(legacy_egress);
    valid_legacy_body["attempts"][1]["target_ip"] = json!(legacy_egress);
    let legacy_upload = json!({
        "id": legacy_id,
        "name": "ipquality_result",
        "text": valid_legacy_body.to_string(),
        "complete": true,
        "revision": 2,
        "collected_at": at + 3
    });
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/diagnostics/{legacy_id}/sections",
                panel.base
            ))
            .bearer_auth(&ack.session_token)
            .json(&legacy_upload)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );

    let ip_view: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/ip-quality"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(ip_view["node_quality"]["ready"], true);
    assert_eq!(
        ip_view["node_quality"]["current_egress_ips"],
        json!([egress_ip])
    );
    let report_ids: Vec<_> = ip_view["node_quality"]["reports"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert!(report_ids.contains(&id.to_string().as_str()));
    assert!(report_ids.contains(&legacy_id.to_string().as_str()));

    // Unfrozen legacy daily/tcpquality jobs must never be dispatched by pending(), while retaining terminal cleanup reporting.
    let unfrozen_daily = Uuid::new_v4();
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,status,job,created_at,updated_at,expires_at) VALUES($1,$2,'queued',$3,$4,$4,$5)")
        .bind(unfrozen_daily)
        .bind(server)
        .bind(json!({
            "id": unfrozen_daily,
            "plugin": "nodequality",
            "version": apsp_observability::diagnostics::TOOL_VERSION,
            "timeout_secs": 90,
            "options": {"mode": "daily"}
        }))
        .bind(at)
        .bind(at + 300)
        .execute(&panel.state.pool)
        .await?;
    let queue: Value = panel
        .client
        .get(format!("{}/api/agent/v1/diagnostics", panel.base))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(queue, json!([]));
    let rejected_daily: (String, String, bool) =
        sqlx::query_as("SELECT status,error,agent_completed FROM diagnostic_jobs WHERE id=$1")
            .bind(unfrozen_daily)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(rejected_daily.0, "failed");
    assert_eq!(
        rejected_daily.1,
        apsp_observability::diagnostics::service::LEGACY_UNFROZEN_DENIAL
    );
    assert!(!rejected_daily.2);
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/diagnostics/{unfrozen_daily}",
                panel.base
            ))
            .bearer_auth(&ack.session_token)
            .json(&json!({"id": unfrozen_daily, "status": "failed", "error": rejected_daily.1}))
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    Ok(())
}
