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
        "sessions",
        "enrollment_tokens",
    ] {
        // Administrator login creates its own session; device credentials are
        // the imported state that must survive the business ownership move.
        let filter = if table == "sessions" {
            " WHERE server_id IS NOT NULL"
        } else {
            ""
        };
        let value = sqlx::query_scalar(&format!(
            "SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY to_jsonb(r)::text),'[]'::jsonb) FROM {table} r{filter}"
        )).fetch_one(pool).await?;
        snapshot.insert(table, value);
    }
    Ok(snapshot)
}
