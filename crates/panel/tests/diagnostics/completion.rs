use super::*;

async fn create_daily(panel: &TestPanel, cookie: &str, server: i64) -> Result<Value> {
    let response = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/diagnostics/nodequality"),
            cookie,
            Some(json!({"mode":"daily"})),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    Ok(response.json().await?)
}

async fn stored(panel: &TestPanel, id: Uuid) -> Result<Value> {
    Ok(
        sqlx::query_scalar("SELECT to_jsonb(j) FROM diagnostic_jobs j WHERE id=$1")
            .bind(id)
            .fetch_one(&panel.state.pool)
            .await?,
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn cleaning_keeps_mutex_reports_and_chapters_until_terminal_confirmation(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "等待清理设备").await?;
    capable(&panel, server).await?;
    fixture(&panel).await?;
    let record = create_daily(&panel, &cookie, server).await?;
    let id = Uuid::parse_str(record["id"].as_str().unwrap())?;
    sqlx::query("UPDATE diagnostic_jobs SET expires_at=0 WHERE id=$1")
        .bind(id)
        .execute(&panel.state.pool)
        .await?;
    let chapter = json!({"id":id,"name":"net_quality","text":"停止前已经保存的网络章节","complete":false,"revision":1,"collected_at":apsp_protocol::now_timestamp()});
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/diagnostics/{id}/sections",
                panel.base
            ))
            .bearer_auth(&ack.session_token)
            .json(&chapter)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let cleaning = json!({"id":id,"status":"cleaning","error":"原始内存保护原因；挂载尚未清理","report":{"text":"停止时保存的部分报告"}});
    for _ in 0..2 {
        assert_eq!(
            update(&panel, &ack, &id.to_string(), cleaning.clone())
                .await?
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    diagnostics::expire(&panel.state).await?;
    let saved = stored(&panel, id).await?;
    assert_eq!(saved["status"], "cleaning");
    assert_eq!(saved["agent_completed"], false);
    assert_eq!(saved["report"]["text"], "停止时保存的部分报告");
    assert_eq!(saved["report_completeness"], "partial");
    for route in [
        format!("/api/servers/{server}/diagnostics/nodequality"),
        format!("/api/servers/{server}/node-quality/reports"),
    ] {
        assert_eq!(
            panel
                .admin(Method::POST, &route, &cookie, Some(json!({"mode":"daily"})))
                .await?
                .status(),
            StatusCode::CONFLICT
        );
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
    assert_eq!(stored(&panel, id).await?, saved);
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
    for route in [
        format!("/api/servers/{server}/diagnostics"),
        format!("/api/servers/{server}/node-quality"),
    ] {
        let view: Value = panel
            .admin(Method::GET, &route, &cookie, None)
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(view["reports"][0]["status"], "cleaning");
        assert_eq!(view["reports"][0]["report"]["text"], "停止时保存的部分报告");
        assert_eq!(view["reports"][0]["sections"][0]["text"], chapter["text"]);
    }
    let recreated =
        apsp_panel::AppState::new(panel.state.pool.clone(), (*panel.state.config).clone()).await?;
    diagnostics::expire(&recreated).await?;
    assert_eq!(stored(&panel, id).await?, saved);
    assert_eq!(
        update(
            &panel,
            &ack,
            &id.to_string(),
            json!({"id":id,"status":"failed","error":"原始内存保护原因；清理已确认"})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    let finished = stored(&panel, id).await?;
    assert_eq!(finished["status"], "failed");
    assert_eq!(finished["agent_completed"], true);
    assert_eq!(finished["report"], saved["report"]);
    for payload in [
        json!({"id":id,"status":"cleaning","error":"过期的清理消息"}),
        json!({"id":id,"status":"failed","error":"重复消息不得改写原始结果","report":{"text":"另一份报告"}}),
        json!({"id":id,"status":"running"}),
    ] {
        assert_eq!(
            update(&panel, &ack, &id.to_string(), payload)
                .await?
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(stored(&panel, id).await?, finished);
    assert_ne!(
        create_daily(&panel, &cookie, server).await?["id"],
        record["id"]
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn late_cleaning_restores_panel_timeout_without_requiring_new_capability_for_history(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "超时重连设备").await?;
    capable(&panel, server).await?;
    fixture(&panel).await?;
    let record = create_daily(&panel, &cookie, server).await?;
    let id = Uuid::parse_str(record["id"].as_str().unwrap())?;
    sqlx::query("UPDATE diagnostic_jobs SET expires_at=0 WHERE id=$1")
        .bind(id)
        .execute(&panel.state.pool)
        .await?;
    diagnostics::expire(&panel.state).await?;
    let expired = stored(&panel, id).await?;
    assert_eq!(expired["status"], "failed");
    assert_eq!(expired["agent_completed"], false);
    // Create with the current mandatory capability, then explicitly model a
    // historical device that no longer advertises it when reporting this job.
    sqlx::query("UPDATE servers SET capabilities=capabilities-$2 WHERE id=$1")
        .bind(server)
        .bind(apsp_protocol::DIAGNOSTIC_COMPLETION_CAPABILITY)
        .execute(&panel.state.pool)
        .await?;
    let capabilities: Value = sqlx::query_scalar("SELECT capabilities FROM servers WHERE id=$1")
        .bind(server)
        .fetch_one(&panel.state.pool)
        .await?;
    assert!(
        !capabilities
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some(apsp_protocol::DIAGNOSTIC_COMPLETION_CAPABILITY))
    );
    assert_eq!(
        update(
            &panel,
            &ack,
            &id.to_string(),
            json!({"id":id,"status":"cleaning","error":"诊断已停止，等待私有挂载清理"})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    let restored = stored(&panel, id).await?;
    assert_eq!(restored["status"], "cleaning");
    assert_eq!(restored["agent_completed"], false);
    diagnostics::expire(&panel.state).await?;
    assert_eq!(stored(&panel, id).await?, restored);
    assert_eq!(
        update(
            &panel,
            &ack,
            &id.to_string(),
            json!({"id":id,"status":"succeeded","report":{"text":"完整报告与清理均已确认"}})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    let final_record = stored(&panel, id).await?;
    assert_eq!(final_record["status"], "succeeded");
    assert_eq!(final_record["agent_completed"], true);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn late_cleaning_conflict_preserves_evidence_without_replacing_new_task(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "晚到清理互斥设备")
        .await?;
    capable(&panel, server).await?;
    fixture(&panel).await?;
    let first = create_daily(&panel, &cookie, server).await?;
    let first_id = Uuid::parse_str(first["id"].as_str().unwrap())?;
    sqlx::query("UPDATE diagnostic_jobs SET expires_at=0 WHERE id=$1")
        .bind(first_id)
        .execute(&panel.state.pool)
        .await?;
    diagnostics::expire(&panel.state).await?;
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("/api/servers/{server}/diagnostics/nodequality"),
                &cookie,
                Some(json!({"mode":"daily"}))
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    // Preserve recovery for a second queue admitted by the older timeout code.
    let second_id = Uuid::new_v4();
    let mut second_job = first["job"].clone();
    second_job["id"] = json!(second_id);
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,job,created_at,updated_at,expires_at,expected_sections) VALUES($1,$2,$3,$4,$4,$4+3600,ARRAY['header_info'])")
        .bind(second_id).bind(server).bind(second_job).bind(apsp_protocol::now_timestamp())
        .execute(&panel.state.pool).await?;
    let second_before = stored(&panel, second_id).await?;
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
    let payload = json!({"id":first_id,"status":"cleaning","error":"原任务仍有挂载，继续保护同机业务","report":{"text":"断连期间固定的原任务报告"}});
    assert_eq!(
        update(&panel, &ack, &first_id.to_string(), payload)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let first_saved = stored(&panel, first_id).await?;
    assert_eq!(first_saved["status"], "failed");
    assert_eq!(first_saved["agent_completed"], false);
    assert_eq!(first_saved["error"], "原任务仍有挂载，继续保护同机业务");
    assert_eq!(first_saved["report"]["text"], "断连期间固定的原任务报告");
    assert_eq!(stored(&panel, second_id).await?, second_before);
    assert_eq!(
        update(
            &panel,
            &ack,
            &second_id.to_string(),
            json!({"id":second_id,"status":"failed","error":"新任务未运行，等待前一任务清理"})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        update(
            &panel,
            &ack,
            &first_id.to_string(),
            json!({"id":first_id,"status":"cleaning","error":"继续等待清理确认"})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    let restored = stored(&panel, first_id).await?;
    assert_eq!(restored["status"], "cleaning");
    assert_eq!(restored["agent_completed"], false);
    assert_eq!(restored["report"], first_saved["report"]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn cleaning_during_cancellation_never_confirms_cleanup_or_erases_report(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "清理期间取消设备")
        .await?;
    capable(&panel, server).await?;
    sqlx::query("UPDATE servers SET capabilities=capabilities || $2 WHERE id=$1")
        .bind(server)
        .bind(json!([
            apsp_protocol::DIAGNOSTIC_CANCEL_CAPABILITY,
            apsp_protocol::DIAGNOSTIC_COMPLETION_CAPABILITY
        ]))
        .execute(&panel.state.pool)
        .await?;
    fixture(&panel).await?;
    let record = create_daily(&panel, &cookie, server).await?;
    let id = Uuid::parse_str(record["id"].as_str().unwrap())?;
    assert_eq!(
        update(
            &panel,
            &ack,
            &id.to_string(),
            json!({"id":id,"status":"cleaning","report":{"text":"取消前保存的报告"}})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    let cancel_path = format!("/api/servers/{server}/diagnostics/{id}/cancel");
    assert_eq!(
        panel
            .admin(Method::POST, &cancel_path, &cookie, None)
            .await?
            .status(),
        StatusCode::ACCEPTED
    );
    sqlx::query("UPDATE diagnostic_jobs SET expires_at=0 WHERE id=$1")
        .bind(id)
        .execute(&panel.state.pool)
        .await?;
    diagnostics::expire(&panel.state).await?;
    for status in ["cleaning", "running"] {
        assert_eq!(
            update(
                &panel,
                &ack,
                &id.to_string(),
                json!({"id":id,"status":status})
            )
            .await?
            .status(),
            StatusCode::NO_CONTENT
        );
        let pending = stored(&panel, id).await?;
        assert_eq!(pending["status"], "cancel_requested");
        assert_eq!(pending["agent_completed"], false);
        assert_eq!(pending["report"]["text"], "取消前保存的报告");
    }
    let endpoint = format!(
        "{}/api/agent/v1/diagnostics/{id}/cancel-confirmation",
        panel.base
    );
    for confirmed in [false, true] {
        let payload =
            json!({"id":id,"server_id":server,"plugin":"nodequality","confirmed":confirmed});
        assert_eq!(
            panel
                .client
                .post(&endpoint)
                .bearer_auth(&ack.session_token)
                .json(&payload)
                .send()
                .await?
                .status(),
            StatusCode::NO_CONTENT
        );
        let saved = stored(&panel, id).await?;
        assert_eq!(saved["agent_completed"], confirmed);
        assert_eq!(
            saved["status"],
            if confirmed {
                "cancelled"
            } else {
                "cancel_requested"
            }
        );
        assert_eq!(saved["report"]["text"], "取消前保存的报告");
    }
    let cancelled = stored(&panel, id).await?;
    assert_eq!(
        update(
            &panel,
            &ack,
            &id.to_string(),
            json!({"id":id,"status":"cleaning","error":"取消确认前排队的旧消息"})
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(stored(&panel, id).await?, cancelled);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn cleaning_reports_keep_device_plugin_and_size_validation(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "清理报告验证设备")
        .await?;
    let (_other, _other_socket, other_ack) =
        panel.authenticated_device(&cookie, "另一清理设备").await?;
    capable(&panel, server).await?;
    fixture(&panel).await?;
    let record = create_daily(&panel, &cookie, server).await?;
    let id = record["id"].as_str().unwrap();
    assert_eq!(
        update(&panel, &other_ack, id, json!({"id":id,"status":"cleaning"}))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    for report in [
        json!({"text":"   "}),
        json!({"text":"a".repeat(diagnostics::REPORT_LIMIT + 1)}),
        json!({"text":"已有输出","report_url":"https://untrusted.example.invalid/report"}),
    ] {
        assert_eq!(
            update(
                &panel,
                &ack,
                id,
                json!({"id":id,"status":"cleaning","report":report})
            )
            .await?
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let original = stored(&panel, Uuid::parse_str(id)?).await?;
    assert_eq!(original["status"], "queued");
    assert_eq!(original["agent_completed"], false);
    assert_eq!(original["report"], Value::Null);
    assert_eq!(update(&panel, &ack, id, json!({"id":id,"status":"cleaning","error":"x".repeat(5000),"report":{"text":"已验证的文本"}})).await?.status(), StatusCode::NO_CONTENT);
    let saved = stored(&panel, Uuid::parse_str(id)?).await?;
    assert_eq!(saved["error"].as_str().unwrap().chars().count(), 4096);
    assert_eq!(saved["report"]["text"], "已验证的文本");
    assert_eq!(saved["agent_completed"], false);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_late_cleaning_and_creation_share_server_serialization(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    fixture(&panel).await?;
    for round in 0..4 {
        let (server, _socket, ack) = panel
            .authenticated_device(&cookie, &format!("清理创建竞态 {round}"))
            .await?;
        capable(&panel, server).await?;
        let old = create_daily(&panel, &cookie, server).await?;
        let id = Uuid::parse_str(old["id"].as_str().unwrap())?;
        sqlx::query("UPDATE diagnostic_jobs SET expires_at=0 WHERE id=$1")
            .bind(id)
            .execute(&panel.state.pool)
            .await?;
        diagnostics::expire(&panel.state).await?;
        let route = if round % 2 == 0 {
            format!("/api/servers/{server}/diagnostics/nodequality")
        } else {
            format!("/api/servers/{server}/node-quality/reports")
        };
        let id_text = id.to_string();
        let payload = json!({"id":id,"status":"cleaning","error":"竞态中的原清理原因","report":{"text":"原任务保留的报告"}});
        let (cleaning, creation) = tokio::join!(
            update(&panel, &ack, &id_text, payload),
            panel.admin(Method::POST, &route, &cookie, Some(json!({"mode":"daily"}))),
        );
        let cleaning = cleaning?;
        let creation = creation?;
        let saved = stored(&panel, id).await?;
        assert_eq!(creation.status(), StatusCode::CONFLICT);
        assert_eq!(cleaning.status(), StatusCode::NO_CONTENT);
        assert_eq!(saved["status"], "cleaning");
        assert_eq!(saved["agent_completed"], false);
        assert_eq!(saved["report"]["text"], "原任务保留的报告");
        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM diagnostic_jobs WHERE server_id=$1 AND status IN ('queued','running','cleaning','cancel_requested')").bind(server).fetch_one(&panel.state.pool).await?;
        assert_eq!(active, 1);
    }
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn panel_timeout_retains_ownership_until_legacy_shaped_task_is_confirmed_cancelled(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, device) = panel
        .authenticated_device(&cookie, "超时未启动任务清理")
        .await?;
    capable(&panel, server).await?;
    fixture(&panel).await?;
    let record = create_daily(&panel, &cookie, server).await?;
    let id = Uuid::parse_str(record["id"].as_str().unwrap())?;
    // Before plugin registration, a fully shaped device task omitted plugin.
    // Its queued panel state cannot establish whether a device had started it.
    sqlx::query("UPDATE diagnostic_jobs SET job=job-'plugin',expires_at=0 WHERE id=$1")
        .bind(id)
        .execute(&panel.state.pool)
        .await?;
    diagnostics::expire(&panel.state).await?;
    let expired = stored(&panel, id).await?;
    assert_eq!(expired["status"], "failed");
    assert_eq!(expired["agent_completed"], false);
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
    assert_eq!(view["reports"][0]["cleanup_pending"], true);
    for route in [
        format!("/api/servers/{server}/diagnostics/nodequality"),
        format!("/api/servers/{server}/node-quality/reports"),
    ] {
        assert_eq!(
            panel
                .admin(Method::POST, &route, &cookie, Some(json!({"mode":"daily"})))
                .await?
                .status(),
            StatusCode::CONFLICT
        );
    }
    sqlx::query("UPDATE servers SET capabilities=capabilities || $2 WHERE id=$1")
        .bind(server)
        .bind(json!([apsp_protocol::DIAGNOSTIC_CANCEL_CAPABILITY]))
        .execute(&panel.state.pool)
        .await?;
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("/api/servers/{server}/diagnostics/{id}/cancel"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::ACCEPTED
    );
    let pending: Value = panel
        .client
        .get(format!(
            "{}/api/agent/v1/diagnostics/cancellations",
            panel.base
        ))
        .bearer_auth(&device.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending[0]["job"]["plugin"], "nodequality");
    assert_eq!(pending[0]["job"]["id"], json!(id));
    assert_eq!(stored(&panel, id).await?["job"], expired["job"]);
    let endpoint = format!(
        "{}/api/agent/v1/diagnostics/{id}/cancel-confirmation",
        panel.base
    );
    for confirmed in [false, true] {
        assert_eq!(panel.client.post(&endpoint).bearer_auth(&device.session_token)
            .json(&json!({"id":id,"server_id":server,"plugin":"nodequality","confirmed":confirmed}))
            .send().await?.status(), StatusCode::NO_CONTENT);
        let saved = stored(&panel, id).await?;
        assert_eq!(saved["agent_completed"], confirmed);
        assert_eq!(saved["job"], expired["job"]);
        if !confirmed {
            assert_eq!(
                panel
                    .admin(
                        Method::POST,
                        &format!("/api/servers/{server}/diagnostics/nodequality"),
                        &cookie,
                        Some(json!({"mode":"daily"}))
                    )
                    .await?
                    .status(),
                StatusCode::CONFLICT
            );
        }
    }
    assert_ne!(
        create_daily(&panel, &cookie, server).await?["id"],
        record["id"]
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn unresolved_corrupt_tasks_remain_visible_and_block_starts_beyond_latest_history(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, device) = panel
        .authenticated_device(&cookie, "历史之外的未确认任务")
        .await?;
    capable(&panel, server).await?;
    fixture(&panel).await?;
    let first = create_daily(&panel, &cookie, server).await?;
    let id = Uuid::parse_str(first["id"].as_str().unwrap())?;
    sqlx::query("UPDATE diagnostic_jobs SET status='failed',error='TEST_ONLY old panel timeout',created_at=1,expires_at=0 WHERE id=$1")
        .bind(id).execute(&panel.state.pool).await?;
    for sequence in 0..12_i64 {
        sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,job,status,report,created_at,updated_at,expires_at,agent_completed) VALUES($1,$2,'{\"legacy_note\":\"preserved\"}','succeeded','{\"text\":\"newer historical report\"}',$3,$3,$3,TRUE)")
            .bind(Uuid::new_v4()).bind(server).bind(sequence+2).execute(&panel.state.pool).await?;
    }
    sqlx::query("UPDATE servers SET capabilities=capabilities || $2 WHERE id=$1")
        .bind(server)
        .bind(json!([apsp_protocol::DIAGNOSTIC_CANCEL_CAPABILITY]))
        .execute(&panel.state.pool)
        .await?;
    for damaged in [
        Value::Null,
        json!(Uuid::new_v4()),
        json!("damaged-task-identity"),
        json!(id.to_string().to_uppercase()),
    ] {
        sqlx::query("UPDATE diagnostic_jobs SET job=jsonb_set(job,'{id}',$2) WHERE id=$1")
            .bind(id)
            .bind(&damaged)
            .execute(&panel.state.pool)
            .await?;
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
        assert_eq!(view["reports"].as_array().unwrap().len(), 10);
        assert_eq!(view["reports"][0]["id"], json!(id));
        assert_eq!(view["reports"][0]["cleanup_pending"], true);
        assert_eq!(
            panel
                .admin(
                    Method::POST,
                    &format!("/api/servers/{server}/diagnostics/nodequality"),
                    &cookie,
                    Some(json!({"mode":"daily"}))
                )
                .await?
                .status(),
            StatusCode::CONFLICT
        );
        let queue: Value = panel
            .client
            .get(format!("{}/api/agent/v1/diagnostics", panel.base))
            .bearer_auth(&device.session_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(queue, json!([]));
        if damaged != json!(id.to_string().to_uppercase()) {
            assert_eq!(
                panel
                    .admin(
                        Method::POST,
                        &format!("/api/servers/{server}/diagnostics/{id}/cancel"),
                        &cookie,
                        None
                    )
                    .await?
                    .status(),
                StatusCode::CONFLICT
            );
        }
        assert_eq!(stored(&panel, id).await?["job"]["id"], damaged);
    }
    let (historical, _other_socket, _other_device) = panel
        .authenticated_device(&cookie, "纯文本历史设备")
        .await?;
    capable(&panel, historical).await?;
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,job,status,report,created_at,updated_at,expires_at) VALUES($1,$2,'{\"options\":{\"network_mode\":\"low\"}}','failed','{\"text\":\"plain legacy text without a device task\"}',1,1,1)")
        .bind(Uuid::new_v4()).bind(historical).execute(&panel.state.pool).await?;
    assert!(create_daily(&panel, &cookie, historical).await?["id"].is_string());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM diagnostic_jobs WHERE server_id=$1")
            .bind(server)
            .fetch_one(&panel.state.pool)
            .await?,
        13
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn completion_migration_after_published_schema_preserves_history_and_adds_mutex(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "清理状态迁移设备").await?;
    let historical_id = Uuid::new_v4();
    let historical_job =
        json!({"options":{"network_mode":"low"},"legacy_marker":"unchanged-history"});
    let historical_report = json!({"text":"迁移前的完整报告","report_url":null});
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,job,status,report,error,created_at,updated_at,expires_at,agent_completed,expected_sections,report_completeness) VALUES($1,$2,$3,'succeeded',$4,NULL,1,2,3,TRUE,ARRAY['header_info'],'complete')").bind(historical_id).bind(server).bind(&historical_job).bind(&historical_report).execute(&pool).await?;
    sqlx::query("INSERT INTO diagnostic_report_sections(job_id,name,text,complete,revision,collected_at,received_at) VALUES($1,'header_info','迁移前的独立章节',TRUE,2,1,2)").bind(historical_id).execute(&pool).await?;
    let before = stored(&panel, historical_id).await?;
    let running = Uuid::new_v4();
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,job,status,created_at,updated_at,expires_at) VALUES($1,$2,'{}','running',1,1,9999999999)").bind(running).bind(server).execute(&pool).await?;
    // Recreate the published 0033 diagnostic schema, then apply only the new
    // migration while retaining the exact history row and section contents.
    sqlx::raw_sql("ALTER TABLE diagnostic_jobs DROP CONSTRAINT diagnostic_jobs_status_check; ALTER TABLE diagnostic_jobs ADD CONSTRAINT diagnostic_jobs_status_check CHECK(status IN ('queued','running','cancel_requested','cancelled','succeeded','failed')); DROP INDEX diagnostic_active_server_idx; CREATE UNIQUE INDEX diagnostic_active_server_idx ON diagnostic_jobs(server_id) WHERE status IN ('queued','running'); DELETE FROM _sqlx_migrations WHERE version=34;").execute(&pool).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    assert_eq!(stored(&panel, historical_id).await?, before);
    let chapter: (String, bool, i64, i64, i64) = sqlx::query_as("SELECT text,complete,revision,collected_at,received_at FROM diagnostic_report_sections WHERE job_id=$1").bind(historical_id).fetch_one(&pool).await?;
    assert_eq!(chapter, ("迁移前的独立章节".into(), true, 2, 1, 2));
    sqlx::query("UPDATE diagnostic_jobs SET status='cleaning' WHERE id=$1")
        .bind(running)
        .execute(&pool)
        .await?;
    let conflict = sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,job,status,created_at,updated_at,expires_at) VALUES($1,$2,'{}','queued',1,1,9999999999)").bind(Uuid::new_v4()).bind(server).execute(&pool).await.unwrap_err();
    assert_eq!(
        conflict
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23505")
    );
    let index: String = sqlx::query_scalar("SELECT indexdef FROM pg_indexes WHERE schemaname='public' AND indexname='diagnostic_active_server_idx'").fetch_one(&pool).await?;
    assert!(index.contains("cleaning"));
    let applied: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE version IN (33,34) AND success ORDER BY version").fetch_all(&pool).await?;
    assert_eq!(applied, vec![33, 34]);
    Ok(())
}
