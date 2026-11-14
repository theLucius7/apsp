//! Cloud resources linked to servers: the link records, the observation history of each
//! resource, and the read-only Hetzner inventory (resources). A linked resource's price and
//! expiry are its server's asset settings; the budget, and the purchase of a resource that is
//! not linked, are costs' (`costs`).

pub mod costs;
pub mod hetzner;

use crate::text::label;
use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
    server_assets::AssetSettings,
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
/// (identity, account, cloud snapshot, server link). The page adds what costs and operations
/// keep (`/api/operations/cloud`, composed in the application): the bills, the budget in
/// `link` and, for a resource without a server (`linked` false), its purchase in `link`, and
/// the power state.
pub struct Listed {
    pub id: Uuid,
    pub account_id: Uuid,
    pub linked: bool,
    pub view: Value,
}

/// The page's resources by account and resource name: all of them for `global` readers,
/// otherwise those linked to `servers`. A linked resource's `link` carries its server's
/// price, currency, expiry and order reference as the purchase.
pub async fn listed(pool: &PgPool, global: bool, servers: &[i64]) -> ApiResult<Vec<Listed>> {
    use crate::server_assets::sql;
    let rows: Vec<(Uuid, Uuid, bool, Value)> = sqlx::query_as(&format!("SELECT r.id,r.account_id,l.server_id IS NOT NULL,jsonb_build_object('id',r.id,'account_id',r.account_id,'account_name',a.name,'provider','alicloud','kind',r.kind,'region',r.region,'cloud_id',r.cloud_id,'name',r.name,'snapshot',r.snapshot,'checked_at',r.checked_at,'error_code',r.error_code,'link',CASE WHEN l.resource_id IS NULL THEN NULL WHEN l.server_id IS NULL THEN jsonb_build_object('resource_id',l.resource_id,'server_id',l.server_id,'notes',l.notes,'updated_at',l.updated_at,'updated_by',l.updated_by) ELSE jsonb_build_object('resource_id',l.resource_id,'server_id',l.server_id,'notes',l.notes,'updated_at',l.updated_at,'updated_by',l.updated_by,'purchase_reference',COALESCE({reference},''),'purchase_amount',{price},'currency',CASE WHEN {price} IS NULL THEN NULL ELSE {currency} END,'expires_at',{expires_at}) END,'provider_read_only_source','阿里云 ECS/VPC/BSS 官方 API 缓存') FROM alicloud_resources r JOIN alicloud_accounts a ON a.id=r.account_id LEFT JOIN operations_cloud_links l ON l.resource_id=r.id LEFT JOIN servers s ON s.id=l.server_id WHERE NOT r.archived AND NOT a.archived AND ($1 OR l.server_id=ANY($2)) ORDER BY a.name,r.name LIMIT 500",
        reference = sql::order_reference("s"),
        price = sql::price("s"),
        currency = sql::currency("s"),
        expires_at = sql::expires_at("s"),
    ))
    .bind(global).bind(servers).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, account_id, linked, view)| Listed {
            id,
            account_id,
            linked,
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
    /// The currency of the purchase amount; consoles from before budgets had their own
    /// currency send it for both.
    currency: Option<String>,
    monthly_budget: Option<String>,
    budget_currency: Option<String>,
    expires_at: Option<i64>,
    notes: String,
}

/// Two plain decimal amounts are the same number (`12.5` and `12.50`).
fn same_amount(left: &str, right: &str) -> bool {
    let digits = |value: &str| {
        apsp_panel_host::money::decimal(value, usize::MAX).map(|(whole, fraction)| {
            let whole = whole.trim_start_matches('0');
            (whole.to_owned(), fraction.trim_end_matches('0').to_owned())
        })
    };
    digits(left).is_some_and(|left| digits(right) == Some(left))
}

/// A linked resource's purchase is its server's: the form may leave it empty or repeat the
/// server's values, but cannot record another.
fn check_server_purchase(request: &Link, server: &AssetSettings) -> ApiResult<()> {
    let reference = request.purchase_reference.trim();
    let order = server
        .procurement
        .as_ref()
        .map_or("", |procurement| procurement.order_reference.as_str());
    let amount = match (request.purchase_amount.as_deref(), server.price.as_deref()) {
        (None, _) => true,
        (Some(amount), Some(price)) => {
            same_amount(amount, price)
                && request.currency.as_deref() == Some(server.currency.as_str())
        }
        (Some(_), None) => false,
    };
    if (reference.is_empty() || reference == order)
        && amount
        && request
            .expires_at
            .is_none_or(|at| server.expires_at == Some(at))
    {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "已关联服务器的云资源，采购金额、币种、到期与订单参考取自服务器的资产设置，请在服务器的资产与运维页修改".into(),
        ))
    }
}

