//! Business-owned descriptions consumed by the panel host through plugin hooks.

use std::collections::BTreeSet;

use apsp_panel_host::{
    AppState,
    error::ApiResult,
    plugin_api::{RuntimeStepPolicy, SearchSource},
};
use serde_json::{Value, json};

pub fn sensitive_key(key: &str) -> bool {
    key.contains("subscription")
}

pub fn worker_names() -> &'static [&'static str] {
    &["sing-box"]
}

pub fn runtime_step_policy(kind: &str) -> Option<RuntimeStepPolicy> {
    (kind == "singbox_retry_deployment").then_some(RuntimeStepPolicy {
        version: "1.14.2",
        capability: "proxy:write",
    })
}

pub async fn audit_snapshot(state: &AppState, path: &str) -> ApiResult<Option<Value>> {
    let Some(id) = path
        .strip_prefix("/api/plugins/sing-box/nodes/")
        .filter(|value| !value.contains('/'))
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return Ok(None);
    };
    let snapshot: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(n) FROM nodes n WHERE id=$1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    Ok(Some(snapshot.unwrap_or(Value::Null)))
}

pub fn signed_tool_packages(entries: &[apsp_panel_host::artifacts::ArtifactEntry]) -> Vec<Value> {
    let versions: BTreeSet<_> = entries
        .iter()
        .filter(|entry| entry.name == "sing-box")
        .map(|entry| entry.version.clone())
        .collect();
    versions
        .into_iter()
        .map(|version| {
            json!({"name":"github.com/sagernet/sing-box",
                "version":version,"ecosystem":"Go","scope":"available-signed-artifact"})
        })
        .collect()
}

pub fn search_sources() -> Vec<SearchSource> {
    vec![
        SearchSource {
            category: "node",
            capability: "proxy:read",
            global_only: true,
            extra_ctes: "",
            select: r#"
 SELECT jsonb_build_object('kind','node','id',n.id,'label',n.name||' · '||n.id,
   'path','/plugins/sing-box/nodes/direct/'||n.id,'server_id',n.server_id,'sampled_at',NULL)
 FROM nodes n CROSS JOIN visibility v WHERE v.unrestricted AND n.deleted_at IS NULL
   AND (n.name ILIKE $1 OR n.public_host ILIKE $1 OR n.sni ILIKE $1 OR n.id::text ILIKE $1)
 ORDER BY n.name,n.id"#,
        },
        SearchSource {
            category: "proxy-user",
            capability: "proxy:read",
            global_only: true,
            extra_ctes: "",
            select: r#"
 SELECT jsonb_build_object('kind','proxy-user','id',u.id,'label',u.name||' · '||u.id,
   'path','/plugins/sing-box/users','sampled_at',NULL)
 FROM users u CROSS JOIN visibility v WHERE v.unrestricted AND u.deleted_at IS NULL
   AND (u.name ILIKE $1 OR u.id::text ILIKE $1) ORDER BY u.name,u.id"#,
        },
    ]
}
