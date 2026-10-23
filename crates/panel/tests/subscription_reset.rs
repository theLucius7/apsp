#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_panel::{agent_api, usage};
use apsp_protocol::{ApplyResult, ApplyStatus, UsageBatch, UsageRecord};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

async fn unaffected_state(pool: &PgPool) -> Result<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('users',(SELECT jsonb_agg(to_jsonb(u)-'subscription_token' ORDER BY id) FROM users u),'accesses',(SELECT jsonb_agg(to_jsonb(a) ORDER BY user_id,node_id) FROM accesses a),'servers',(SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM servers s),'deployments',(SELECT jsonb_agg(to_jsonb(d) ORDER BY server_id,module,rev) FROM deployments d),'status',(SELECT jsonb_agg(to_jsonb(s) ORDER BY server_id,module) FROM server_module_status s),'batches',(SELECT jsonb_agg(to_jsonb(b) ORDER BY server_id,epoch,seq) FROM usage_batches b),'records',(SELECT jsonb_agg(to_jsonb(r) ORDER BY server_id,epoch,seq,stat_name) FROM usage_records r))")
    .fetch_one(pool).await?)
}

#[sqlx::test(migrations = "./migrations")]
async fn reset_immediately_revokes_old_links_and_preserves_accesses_usage_and_config(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Subscription").await?;
    let node = id(&panel.create_node(&cookie, server, "Node").await?)?;
    let user = panel.create_user(&cookie, "Member").await?;
    let user_id = id(&user)?;
    panel.grant(&cookie, user_id, node).await?;
    panel.publish_now().await?;
    agent_api::record_apply_result(
        &panel.state,
        server,
        ApplyResult {
            module: "singbox".into(),
            rev: 1,
            op_id: Uuid::new_v4(),
            status: ApplyStatus::Applied,
            healthy: true,
            error: None,
        },
    )
    .await?;
    usage::ingest(
        &panel.state,
        server,
        UsageBatch {
            epoch: Uuid::new_v4(),
            seq: 1,
            period_start: 100,
            period_end: 130,
            records: vec![UsageRecord {
                stat_name: format!("u{user_id}_n{node}"),
                uplink: 23,
                downlink: 42,
            }],
        },
    )
    .await?;
    let path = format!("/api/plugins/sing-box/users/{user_id}/subscription/reset");
    assert_eq!(
        panel
            .client
            .post(format!("{}{path}", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let before = unaffected_state(&pool).await?;
    let mut old = user["subscription_token"].as_str().unwrap().to_owned();
    let contents = panel
        .client
        .get(format!("{}/sub/{old}", panel.base))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(!contents.is_empty());
    for _ in 0..3 {
        let reset: Value = panel
            .admin(Method::POST, &path, &cookie, None)
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(reset["id"], user_id);
        assert_eq!(reset["name"], "Member");
        let new = reset["subscription_token"].as_str().unwrap();
        assert_ne!(new, old);
        assert_eq!(URL_SAFE_NO_PAD.decode(new)?.len(), 32);
        assert_eq!(
            reset["subscription_url"],
            format!("{}/sub/{new}", panel.base)
        );
        for format in ["links", "singbox"] {
            assert_eq!(
                panel
                    .client
                    .get(format!("{}/sub/{old}?format={format}", panel.base))
                    .send()
                    .await?
                    .status(),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                panel
                    .client
                    .get(format!("{}/sub/{new}?format={format}", panel.base))
                    .send()
                    .await?
                    .status(),
                StatusCode::OK
            );
        }
        assert_eq!(
            panel
                .client
                .get(format!("{}/sub/{new}", panel.base))
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?,
            contents
        );
        assert_eq!(unaffected_state(&pool).await?, before);
        old = new.to_owned();
    }
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn reset_is_atomic_and_deleted_or_missing_users_cannot_reset(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = panel.create_user(&cookie, "Concurrent").await?;
    let id = id(&user)?;
    let path = format!("/api/plugins/sing-box/users/{id}/subscription/reset");
    let (a, b) = tokio::join!(
        panel.admin(Method::POST, &path, &cookie, None),
        panel.admin(Method::POST, &path, &cookie, None)
    );
    let a: Value = a?.error_for_status()?.json().await?;
    let b: Value = b?.error_for_status()?.json().await?;
    assert_ne!(a["subscription_token"], b["subscription_token"]);
    let current: String = sqlx::query_scalar("SELECT subscription_token FROM users WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await?;
    for value in [&a, &b] {
        let token = value["subscription_token"].as_str().unwrap();
        assert_eq!(
            panel
                .client
                .get(format!("{}/sub/{token}", panel.base))
                .send()
                .await?
                .status(),
            if token == current {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            }
        );
    }
    panel
        .admin(
            Method::DELETE,
            &format!("/api/plugins/sing-box/users/{id}"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?;
    assert_eq!(
        panel
            .admin(Method::POST, &path, &cookie, None)
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        panel
            .admin(
                Method::POST,
                "/api/plugins/sing-box/users/999999/subscription/reset",
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let unchanged: String = sqlx::query_scalar("SELECT subscription_token FROM users WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(unchanged, current);
    Ok(())
}
