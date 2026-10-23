#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use business_support::{TestPanel, id};
use sqlx::{PgPool, Row};
use std::time::Duration;
use x25519_dalek::{PublicKey, StaticSecret};

fn validate_pair(private: &str, public: &str) -> Result<()> {
    anyhow::ensure!(
        private.len() == 43 && public.len() == 43,
        "key encoding is not unpadded base64url"
    );
    let private_bytes: [u8; 32] = URL_SAFE_NO_PAD
        .decode(private)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("private key is not 32 bytes"))?;
    let public_bytes: [u8; 32] = URL_SAFE_NO_PAD
        .decode(public)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("public key is not 32 bytes"))?;
    anyhow::ensure!(
        URL_SAFE_NO_PAD.encode(private_bytes) == private
            && URL_SAFE_NO_PAD.encode(public_bytes) == public,
        "noncanonical key encoding"
    );
    anyhow::ensure!(
        PublicKey::from(&StaticSecret::from(private_bytes)).as_bytes() == &public_bytes,
        "X25519 public key does not match private key"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "Requires PostgreSQL and APSP_TEST_SINGBOX pointing to an upstream runtime; run --ignored"]
async fn panel_reality_keys_match_upstream_generated_key_format(pool: PgPool) -> Result<()> {
    let binary = std::env::var("APSP_TEST_SINGBOX").context("set APSP_TEST_SINGBOX")?;
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(&binary)
            .args(["generate", "reality-keypair"])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    anyhow::ensure!(output.status.success(), "upstream key generation failed");
    let output = String::from_utf8(output.stdout)?;
    let private = output
        .lines()
        .find_map(|line| line.strip_prefix("PrivateKey: "))
        .context("upstream PrivateKey label")?;
    let public = output
        .lines()
        .find_map(|line| line.strip_prefix("PublicKey: "))
        .context("upstream PublicKey label")?;
    validate_pair(private, public)?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Key format").await?;
    let node = panel.create_node(&cookie, server, "Reality node").await?;
    let row = sqlx::query("SELECT private_key,public_key,short_id FROM nodes WHERE id=$1")
        .bind(id(&node)?)
        .fetch_one(&pool)
        .await?;
    let private: String = row.get("private_key");
    let public: String = row.get("public_key");
    validate_pair(&private, &public)?;
    assert_eq!(node["public_key"], public);
    assert!(node.get("private_key").is_none());
    let short_id: String = row.get("short_id");
    assert_eq!(short_id.len(), 8);
    assert!(short_id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let user = panel.create_user(&cookie, "Reality check user").await?;
    panel.grant(&cookie, id(&user)?, id(&node)?).await?;
    panel.publish_now().await?;
    let bundle: String = sqlx::query_scalar("SELECT bundle FROM deployments WHERE server_id=$1 AND module='singbox' ORDER BY rev DESC LIMIT 1")
        .bind(server).fetch_one(&pool).await?;
    let bundle: apsp_protocol::Bundle = serde_json::from_str(&bundle)?;
    let path = panel.state.config.data_dir.join("reality-check.json");
    tokio::fs::write(
        &path,
        bundle
            .files
            .get("config.json")
            .context("compiled native config")?,
    )
    .await?;
    let check = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(binary)
            .args(["check", "-c"])
            .arg(path)
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    anyhow::ensure!(
        check.status.success(),
        "upstream rejected the published configuration with panel-generated Reality keys"
    );
    Ok(())
}
