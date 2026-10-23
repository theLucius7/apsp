#![forbid(unsafe_code)]

mod business_support;
#[path = "singbox_groups/chains.rs"]
mod chains;
#[path = "singbox_groups/modern.rs"]
mod modern;
#[path = "singbox_groups/packages.rs"]
mod packages;
#[path = "singbox_groups/policies.rs"]
mod policies;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
#[path = "singbox_groups/scheduling.rs"]
mod scheduling;

use anyhow::{Context, Result, ensure};
use apsp_panel::plugins::singbox::entitlements;
use apsp_protocol::{Bundle, UsageBatch, UsageRecord, now_timestamp};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const ROOT: &str = "/api/plugins/sing-box";

async fn call(
    panel: &TestPanel,
    cookie: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    let response = panel
        .admin(method, &format!("{ROOT}{path}"), cookie, body)
        .await?;
    let status = response.status();
    ensure!(
        status.is_success(),
        "{path}: {status}: {}",
        response.text().await?
    );
    if status == StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    Ok(response.json().await?)
}

async fn create_policy(
    panel: &TestPanel,
    cookie: &str,
    nodes: &[i64],
    chains: &[i64],
) -> Result<i64> {
    id(&call(
        panel,
        cookie,
        Method::POST,
        "/policy-groups",
        Some(json!({"name":"Policy","node_ids":nodes,"chain_ids":chains})),
    )
    .await?)
}
async fn set_policies(panel: &TestPanel, cookie: &str, user: i64, groups: &[i64]) -> Result<()> {
    call(
        panel,
        cookie,
        Method::PUT,
        &format!("/users/{user}/policy-groups"),
        Some(json!({"group_ids":groups})),
    )
    .await?;
    Ok(())
}
fn plan_body(quota: Option<&str>) -> Value {
    json!({"name":"Monthly plan","monthly_bytes":quota,"reset_day":1,"reset_hour":0,"reset_minute":0,"timezone":"UTC","duration_days":365})
}
async fn create_plan(panel: &TestPanel, cookie: &str, quota: Option<&str>) -> Result<i64> {
    id(&call(
        panel,
        cookie,
        Method::POST,
        "/package-groups",
        Some(plan_body(quota)),
    )
    .await?)
}
async fn assign(
    panel: &TestPanel,
    cookie: &str,
    user: i64,
    plan: i64,
    request: Uuid,
) -> Result<Value> {
    call(
        panel,
        cookie,
        Method::POST,
        &format!("/users/{user}/package"),
        Some(json!({"package_group_id":plan,"request_id":request})),
    )
    .await
}
async fn entitlement(panel: &TestPanel, cookie: &str, user: i64) -> Result<Value> {
    call(
        panel,
        cookie,
        Method::GET,
        &format!("/users/{user}/entitlement"),
        None,
    )
    .await
}
async fn eligible(pool: &PgPool, user: i64, at: i64) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT node_id FROM singbox_eligible_accesses($1) WHERE user_id=$2 ORDER BY node_id",
    )
    .bind(at)
    .bind(user)
    .fetch_all(pool)
    .await?)
}
async fn latest_config(pool: &PgPool, server: i64) -> Result<Value> {
    let bundle: String = sqlx::query_scalar("SELECT bundle FROM deployments WHERE server_id=$1 AND module='singbox' ORDER BY rev DESC LIMIT 1")
        .bind(server).fetch_one(pool).await?;
    let bundle: Bundle = serde_json::from_str(&bundle)?;
    Ok(serde_json::from_str(
        bundle.files.get("config.json").context("native config")?,
    )?)
}
async fn applied(pool: &PgPool) -> Result<()> {
    sqlx::query("UPDATE server_module_status SET applied_rev=target_rev,healthy=TRUE WHERE module='singbox'").execute(pool).await?;
    Ok(())
}
fn usage(user: i64, node: i64, end: i64, up: u64, down: u64) -> UsageBatch {
    UsageBatch {
        epoch: Uuid::new_v4(),
        seq: 1,
        period_start: end - 1,
        period_end: end,
        records: vec![UsageRecord {
            stat_name: format!("u{user}_n{node}"),
            uplink: up,
            downlink: down,
        }],
    }
}
async fn stamp(pool: &PgPool, date: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT EXTRACT(EPOCH FROM $1::text::timestamptz)::bigint")
            .bind(date)
            .fetch_one(pool)
            .await?,
    )
}
