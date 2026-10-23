use super::*;
use apsp_protocol::{DiagnosticCancelResult, Envelope, now_timestamp};
use business_support::{receive_envelope, send_envelope};
use uuid::Uuid;

async fn wait_for_cancel_error(panel: &TestPanel, id: Uuid) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let error: Option<String> =
                sqlx::query_scalar("SELECT cancel_error FROM diagnostic_jobs WHERE id=$1")
                    .bind(id)
                    .fetch_one(&panel.state.pool)
                    .await?;
            if error.is_some() {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn cancellation_waits_for_device_confirmation_and_preserves_late_reports(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server_id, mut socket, ack) = panel.authenticated_device(&cookie, "确认取消设备").await?;
    let (other_id, _other, other_ack) = panel.authenticated_device(&cookie, "另一设备").await?;
    capable(&panel, server_id).await?;
    fixture(&panel).await?;
    let reports = format!("/api/servers/{server_id}/node-quality/reports");
    let record: Value = panel
        .admin(
            Method::POST,
            &reports,
            &cookie,
            Some(json!({"mode":"daily"})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = Uuid::parse_str(record["id"].as_str().unwrap())?;
    // Model a full task already running before rollout. Its original hardware
    // chapters and late reports must remain available during cancellation.
    sqlx::query("UPDATE diagnostic_jobs SET status='running',job=jsonb_set(job,'{options,mode}','\"full\"'),expected_sections=ARRAY['header_info','hardware_quality','ip_quality','net_quality','backroute_trace','environment'] WHERE id=$1")
        .bind(id).execute(&panel.state.pool).await?;
    let path = format!("/api/servers/{server_id}/diagnostics/{id}/cancel");
    assert_eq!(
        panel
            .client
            .post(format!("{}{path}", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let unsupported = panel.admin(Method::POST, &path, &cookie, None).await?;
    assert_eq!(unsupported.status(), StatusCode::CONFLICT);
    assert!(unsupported.text().await?.contains("不支持确认式取消"));
    sqlx::query("UPDATE servers SET capabilities=capabilities || $2 WHERE id=$1")
        .bind(server_id)
        .bind(json!([apsp_protocol::DIAGNOSTIC_CANCEL_CAPABILITY]))
        .execute(&panel.state.pool)
        .await?;
    let response = panel.admin(Method::POST, &path, &cookie, None).await?;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requested: Value = response.json().await?;
    assert_eq!(requested["status"], "cancel_requested");
    assert!(!requested["agent_completed"].as_bool().unwrap());
    let delivered = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Message::DiagnosticCancelRequest(request) =
                receive_envelope(&mut socket).await?.decode()?
            {
                return Ok::<_, anyhow::Error>(request);
            }
        }
    })
    .await??;
    assert_eq!(delivered.server_id, server_id);
    assert_eq!(delivered.job.id, id);
    assert_eq!(delivered.job.plugin, "nodequality");
    let duplicate: Value = panel
        .admin(Method::POST, &path, &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        duplicate["cancel_requested_at"],
        requested["cancel_requested_at"]
    );
    assert_eq!(duplicate["status"], "cancel_requested");
    let pending: Value = panel
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
    assert_eq!(pending[0]["job"]["id"], id.to_string());
    let foreign: Value = panel
        .client
        .get(format!(
            "{}/api/agent/v1/diagnostics/cancellations",
            panel.base
        ))
        .bearer_auth(&other_ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(foreign, json!([]));
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("/api/servers/{other_id}/diagnostics/{id}/cancel"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let failure = DiagnosticCancelResult {
        server_id,
        id,
        plugin: "nodequality".into(),
        confirmed: false,
        report: None,
        error: Some("仍有进程或挂载".into()),
    };
    send_envelope(
        &mut socket,
        Envelope::new("diagnostic.cancel.result", &failure)?,
    )
    .await?;
    wait_for_cancel_error(&panel, id).await?;
    sqlx::query("UPDATE diagnostic_jobs SET expires_at=$2 WHERE id=$1")
        .bind(id)
        .bind(now_timestamp() - 1)
        .execute(&panel.state.pool)
        .await?;
    diagnostics::expire(&panel.state).await?;
    assert_eq!(
        update(
            &panel,
            &ack,
            &id.to_string(),
            json!({"id":id,"status":"succeeded","report":{"text":"已经产生的报告"}})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    let status: String = sqlx::query_scalar("SELECT status FROM diagnostic_jobs WHERE id=$1")
        .bind(id)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(status, "cancel_requested");
    let sections_endpoint = format!("{}/api/agent/v1/diagnostics/{id}/sections", panel.base);
    let chapter = json!({"id":id,"name":"header_info","text":"取消前保存的章节","complete":true,"revision":1,"collected_at":now_timestamp()});
    assert_eq!(
        panel
            .client
            .post(&sections_endpoint)
            .bearer_auth(&ack.session_token)
            .json(&chapter)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let confirmation = DiagnosticCancelResult {
        confirmed: true,
        error: None,
        report: Some(apsp_protocol::DiagnosticReport {
            text: "较晚的不同片段不得覆盖已有报告".into(),
            report_url: None,
        }),
        ..failure
    };
    let confirm = format!(
        "{}/api/agent/v1/diagnostics/{id}/cancel-confirmation",
        panel.base
    );
    assert_eq!(
        panel
            .client
            .post(&confirm)
            .bearer_auth(&other_ack.session_token)
            .json(&confirmation)
            .send()
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    for _ in 0..2 {
        assert_eq!(
            panel
                .client
                .post(&confirm)
                .bearer_auth(&ack.session_token)
                .json(&confirmation)
                .send()
                .await?
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    let row: (String, Option<Value>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT status,report,cancel_error,cancel_confirmed_at FROM diagnostic_jobs WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(row.0, "cancelled");
    assert_eq!(row.1.unwrap()["text"], "已经产生的报告");
    assert!(row.2.is_none() && row.3.is_some());
    let duplicate: Value = panel
        .admin(Method::POST, &path, &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(duplicate["status"], "cancelled");
    assert_eq!(duplicate["sections"][0]["text"], chapter["text"]);
    assert_eq!(duplicate["report_completeness"], "partial");
    let late_chapter = json!({"id":id,"name":"hardware_quality","text":"迟到的停止前快照","complete":false,"revision":1,"collected_at":now_timestamp()});
    for _ in 0..2 {
        assert_eq!(
            panel
                .client
                .post(&sections_endpoint)
                .bearer_auth(&ack.session_token)
                .json(&late_chapter)
                .send()
                .await?
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(
        update(
            &panel,
            &ack,
            &id.to_string(),
            json!({"id":id,"status":"succeeded","report":{"text":"迟到自然结果不得复活或覆盖"}})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    for view_path in [
        reports.clone(),
        format!("/api/servers/{server_id}/node-quality"),
    ] {
        let view: Value = panel
            .admin(Method::GET, &view_path, &cookie, None)
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(view["cancel_supported"], true);
        assert_eq!(view["reports"][0]["status"], "cancelled");
        assert_eq!(view["reports"][0]["report"]["text"], "已经产生的报告");
        assert_eq!(view["reports"][0]["sections"].as_array().unwrap().len(), 2);
    }

    assert_eq!(
        update(
            &panel,
            &ack,
            &id.to_string(),
            json!({"id":id,"status":"running"})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    let status: String = sqlx::query_scalar("SELECT status FROM diagnostic_jobs WHERE id=$1")
        .bind(id)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(status, "cancelled");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn natural_completion_rejects_cancellation_and_requested_cleanup_blocks_new_jobs(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server_id, _socket, ack) = panel.authenticated_device(&cookie, "取消竞态设备").await?;
    capable(&panel, server_id).await?;
    sqlx::query("UPDATE servers SET capabilities=capabilities || $2 WHERE id=$1")
        .bind(server_id)
        .bind(json!([apsp_protocol::DIAGNOSTIC_CANCEL_CAPABILITY]))
        .execute(&panel.state.pool)
        .await?;
    fixture(&panel).await?;
    let reports = format!("/api/servers/{server_id}/node-quality/reports");
    let first: Value = panel
        .admin(
            Method::POST,
            &reports,
            &cookie,
            Some(json!({"mode":"daily"})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = first["id"].as_str().unwrap();
    assert_eq!(
        update(
            &panel,
            &ack,
            id,
            json!({"id":id,"status":"succeeded","report":{"text":"自然完成"}})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("/api/servers/{server_id}/diagnostics/{id}/cancel"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let second: Value = panel
        .admin(
            Method::POST,
            &reports,
            &cookie,
            Some(json!({"mode":"daily"})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = second["id"].as_str().unwrap();
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("/api/servers/{server_id}/diagnostics/{id}/cancel"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &reports,
                &cookie,
                Some(json!({"mode":"daily"}))
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let unsolicited =
        json!({"server_id":server_id,"id":first["id"],"plugin":"nodequality","confirmed":true});
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/diagnostics/{}/cancel-confirmation",
                panel.base,
                first["id"].as_str().unwrap()
            ))
            .bearer_auth(&ack.session_token)
            .json(&unsolicited)
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    // A task queued before chapter support retains whole-report semantics while
    // waiting for cancellation proof and after that proof is acknowledged.
    let legacy_id = Uuid::parse_str(id)?;
    sqlx::query("UPDATE diagnostic_jobs SET expected_sections='{}' WHERE id=$1")
        .bind(legacy_id)
        .execute(&panel.state.pool)
        .await?;
    assert_eq!(
        update(
            &panel,
            &ack,
            id,
            json!({"id":id,"status":"succeeded","report":{"text":"旧任务迟到的完整报告"}})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    let saved: (String, String) =
        sqlx::query_as("SELECT status,report_completeness FROM diagnostic_jobs WHERE id=$1")
            .bind(legacy_id)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(saved, ("cancel_requested".into(), "legacy".into()));
    let confirmation =
        json!({"server_id":server_id,"id":id,"plugin":"nodequality","confirmed":true});
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/diagnostics/{id}/cancel-confirmation",
                panel.base
            ))
            .bearer_auth(&ack.session_token)
            .json(&confirmation)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let saved: (String, String, Value) =
        sqlx::query_as("SELECT status,report_completeness,report FROM diagnostic_jobs WHERE id=$1")
            .bind(legacy_id)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(
        saved,
        (
            "cancelled".into(),
            "legacy".into(),
            json!({"text":"旧任务迟到的完整报告"})
        )
    );
    Ok(())
}
