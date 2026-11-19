#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_protocol::{SELF_CONTAINED_AGENT_CAPABILITY, now_timestamp};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

#[sqlx::test]
async fn self_contained_manifest_omits_artifact_descriptors_and_gates_legacy_or_mismatched_agents(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "self-contained-manifest")
        .await?;
    let node = id(&panel.create_node(&cookie, server, "node").await?)?;
    let user = id(&panel.create_user(&cookie, "user").await?)?;
    panel.grant(&cookie, user, node).await?;
    panel.publish_now().await?;

    let self_contained = json!([
        SELF_CONTAINED_AGENT_CAPABILITY,
        "singbox",
        apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY,
    ]);
    sqlx::query("UPDATE servers SET capabilities=$2,last_seen=$3 WHERE id=$1")
        .bind(server)
        .bind(&self_contained)
        .bind(now_timestamp())
        .execute(&panel.state.pool)
        .await?;

    for info in [
        json!({"arch":"amd64","os":"linux","libc":"gnu","components":{"sing-box":apsp_traffic::RUNTIME_VERSION}}),
        json!({"arch":"amd64","os":"linux","libc":"musl","components":{"sing-box":apsp_traffic::RUNTIME_VERSION}}),
        json!({"arch":"amd64","os":"freebsd","components":{"sing-box":apsp_traffic::RUNTIME_VERSION}}),
        json!({"arch":"arm64","os":"macos","components":{"sing-box":apsp_traffic::RUNTIME_VERSION}}),
        json!({"arch":"amd64","os":"windows","components":{"sing-box":apsp_traffic::RUNTIME_VERSION}}),
    ] {
        sqlx::query("UPDATE servers SET static_info=$2 WHERE id=$1")
            .bind(server)
            .bind(&info)
            .execute(&panel.state.pool)
            .await?;
        let response = panel
            .client
            .get(format!("{}/api/agent/v1/manifest", panel.base))
            .bearer_auth(&ack.session_token)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "platform: {info}");
        let manifest: Value = response.json().await?;
        let module = &manifest["modules"]["singbox"];
        assert!(module["artifact"].is_null());
        assert_eq!(module["kernel_version"], apsp_traffic::RUNTIME_VERSION);
        assert_eq!(module["config_rev"], 1);
        assert!(
            module["bundle_url"]
                .as_str()
                .unwrap()
                .ends_with("/api/agent/v1/bundles/1")
        );
    }

    // Panel never serves standalone runtime or Agent binary artifacts.
    assert_eq!(
        panel
            .client
            .get(format!(
                "{}/api/agent/v1/artifacts/sing-box/{}/amd64",
                panel.base,
                apsp_traffic::RUNTIME_VERSION
            ))
            .bearer_auth(&ack.session_token)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );

    // Mismatched embedded sing-box versions or legacy/monitor-only capabilities are refused with 409.
    for (capabilities, info) in [
        (
            json!([
                "singbox",
                apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY
            ]),
            json!({"arch":"amd64","os":"linux"}),
        ),
        (
            json!([
                SELF_CONTAINED_AGENT_CAPABILITY,
                apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY,
            ]),
            json!({"arch":"amd64","os":"linux"}),
        ),
        (
            self_contained,
            json!({"arch":"amd64","os":"linux","components":{"sing-box":"1.14.0-apsp.1"}}),
        ),
    ] {
        sqlx::query("UPDATE servers SET capabilities=$2,static_info=$3 WHERE id=$1")
            .bind(server)
            .bind(capabilities)
            .bind(info)
            .execute(&panel.state.pool)
            .await?;
        let response = panel
            .client
            .get(format!("{}/api/agent/v1/manifest", panel.base))
            .bearer_auth(&ack.session_token)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
    Ok(())
}

#[sqlx::test]
async fn explicit_redeploy_requires_active_configuration_and_reissues_current_bundle(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "redeploy-target").await?;
    let redeploy_path = format!("/api/traffic/servers/{server}/deployments/redeploy");

    assert_eq!(
        panel
            .admin(Method::POST, &redeploy_path, &cookie, None)
            .await?
            .status(),
        StatusCode::CONFLICT
    );

    let node = id(&panel.create_node(&cookie, server, "redeploy-node").await?)?;
    let user = id(&panel.create_user(&cookie, "redeploy-user").await?)?;
    panel.grant(&cookie, user, node).await?;
    panel.publish_now().await?;

    let first_sha: String = sqlx::query_scalar(
        "SELECT bundle_sha256 FROM deployments WHERE server_id=$1 AND module='singbox' AND rev=1",
    )
    .bind(server)
    .fetch_one(&panel.state.pool)
    .await?;

    let redeployed: Value = panel
        .admin(Method::POST, &redeploy_path, &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(redeployed["revision"], 2);

    let second_sha: String = sqlx::query_scalar(
        "SELECT bundle_sha256 FROM deployments WHERE server_id=$1 AND module='singbox' AND rev=2",
    )
    .bind(server)
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(first_sha, second_sha);
    Ok(())
}
