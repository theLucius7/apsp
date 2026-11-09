#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use business_support::TestPanel;
use reqwest::Method;
use serde_json::Value;
use sqlx::PgPool;

#[sqlx::test]
async fn retiring_chain_entry_is_never_listed_as_a_direct_node(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel
        .create_server(&cookie, "TEST_ONLY namespace guard")
        .await?;
    let node = panel
        .create_node(&cookie, server, "TEST_ONLY reserved ordered entry")
        .await?;
    let id = node["id"].as_i64().unwrap();
    // Model an unresolved, soft-deleted chain. Neither listing may
    // treat pending device cleanup as proof that the dedicated entry is free.
    sqlx::query("INSERT INTO singbox_chains(name,entry_node_id,path_kind,phase,deleted_at) VALUES('TEST_ONLY retained',$1,'ordered','retiring',1)").bind(id).execute(&panel.state.pool).await?;
    for path in ["ordered-proxy-resources", "node-catalog"] {
        let listed: Value = panel
            .admin(
                Method::GET,
                &format!("/api/plugins/sing-box/{path}"),
                &cookie,
                None,
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .all(|row| !(row["kind"] == "direct" && row["id"] == id)),
            "{path} lists the reserved entry as a direct node"
        );
    }
    Ok(())
}