pub async fn link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Link>,
) -> ApiResult<Json<Value>> {
    let actor = control_center::require_capability(&state, &headers, "cloud:write").await?;
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
        let row = crate::servers::lock_row(
            &mut tx,
            server,
            crate::servers::RowLock::Share,
            "asset_settings",
        )
        .await?
        .ok_or(ApiError::NotFound)?;
        let settings: AssetSettings =
            serde_json::from_value(row.get("asset_settings")).map_err(anyhow::Error::from)?;
        check_server_purchase(&request, &settings)?;
    }
    let changing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alicloud_security_group_operations WHERE resource_id=$1 AND status IN ('running','unknown'))")
        .bind(id).fetch_one(&mut *tx).await?;
    if changing {
        return Err(ApiError::Conflict(
            "安全组操作仍在执行或等待核对，暂不能修改资源关联".into(),
        ));
    }
    sqlx::query("INSERT INTO operations_cloud_links(resource_id,server_id,notes,updated_at,updated_by) VALUES($1,$2,$3,$4,$5) ON CONFLICT(resource_id) DO UPDATE SET server_id=EXCLUDED.server_id,notes=EXCLUDED.notes,updated_at=EXCLUDED.updated_at,updated_by=EXCLUDED.updated_by")
        .bind(id).bind(request.server_id).bind(&request.notes).bind(now_timestamp()).bind(actor).execute(&mut *tx).await?;
    let linked = request.server_id.is_some();
    let budget_currency = match request.budget_currency.as_deref() {
        Some(currency) => costs::BudgetCurrency::Given(Some(currency)),
        // A form with one currency for both: repeating a linked server's purchase, that
        // currency is the purchase's and the budget keeps its own.
        None if linked && request.purchase_amount.is_some() => {
            costs::BudgetCurrency::Kept(request.currency.as_deref())
        }
        None => costs::BudgetCurrency::Given(request.currency.as_deref()),
    };
    costs::record(
        &mut tx,
        id,
        costs::Recorded {
            purchase: (!linked).then(|| costs::Purchase {
                reference: &request.purchase_reference,
                amount: request.purchase_amount.as_deref(),
                currency: request.currency.as_deref(),
                expires_at: request.expires_at,
            }),
            monthly_budget: request.monthly_budget.as_deref(),
            budget_currency,
        },
    )
    .await?;
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

/// What costs last read of a resource's monthly bill, as stored: the bill, or why it could
/// not be read.
pub struct Bill {
    pub bill: Option<Value>,
    pub error: Option<String>,
}

/// A resource whose observation was recorded in this pass, for costs' budget and expiry
/// checks: its name and the server it is linked to.
pub struct Observed {
    pub id: Uuid,
    pub name: String,
    pub server_id: Option<i64>,
}

/// Records each resource's cloud snapshot and bill in its observation history; `bills` holds
/// what costs read, by resource. Returns the resources observed, none while another pass
/// holds the observation lock.
pub async fn observe(pool: &PgPool, bills: &HashMap<Uuid, Bill>) -> ApiResult<Vec<Observed>> {
    let mut tx = pool.begin().await?;
    let locked = crate::locks::CLOUD_OBSERVATIONS.try_lock(&mut tx).await?;
    if !locked {
        return Ok(Vec::new());
    }
    let rows=sqlx::query("SELECT r.id,r.name,r.snapshot,r.checked_at,r.error_code,r.last_attempt_at,l.server_id FROM alicloud_resources r LEFT JOIN operations_cloud_links l ON l.resource_id=r.id WHERE NOT r.archived AND (r.checked_at IS NOT NULL OR r.last_attempt_at>0) ORDER BY r.last_attempt_at DESC LIMIT 500").fetch_all(&mut *tx).await?;
    let mut observed = Vec::with_capacity(rows.len());
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
        observed.push(Observed {
            id,
            name: row.get("name"),
            server_id: row.get("server_id"),
        });
    }
    tx.commit().await?;
    Ok(observed)
}
