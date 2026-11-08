//! The local control interface of each server's sing-box runtime.
//!
//! Every managed configuration exposes the Clash API on loopback with a per-server secret, so
//! the Agent can list and close authenticated sessions (ADR 0129). A configuration that already
//! has a controller, such as one for ordered-path validation or a taken-over host, keeps it.

use apsp_compiler::preserved::SESSION_CONTROL_LISTEN;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};

/// Create the server's control secret once; later calls keep it. Call this outside the
/// rolled-back savepoints of previews, so that repeated previews compile the same secret.
pub(crate) async fn ensure(
    tx: &mut Transaction<'_, Postgres>,
    server_id: i64,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO singbox_runtime_controls(server_id,control_secret) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(server_id)
        .bind(crate::auth::random_token())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Add the loopback controller to a compiled configuration that has none. A configuration that
/// already has one is returned byte for byte, so its digest and deployed revision stay the same.
pub(crate) async fn attach(
    tx: &mut Transaction<'_, Postgres>,
    config: &str,
    server_id: i64,
) -> anyhow::Result<String> {
    ensure(tx, server_id).await?;
    let mut value: Value = serde_json::from_str(config)?;
    if value.pointer("/experimental/clash_api").is_some() {
        return Ok(config.to_owned());
    }
    let secret: String = sqlx::query_scalar(
        "SELECT control_secret FROM singbox_runtime_controls WHERE server_id=$1",
    )
    .bind(server_id)
    .fetch_one(&mut **tx)
    .await?;
    value["experimental"]["clash_api"] = json!({"external_controller":SESSION_CONTROL_LISTEN,"secret":secret,"access_control_allow_origin":[]});
    // The compiler ends every configuration with a newline.
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "../../../crates/panel/migrations")]
    async fn controller_is_added_once_with_a_stable_secret(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<()> {
        let mut tx = pool.begin().await?;
        let host: i64 = sqlx::query_scalar(
            "INSERT INTO servers(name) VALUES('TEST_ONLY control') RETURNING id",
        )
        .fetch_one(&mut *tx)
        .await?;
        let plain = json!({"outbounds":[{"type":"direct","tag":"direct"}]}).to_string();
        let first = attach(&mut tx, &plain, host).await?;
        assert_eq!(attach(&mut tx, &plain, host).await?, first);
        assert!(first.ends_with("}\n"));
        let parsed: Value = serde_json::from_str(&first)?;
        let controller = &parsed["experimental"]["clash_api"];
        assert_eq!(controller["external_controller"], SESSION_CONTROL_LISTEN);
        assert!(controller["secret"].as_str().is_some_and(|s| s.len() >= 32));
        // An existing controller (ordered paths, taken-over hosts) is kept byte for byte.
        let existing = "{\n  \"experimental\": {\"clash_api\": {\"external_controller\": \"127.0.0.1:18086\", \"secret\": \"TEST_ONLY path\"}}\n}\n";
        assert_eq!(attach(&mut tx, existing, host).await?, existing);
        tx.rollback().await?;
        Ok(())
    }
}
