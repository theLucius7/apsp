//! Cloud resources linked to servers: the link records, billing and expiry observations,
//! and the read-only Hetzner inventory (resources).

pub mod hetzner;

use crate::text::label;
use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use uuid::Uuid;

/// A registered cloud resource as the operations page lists it: what resources keeps of it
/// (identity, account, cloud snapshot, server link) and, until operations keeps its own
/// rows, the power columns. The page adds what costs keeps (`/api/operations/cloud`, composed
/// in the application).
pub struct Listed {
    pub id: Uuid,
    pub account_id: Uuid,
    pub view: Value,
}

/// The page's resources by account and resource name: all of them for `global` readers,
/// otherwise those linked to `servers`.
pub async fn listed(pool: &PgPool, global: bool, servers: &[i64]) -> ApiResult<Vec<Listed>> {
    let rows: Vec<(Uuid, Uuid, Value)> = sqlx::query_as("SELECT r.id,r.account_id,jsonb_build_object('id',r.id,'account_id',r.account_id,'account_name',a.name,'provider','alicloud','kind',r.kind,'region',r.region,'cloud_id',r.cloud_id,'name',r.name,'snapshot',r.snapshot,'checked_at',r.checked_at,'error_code',r.error_code,'manual_hold',r.manual_hold,'power_policy',r.power_policy,'power_state',r.power_state,'power_error',r.power_error,'link',CASE WHEN l.resource_id IS NULL THEN NULL ELSE to_jsonb(l) END,'provider_read_only_source','阿里云 ECS/VPC/BSS 官方 API 缓存') FROM alicloud_resources r JOIN alicloud_accounts a ON a.id=r.account_id LEFT JOIN operations_cloud_links l ON l.resource_id=r.id WHERE NOT r.archived AND NOT a.archived AND ($1 OR l.server_id=ANY($2)) ORDER BY a.name,r.name LIMIT 500")
        .bind(global).bind(servers).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, account_id, view)| Listed {
            id,
            account_id,
            view,
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    server_id: Option<i64>,
    purchase_reference: String,
    purchase_amount: Option<String>,
    currency: Option<String>,
    monthly_budget: Option<String>,
    expires_at: Option<i64>,
    notes: String,
}

fn money(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        apsp_panel_host::money::decimal(value, 6).is_some_and(|(whole, _)| whole.len() <= 18)
    })
}

pub async fn link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Link>,
) -> ApiResult<Json<Value>> {
    let actor = control_center::require_capability(&state, &headers, "cloud:write").await?;
    if request.purchase_reference.chars().count() > 512
        || request.notes.chars().count() > 4096
        || !money(request.purchase_amount.as_deref())
        || !money(request.monthly_budget.as_deref())
        || request
            .currency
            .as_ref()
            .is_some_and(|s| !apsp_panel_host::money::currency(s))
        || ((request.purchase_amount.is_some() || request.monthly_budget.is_some())
            && request.currency.is_none())
    {
        return Err(ApiError::BadRequest(
            "采购引用、金额、币种或备注无效".into(),
        ));
    }
    if !request.purchase_reference.is_empty() {
        label(&request.purchase_reference, 512)?;
    }
    if !request.notes.is_empty() {
        label(&request.notes, 4096)?;
    }
    let account: Uuid = sqlx::query_scalar(
        "SELECT account_id FROM alicloud_resources WHERE id=$1 AND NOT archived",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;
    let mut tx = crate::cloud::lock(&state.pool, account).await?;
    let current: Option<Uuid> = sqlx::query_scalar(
        "SELECT account_id FROM alicloud_resources WHERE id=$1 AND NOT archived FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if current != Some(account) {
        return Err(ApiError::Conflict("云资源归属已变化，请重新读取".into()));
    }
    let previous: Option<i64> = sqlx::query_scalar(
        "SELECT server_id FROM operations_cloud_links WHERE resource_id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    if previous.is_none() || request.server_id.is_none() {
        crate::control_center::require_global_material(&state, &headers, "cloud:write").await?;
    }
    for server in [previous, request.server_id].into_iter().flatten() {
        control_center::require_server(&state, &headers, server, "cloud:write").await?;
    }
    if let Some(server) = request.server_id {
        crate::servers::lock_row(&mut tx, server, crate::servers::RowLock::Share, "id")
            .await?
            .ok_or(ApiError::NotFound)?;
    }
    let changing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alicloud_security_group_operations WHERE resource_id=$1 AND status IN ('running','unknown'))")
        .bind(id).fetch_one(&mut *tx).await?;
    if changing {
        return Err(ApiError::Conflict(
            "安全组操作仍在执行或等待核对，暂不能修改资源关联".into(),
        ));
    }
    sqlx::query("INSERT INTO operations_cloud_links(resource_id,server_id,purchase_reference,purchase_amount,currency,monthly_budget,expires_at,notes,updated_at,updated_by) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(resource_id) DO UPDATE SET server_id=EXCLUDED.server_id,purchase_reference=EXCLUDED.purchase_reference,purchase_amount=EXCLUDED.purchase_amount,currency=EXCLUDED.currency,monthly_budget=EXCLUDED.monthly_budget,expires_at=EXCLUDED.expires_at,notes=EXCLUDED.notes,updated_at=EXCLUDED.updated_at,updated_by=EXCLUDED.updated_by")
        .bind(id).bind(request.server_id).bind(request.purchase_reference).bind(request.purchase_amount).bind(request.currency).bind(request.monthly_budget).bind(request.expires_at).bind(request.notes).bind(now_timestamp()).bind(actor).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"resource_id":id,"server_id":request.server_id}),
    ))
}

