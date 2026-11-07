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
async fn retiring_ordered_entry_is_never_reclassified_as_a_flat_direct_resource(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel
        .create_server(&cookie, "TEST_ONLY namespace guard")
        .await?;
    let node = panel
        .create_node(&cookie, server, "TEST_ONLY reserved ordered entry")
        .await?;
    let id = node["id"].as_i64().unwrap();
    // Model an unresolved, soft-deleted ordered lineage. The old API must not
    // treat pending device cleanup as proof that the dedicated entry is free.
    sqlx::query("INSERT INTO singbox_chains(name,entry_node_id,path_kind,phase,deleted_at) VALUES('TEST_ONLY retained',$1,'ordered','retiring',1)").bind(id).execute(&panel.state.pool).await?;
    let flat: Value = panel
        .admin(
            Method::GET,
            "/api/plugins/sing-box/proxy-resources",
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        flat.as_array()
            .unwrap()
            .iter()
            .all(|row| !(row["kind"] == "direct" && row["id"] == id))
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM singbox_live_chains WHERE entry_node_id=$1"
        )
        .bind(id)
        .fetch_one(&panel.state.pool)
        .await?,
        0
    );
    Ok(())
}
