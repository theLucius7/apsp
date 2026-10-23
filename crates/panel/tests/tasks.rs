#![forbid(unsafe_code)]
mod business_support;
#[path = "probe_support.rs"]
mod probe_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use anyhow::Result;
use apsp_panel::probes::ConfiguredProbe;
use apsp_protocol::{
    CommandResult, CommandStatus, ProbeBatch, ProbeKind, ProbeResult, ProbeSpec, RemoteCommand,
    TaskAck, now_timestamp, telemetry::now_millis,
};
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::json;
use sha2::Digest;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test]
async fn commands_are_authenticated_device_scoped_and_terminal_results_immutable(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "commands").await?;
    let (_other, _other_socket, other_ack) = panel.authenticated_device(&cookie, "other").await?;
    let url = format!("/api/servers/{server}/commands");
    let body = json!({"command":"printf fixture","timeout_secs":5,"ttl_secs":60});
    assert_eq!(
        panel
            .admin(Method::POST, &url, &cookie, Some(body.clone()))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remote_commands WHERE server_id=$1")
        .bind(server)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(queued, 0);
    // This fake device now advertises an explicit node-side opt-in.
    sqlx::query("UPDATE servers SET capabilities=capabilities || '[\"command:execute\"]'::jsonb WHERE id=$1")
        .bind(server).execute(&panel.state.pool).await?;
    assert_eq!(
        panel
            .client
            .post(format!("{}{url}", panel.base))
            .json(&body)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let command: RemoteCommand = panel
        .admin(Method::POST, &url, &cookie, Some(body))
        .await?
        .error_for_status()?
        .json()
        .await?;
    let pending: Vec<RemoteCommand> = panel
        .client
        .get(format!("{}/api/agent/v1/commands", panel.base))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending, vec![command.clone()]);
    let result = CommandResult {
        id: command.id,
        status: CommandStatus::Succeeded,
        finished_at: now_timestamp(),
        stdout: "fixture".into(),
        stderr: String::new(),
        timed_out: false,
        truncated: false,
    };
    let endpoint = format!("{}/api/agent/v1/commands/{}", panel.base, command.id);
    assert_eq!(
        panel
            .client
            .post(&endpoint)
            .bearer_auth(&other_ack.session_token)
            .json(&result)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        let ack: TaskAck = panel
            .client
            .post(&endpoint)
            .bearer_auth(&ack.session_token)
            .json(&result)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(ack.ids, vec![command.id]);
    }
    let mut changed = result;
    changed.stdout = "changed".into();
    assert_eq!(
        panel
            .client
            .post(&endpoint)
            .bearer_auth(&ack.session_token)
            .json(&changed)
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let pending: Vec<RemoteCommand> = panel
        .client
        .get(format!("{}/api/agent/v1/commands", panel.base))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(pending.is_empty());
    Ok(())
}

#[sqlx::test]
async fn probes_preserve_missing_latency_deduplicate_and_acknowledge_deleted_targets(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "probes").await?;
    let spec = ProbeSpec {
        id: Uuid::nil(),
        name: "本地测试".into(),
        kind: ProbeKind::Tcp,
        target: "127.0.0.1".into(),
        port: Some(443),
        interval_secs: 10,
        carrier: String::new(),
        monitor: None,
        execution_authorized: None,
        enabled: true,
    };
    let spec: ConfiguredProbe = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/probes"),
            &cookie,
            Some(probe_support::authorized(serde_json::to_value(spec)?)),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let issued = probe_support::issued(
        &panel.state.pool,
        &panel.client,
        &panel.base,
        server,
        &ack.session_token,
    )
    .await?;
    let result = ProbeResult {
        id: Uuid::new_v4(),
        probe_id: spec.id,
        sampled_at: now_millis(),
        latency_ms: None,
        loss_percent: 100.0,
        address_family: None,
        error: None,
        attempts: None,
        execution: Some(probe_support::execution(&issued, spec.id)?),
    };
    let endpoint = format!("{}/api/agent/v1/probe-results", panel.base);
    for _ in 0..2 {
        panel
            .client
            .post(&endpoint)
            .bearer_auth(&ack.session_token)
            .json(&ProbeBatch {
                results: vec![result.clone()],
            })
            .send()
            .await?
            .error_for_status()?;
    }
    let history: Vec<ProbeResult> = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/probe-results"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(history, vec![result.clone()]);
    let mut changed = result;
    changed.loss_percent = 0.0;
    assert_eq!(
        panel
            .client
            .post(&endpoint)
            .bearer_auth(&ack.session_token)
            .json(&ProbeBatch {
                results: vec![changed.clone()]
            })
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    panel
        .admin(
            Method::DELETE,
            &format!("/api/servers/{server}/probes/{}", spec.id),
            &cookie,
            Some(json!({"revision":spec.revision})),
        )
        .await?
        .error_for_status()?;
    changed.id = Uuid::new_v4();
    let received: TaskAck = panel
        .client
        .post(&endpoint)
        .bearer_auth(&ack.session_token)
        .json(&ProbeBatch {
            results: vec![changed.clone()],
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(received.ids, vec![changed.id]);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe_results")
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[sqlx::test]
async fn probe_destination_is_immutable_and_metadata_edits_preserve_offline_history(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "probe identity")
        .await?;
    let mut original: ConfiguredProbe = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/probes"),
            &cookie,
            Some(probe_support::authorized(json!({"id":Uuid::nil(),"name":"原目标","kind":"tcp","target":"original.test","port":443,"interval_secs":10,"carrier":"原线路","enabled":true}))),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    original.execution_authorized = None;
    let issued = probe_support::issued(
        &panel.state.pool,
        &panel.client,
        &panel.base,
        server,
        &ack.session_token,
    )
    .await?;
    let sample = ProbeResult {
        id: Uuid::new_v4(),
        probe_id: original.id,
        sampled_at: now_millis(),
        latency_ms: Some(0.0),
        loss_percent: 0.0,
        address_family: None,
        error: None,
        attempts: None,
        execution: Some(probe_support::execution(&issued, original.id)?),
    };
    let endpoint = format!("{}/api/agent/v1/probe-results", panel.base);
    panel
        .client
        .post(&endpoint)
        .bearer_auth(&ack.session_token)
        .json(&ProbeBatch {
            results: vec![sample.clone()],
        })
        .send()
        .await?
        .error_for_status()?;
    let saved: (serde_json::Value, String) =
        sqlx::query_as("SELECT result,digest FROM probe_results WHERE id=$1")
            .bind(sample.id)
            .fetch_one(&panel.state.pool)
            .await?;
    let path = format!("/api/servers/{server}/probes/{}", original.id);
    let mut kind = original.clone();
    kind.kind = ProbeKind::Icmp;
    kind.port = None;
    let mut target = original.clone();
    target.target = "different.test".into();
    let mut port = original.clone();
    port.port = Some(8443);
    for changed in [kind, target, port] {
        let response = panel
            .admin(
                Method::PATCH,
                &path,
                &cookie,
                Some(serde_json::to_value(changed)?),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(
            response.json::<serde_json::Value>().await?["error"]
                .as_str()
                .unwrap()
                .contains("请新建目标")
        );
        let spec: serde_json::Value =
            sqlx::query_scalar("SELECT spec FROM network_probes WHERE id=$1")
                .bind(original.id)
                .fetch_one(&panel.state.pool)
                .await?;
        assert_eq!(serde_json::from_value::<ProbeSpec>(spec)?, original.spec);
        let preserved: (serde_json::Value, String) =
            sqlx::query_as("SELECT result,digest FROM probe_results WHERE id=$1")
                .bind(sample.id)
                .fetch_one(&panel.state.pool)
                .await?;
        assert_eq!(preserved, saved);
    }
    let mut metadata = original.clone();
    metadata.name = "更新名称".into();
    metadata.interval_secs = 30;
    metadata.enabled = false;
    let updated: ConfiguredProbe = panel
        .admin(
            Method::PATCH,
            &path,
            &cookie,
            Some(serde_json::to_value(&metadata)?),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    metadata.execution_authorized = Some(true);
    metadata.revision = updated.revision;
    assert_eq!(updated, metadata);
    // Exact accepted history still ACKs after pause; new stale-revision samples are discarded.
    let mut offline = sample.clone();
    offline.id = Uuid::new_v4();
    offline.sampled_at = sample.sampled_at;
    offline.latency_ms = None;
    offline.loss_percent = 100.0;
    let received: TaskAck = panel
        .client
        .post(&endpoint)
        .bearer_auth(&ack.session_token)
        .json(&ProbeBatch {
            results: vec![sample.clone(), offline.clone()],
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(received.ids, vec![sample.id, offline.id]);
    let preserved: (serde_json::Value, String) =
        sqlx::query_as("SELECT result,digest FROM probe_results WHERE id=$1")
            .bind(sample.id)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(preserved, saved);
    let history: Vec<ProbeResult> = panel
        .admin(
            Method::GET,
            &format!(
                "/api/servers/{server}/probe-results?probe_id={}",
                original.id
            ),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(history, vec![sample]);
    let overview: Vec<serde_json::Value> = panel
        .admin(Method::GET, "/api/probes/overview", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(overview.len(), 1);
    assert_eq!(overview[0]["probe"], serde_json::to_value(&metadata.spec)?);
    assert_eq!(overview[0]["results"], serde_json::to_value(history)?);
    Ok(())
}

#[sqlx::test]
async fn probe_display_is_authenticated_target_bounded_and_preserves_full_day_history(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "quality").await?;
    let (other, _other_socket, _) = panel.authenticated_device(&cookie, "other quality").await?;
    let now = now_millis() - 1000;
    let mut definitions = Vec::new();
    for (owner, name) in [
        (server, "回显"),
        (server, "连接"),
        (server, "历史"),
        (other, "其他设备"),
    ] {
        let spec: ConfiguredProbe = panel.admin(Method::POST, &format!("/api/servers/{owner}/probes"), &cookie,
            Some(probe_support::authorized(json!({"id":Uuid::nil(),"name":name,"kind":"icmp","target":"127.0.0.1","port":null,"interval_secs":10,"carrier":"测试线路","enabled":true}))))
            .await?.error_for_status()?.json().await?;
        definitions.push(spec);
    }
    let endpoint = format!("{}/api/probes/overview", panel.base);
    assert_eq!(
        panel.client.get(&endpoint).send().await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .client
            .get(&endpoint)
            .bearer_auth(&ack.session_token)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let samples: Vec<_> = definitions[..2]
        .iter()
        .flat_map(|probe| {
            (0..30).map(move |index| ProbeResult {
                id: Uuid::new_v4(),
                probe_id: probe.id,
                sampled_at: now - index * 10_000,
                latency_ms: if index == 0 { None } else { Some(0.0) },
                loss_percent: if index == 0 { 100.0 } else { 0.0 },
                address_family: None,
                error: (index == 1).then(|| "ICMP tool unavailable".into()),
                attempts: None,
                execution: None,
            })
        })
        .collect();
    // Seed genuine legacy-shaped fixture history, without inventing an issuance
    // receipt for timestamps predating this test's live session.
    for result in &samples {
        let value = serde_json::to_value(result)?;
        let digest = format!("{:x}", sha2::Sha256::digest(serde_json::to_vec(result)?));
        sqlx::query("INSERT INTO probe_results(id,server_id,probe_id,sampled_at,result,digest) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(result.id).bind(server).bind(result.probe_id).bind(result.sampled_at).bind(value).bind(digest)
            .execute(&panel.state.pool).await?;
    }
    let overview: Vec<serde_json::Value> = panel
        .admin(Method::GET, "/api/probes/overview", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(overview.len(), 4);
    for spec in &definitions[..2] {
        let entry = overview
            .iter()
            .find(|entry| entry["probe"]["id"] == spec.id.to_string())
            .unwrap();
        assert_eq!(entry["server_id"], server);
        let results: Vec<ProbeResult> = serde_json::from_value(entry["results"].clone())?;
        assert_eq!(
            results,
            samples
                .iter()
                .filter(|result| result.probe_id == spec.id)
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(results[0].latency_ms, None);
        assert_eq!(results[0].loss_percent, 100.0);
        assert!(results[1].error.is_some());
    }
    assert_eq!(
        overview
            .iter()
            .find(|entry| entry["server_id"] == other)
            .unwrap()["results"],
        json!([])
    );
    // More than the legacy global limit, inserted as fixture history without network traffic.
    sqlx::query("INSERT INTO probe_results(id,server_id,probe_id,sampled_at,result,digest)
        SELECT id,$1,$2,at,jsonb_build_object('id',id,'probe_id',$2::uuid,'sampled_at',at,'latency_ms',0,'loss_percent',0,'error',null),'TEST_ONLY'
        FROM (SELECT gen_random_uuid() AS id, $3::bigint - n*10000 AS at FROM generate_series(0,4999) n) samples")
        .bind(server).bind(definitions[2].id).bind(now).execute(&panel.state.pool).await?;
    let url = format!(
        "/api/servers/{server}/probe-results?probe_id={}&hours=24",
        definitions[2].id
    );
    let history: Vec<ProbeResult> = panel
        .admin(Method::GET, &url, &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(history.len(), 5000);
    assert_eq!(history.last().unwrap().sampled_at, now - 4999 * 10_000);
    assert!(
        history
            .iter()
            .all(|result| result.probe_id == definitions[2].id)
    );
    let short: Vec<ProbeResult> = panel
        .admin(
            Method::GET,
            &url.replace("hours=24", "hours=1"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(short.len(), 360);
    let scoped: Vec<ProbeResult> = panel
        .admin(
            Method::GET,
            &format!(
                "/api/servers/{other}/probe-results?probe_id={}",
                definitions[2].id
            ),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(scoped.is_empty());
    assert_eq!(
        panel
            .admin(
                Method::GET,
                &url.replace("hours=24", "hours=25"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    sqlx::query("UPDATE servers SET deleted_at=$1 WHERE id=$2")
        .bind(now_timestamp())
        .bind(other)
        .execute(&panel.state.pool)
        .await?;
    let overview: Vec<serde_json::Value> = panel
        .admin(Method::GET, "/api/probes/overview", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(overview.iter().all(|entry| entry["server_id"] != other));
    Ok(())
}