pub async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Value>>> {
    let server: Option<i64> =
        sqlx::query_scalar("SELECT server_id FROM operations_cloud_links WHERE resource_id=$1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .flatten();
    control_center::require_capability(&state, &headers, "cloud:read").await?;
    if let Some(server) = server {
        control_center::require_server(&state, &headers, server, "cloud:read").await?;
    } else {
        crate::control_center::require_global_material(&state, &headers, "cloud:read").await?;
    }
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(o) FROM operations_cloud_observations o WHERE resource_id=$1 ORDER BY checked_at DESC,id DESC LIMIT 200").bind(id).fetch_all(&state.pool).await?))
}

fn decimal(value: &str) -> Option<i128> {
    let (negative, value) = value
        .strip_prefix('-')
        .map_or((false, value), |v| (true, v));
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || whole.len() > 18
        || fraction.len() > 12
        || !whole.bytes().all(|v| v.is_ascii_digit())
        || !fraction.bytes().all(|v| v.is_ascii_digit())
    {
        return None;
    }
    let whole = whole.parse::<i128>().ok()?.checked_mul(1_000_000_000_000)?;
    let tail = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<i128>()
            .ok()?
            .checked_mul(10_i128.pow(12 - fraction.len() as u32))?
    };
    let total = whole.checked_add(tail)?;
    Some(if negative { -total } else { total })
}

/// What costs last read of a resource's monthly bill, as stored: the bill, or why it could
/// not be read.
pub struct Bill {
    pub bill: Option<Value>,
    pub error: Option<String>,
}

