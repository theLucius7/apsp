//! The cloud pages' combined views. The cloud page shows accounts and resources with their
//! bills and power settings, bandwidth operations, power jobs, address rotations and
//! notifications; the operations page lists the cloud resources with their server links,
//! bills and balance. Each object's keys come from the owners that keep them (resources,
//! costs, until step 7 of ADR 0137 the power columns on resources' rows), so the views
//! belong to the application (ADR 0128).

use apsp_costs::cloud::accounts as costs;
use apsp_governance::{auth, control_center};
use apsp_observability::cloud_notices;
use apsp_operations::cloud::{bandwidth, eip_rotation, power};
use apsp_panel_host::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use apsp_resources::{cloud::inventory, cloud_resources};
use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/resources/alicloud", get(list))
        .route("/api/operations/cloud", get(resources))
}

fn value(item: impl Serialize) -> ApiResult<Value> {
    serde_json::to_value(item).map_err(|error| ApiError::Internal(error.into()))
}

/// One object's view with the keys another owner keeps of it added; no key is kept twice.
fn merged(view: Value, part: Value) -> Value {
    match (view, part) {
        (Value::Object(mut view), Value::Object(part)) => {
            view.extend(part);
            Value::Object(view)
        }
        (view, _) => view,
    }
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    let (accounts, resources) = inventory::listed(&state.pool).await?;
    let account_ids: Vec<Uuid> = accounts.iter().map(|account| account.id).collect();
    let resource_ids: Vec<Uuid> = resources.iter().map(|resource| resource.id).collect();
    let (mut account_costs, mut resource_costs) =
        costs::listed(&state.pool, &account_ids, &resource_ids).await?;
    let accounts = accounts
        .iter()
        .map(|account| {
            let billing = account_costs.remove(&account.id).unwrap_or_default();
            Ok(merged(value(account)?, value(billing)?))
        })
        .collect::<ApiResult<Vec<Value>>>()?;
    let resources = resources
        .iter()
        .map(|resource| {
            let billing = resource_costs.remove(&resource.id).unwrap_or_default();
            Ok(merged(value(resource)?, value(billing)?))
        })
        .collect::<ApiResult<Vec<Value>>>()?;
    let operations = bandwidth::recent(&state.pool).await?;
    let power_jobs = power::recent(&state.pool).await?;
    let events = cloud_notices::recent(&state.pool).await?;
    let eip_rotations = eip_rotation::recent(&state.pool).await?;
    Ok(Json(
        json!({"accounts":accounts,"resources":resources,"operations":operations,"power_jobs":power_jobs,"eip_rotations":eip_rotations,"events":events}),
    ))
}

/// The operations page's cloud resources the reader may see: all of them with global
/// scope (with the account balance), otherwise those linked to the reader's servers.
async fn resources(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let actor = control_center::authenticate(&state, &headers).await?;
    if !actor.allows("cloud:read") {
        return Err(ApiError::Forbidden("当前账号没有云资源读取权限".into()));
    }
    let servers: Vec<i64> = actor
        .server_ids
        .iter()
        .copied()
        .filter(|id| actor.allows_server(*id))
        .collect();
    let token_servers = actor.token_servers.clone();
    let server_filter = if actor.all_servers {
        token_servers.unwrap_or(servers)
    } else {
        servers
    };
    let global = actor.global_servers();
    let listed = cloud_resources::listed(&state.pool, global, &server_filter).await?;
    let pairs: Vec<(Uuid, Uuid)> = listed
        .iter()
        .map(|resource| (resource.id, resource.account_id))
        .collect();
    let mut bills = costs::resource_costs(&state.pool, now_timestamp(), global, &pairs).await?;
    let values: Vec<Value> = listed
        .into_iter()
        .map(|resource| {
            let part = bills.remove(&resource.id).unwrap_or_else(|| json!({}));
            merged(resource.view, part)
        })
        .collect();
    Ok(Json(
        json!({"resources":values,"providers":[{"id":"alicloud","supported_resources":["ecs_fixed_public_ip","independent_pay_as_you_go_eip"],"official_interfaces":["DescribeInstances","DescribeEipAddresses","QueryAccountBalance","DescribeInstanceBill","QueryInstanceBill","DescribeSecurityGroupAttribute","JoinSecurityGroup","LeaveSecurityGroup"],"operations":"使用阿里云插件预览与未知结果对账流程","security_group_membership":{"available":true,"resource_kind":"registered_ecs","group_kind":"normal","baseline_preserved":true,"rules_edit_available":false,"fee":"unknown"},"billing_note":"账单和余额为带查询时间的缓存，不能作为实时硬限额；账号余额需要全局授权"}],"comparison":"使用本平台的历史观测、诊断报告和采购金额；不引入外部排名"}),
    ))
}
