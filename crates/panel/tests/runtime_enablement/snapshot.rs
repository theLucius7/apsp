use anyhow::Result;
use serde_json::Value;
use sqlx::PgPool;
use std::collections::BTreeMap;

pub(super) async fn current(pool: &PgPool) -> Result<BTreeMap<&'static str, Value>> {
    let mut snapshot = BTreeMap::new();
    for table in [
        "servers",
        "nodes",
        "users",
        "accesses",
        "deployments",
        "server_module_status",
        "usage_batches",
        "usage_records",
        "device_sessions",
        "enrollment_tokens",
    ] {
        // Device credentials are the imported state that must survive the business
        // ownership move; administrator logins keep their sessions elsewhere.
        let value = sqlx::query_scalar(&format!(
            "SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY to_jsonb(r)::text),'[]'::jsonb) FROM {table} r"
        )).fetch_one(pool).await?;
        snapshot.insert(table, value);
    }
    Ok(snapshot)
}