/// Records each resource's cloud snapshot and bill in its observation history, and opens
/// the budget and expiry incidents. `bills` holds what costs read, by resource.
pub async fn observe(pool: &PgPool, bills: &HashMap<Uuid, Bill>) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    let locked = crate::locks::CLOUD_OBSERVATIONS.try_lock(&mut tx).await?;
    if !locked {
        return Ok(());
    }
    let rows=sqlx::query("SELECT r.id,r.name,r.snapshot,r.checked_at,r.error_code,r.last_attempt_at,l.server_id,l.monthly_budget,l.currency,l.expires_at FROM alicloud_resources r LEFT JOIN operations_cloud_links l ON l.resource_id=r.id WHERE NOT r.archived AND (r.checked_at IS NOT NULL OR r.last_attempt_at>0) ORDER BY r.last_attempt_at DESC LIMIT 500").fetch_all(&mut *tx).await?;
    for row in rows {
        let id: Uuid = row.get("id");
        let bill = bills.get(&id);
        let bill_error = bill.and_then(|bill| bill.error.clone());
        let checked = row
            .get::<Option<i64>, _>("checked_at")
            .unwrap_or(0)
            .max(row.get("last_attempt_at"));
        let snapshot = json!({"resource":row.get::<Option<Value>,_>("snapshot"),"error_code":row.get::<Option<String>,_>("error_code"),"bill":bill.and_then(|bill| bill.bill.clone()),"bill_error":bill_error});
        let checked = checked.max(snapshot["bill"]["queried_at"].as_i64().unwrap_or(0));
        let previous:Option<Value>=sqlx::query_scalar("SELECT snapshot FROM operations_cloud_observations WHERE resource_id=$1 ORDER BY checked_at DESC,id DESC LIMIT 1").bind(id).fetch_optional(&mut *tx).await?;
        let mut changes = Vec::new();
        if let Some(previous) = &previous {
            for key in [
                "public_ip",
                "bandwidth_mbps",
                "charge_type",
                "resource_charge_type",
                "status",
            ] {
                if previous["resource"][key] != snapshot["resource"][key] {
                    changes.push(json!({"field":key,"before":previous["resource"][key],"after":snapshot["resource"][key]}));
                }
            }
            if previous["error_code"] != snapshot["error_code"] {
                changes.push(json!({"field":"provider_error","before":previous["error_code"],"after":snapshot["error_code"]}));
            }
        }
        sqlx::query("INSERT INTO operations_cloud_observations(resource_id,checked_at,source,snapshot,previous_snapshot,changes) VALUES($1,$2,'alicloud_official_cache',$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(id).bind(checked).bind(&snapshot).bind(previous).bind(json!(changes)).execute(&mut *tx).await?;
        let now = now_timestamp();
        let currency: Option<String> = row.get("currency");
        let budget: Option<String> = row.get("monthly_budget");
        let queried = snapshot["bill"]["queried_at"].as_i64();
        if bill_error.is_none()
            && queried.is_some_and(|v| v <= now && now - v <= 21600)
            && let (Some(currency), Some(budget), Some(entries)) =
                (currency, budget, snapshot["bill"]["rows"].as_array())
        {
            // Bills in another currency are not converted; say so instead of
            // silently treating the budget as never reached.
            if entries
                .iter()
                .any(|v| v["currency"].as_str() != Some(currency.as_str()))
            {
                crate::evidence::open_incident_tx(&mut tx, crate::evidence::IncidentReport { key: &format!("cloud:budget-currency:{id}:{}",snapshot["bill"]["month"].as_str().unwrap_or("unknown")), server: row.get("server_id"), title: &format!("云资源 {} 的账单含预算币种以外的金额，未计入预算比较",row.get::<String,_>("name")), severity: "info", evidence: json!({"provider":"alicloud","resource_id":id,"queried_at":queried,"budget_currency":currency,"bill":snapshot["bill"],"converted":false}), observed_at: queried.unwrap() }).await?;
            }
            let actual = entries
                .iter()
                .filter(|v| v["currency"].as_str() == Some(currency.as_str()))
                .try_fold(0_i128, |sum, entry| {
                    sum.checked_add(decimal(entry["amount"].as_str()?)?)
                });
            if let (Some(actual), Some(budget_number)) = (actual, decimal(&budget))
                && actual >= budget_number
                && entries
                    .iter()
                    .any(|v| v["currency"].as_str() == Some(currency.as_str()))
            {
                crate::evidence::open_incident_tx(&mut tx, crate::evidence::IncidentReport { key: &format!("cloud:budget:{id}:{}",snapshot["bill"]["month"].as_str().unwrap_or("unknown")), server: row.get("server_id"), title: &format!("云资源 {} 的账单缓存达到月预算",row.get::<String,_>("name")), severity: "warning", evidence: json!({"provider":"alicloud","resource_id":id,"queried_at":queried,"currency":currency,"budget":budget,"bill":snapshot["bill"],"real_time_limit":false}), observed_at: queried.unwrap() }).await?;
            }
        }
        if let Some(expires) = row.get::<Option<i64>, _>("expires_at")
            && expires - now <= 7 * 86400
            && now - expires <= 7 * 86400
        {
            crate::evidence::open_incident_tx(&mut tx, crate::evidence::IncidentReport { key: &format!("cloud:expiry:{id}:{expires}"), server: row.get("server_id"), title: &format!("云资源 {} 的采购到期记录需要处理",row.get::<String,_>("name")), severity: "warning", evidence: json!({"resource_id":id,"expires_at":expires,"source":"人工采购台账，非实时云计费状态"}), observed_at: now }).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn budget_comparison_keeps_decimal_precision_and_refunds() {
        assert_eq!(
            super::decimal("100.000000000001"),
            Some(100_000_000_000_001)
        );
        assert_eq!(super::decimal("-0.01"), Some(-10_000_000_000));
        assert_eq!(super::decimal("1e3"), None);
        assert_eq!(super::decimal("1.1234567890123"), None);
    }
}
