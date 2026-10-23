use crate::{
    AppState,
    auth::require_admin,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

const COLUMNS: &str =
    "id,name,monthly_bytes::text,reset_day,reset_hour,reset_minute,timezone,duration_days";

#[derive(Serialize, FromRow)]
pub struct Package {
    pub id: i64,
    pub name: String,
    pub monthly_bytes: Option<String>,
    pub reset_day: i32,
    pub reset_hour: i32,
    pub reset_minute: i32,
    pub timezone: String,
    pub duration_days: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRequest {
    pub name: String,
    pub monthly_bytes: Option<String>,
    pub reset_day: i32,
    pub reset_hour: i32,
    pub reset_minute: i32,
    pub timezone: String,
    pub duration_days: i32,
}

fn validate(request: &PackageRequest) -> ApiResult<String> {
    let name = super::business::name(&request.name)?;
    if request.monthly_bytes.as_ref().is_some_and(|v| {
        v.parse::<u64>().is_err() || v == "0" || v.parse::<u64>().is_ok_and(|n| n.to_string() != *v)
    }) {
        return Err(ApiError::BadRequest(
            "每月流量必须是正整数字节数；不限量请传 null".into(),
        ));
    }
    if !(1..=31).contains(&request.reset_day)
        || !(0..=23).contains(&request.reset_hour)
        || !(0..=59).contains(&request.reset_minute)
        || !(1..=36500).contains(&request.duration_days)
    {
        return Err(ApiError::BadRequest(
            "请检查重置日期、时间与有效天数".into(),
        ));
    }
    if request.timezone.len() > 128 {
        return Err(ApiError::BadRequest("时区名称过长".into()));
    }
    Ok(name)
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Package>>> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM singbox_package_groups WHERE deleted_at IS NULL ORDER BY id"
        ))
        .fetch_all(&state.pool)
        .await?,
    ))
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PackageRequest>,
) -> ApiResult<(StatusCode, Json<Package>)> {
    require_admin(&state, &headers).await?;
    Ok((
        StatusCode::CREATED,
        Json(save(&state, None, request).await?),
    ))
}

pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(request): Json<PackageRequest>,
) -> ApiResult<Json<Package>> {
    require_admin(&state, &headers).await?;
    Ok(Json(save(&state, Some(id), request).await?))
}

async fn save(state: &AppState, id: Option<i64>, request: PackageRequest) -> ApiResult<Package> {
    let name = validate(&request)?;
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    let valid_zone: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name=$1)")
            .bind(&request.timezone)
            .fetch_one(&mut *tx)
            .await?;
    if !valid_zone {
        return Err(ApiError::BadRequest(
            "请选择有效的 IANA 时区，例如 Asia/Taipei".into(),
        ));
    }
    let query = if id.is_some() {
        format!(
            "UPDATE singbox_package_groups SET name=$1,monthly_bytes=$2::text::numeric,reset_day=$3,reset_hour=$4,reset_minute=$5,timezone=$6,duration_days=$7 WHERE id=$8 AND deleted_at IS NULL RETURNING {COLUMNS}"
        )
    } else {
        format!(
            "INSERT INTO singbox_package_groups(name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,duration_days) SELECT $1,$2::text::numeric,$3,$4,$5,$6,$7 WHERE $8::bigint IS NULL RETURNING {COLUMNS}"
        )
    };
    let value = sqlx::query_as(&query)
        .bind(name)
        .bind(request.monthly_bytes)
        .bind(request.reset_day)
        .bind(request.reset_hour)
        .bind(request.reset_minute)
        .bind(request.timezone)
        .bind(request.duration_days)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    tx.commit().await?;
    Ok(value)
}

pub async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    let result = sqlx::query(
        "UPDATE singbox_package_groups SET deleted_at=$2 WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(apsp_protocol::now_timestamp())
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignRequest {
    pub package_group_id: i64,
    pub request_id: Uuid,
}

#[derive(Serialize, FromRow)]
pub struct Assignment {
    pub id: i64,
    pub user_id: i64,
    pub package_group_id: i64,
    pub starts_at: i64,
    pub expires_at: i64,
}

pub async fn assign(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(request): Json<AssignRequest>,
) -> ApiResult<Json<Assignment>> {
    require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    super::business::lock_user(&mut tx, id).await?;
    if let Some(previous) = sqlx::query_as::<_, Assignment>("SELECT id,user_id,package_group_id,starts_at,expires_at FROM singbox_package_assignments WHERE user_id=$1 AND request_id=$2")
        .bind(id).bind(request.request_id).fetch_optional(&mut *tx).await? {
        if previous.package_group_id != request.package_group_id {
            return Err(ApiError::Conflict("此请求编号已用于另一次套餐分配".into()));
        }
        tx.commit().await?;
        return Ok(Json(previous));
    }
    let servers = super::business::lock_user_servers(&mut tx, id).await?;
    let value = sqlx::query_as::<_, Assignment>("INSERT INTO singbox_package_assignments(user_id,request_id,package_group_id,package_name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,starts_at,expires_at) SELECT $1,$2,id,name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,$4,$4+duration_days::bigint*86400 FROM singbox_package_groups WHERE id=$3 AND deleted_at IS NULL RETURNING id,user_id,package_group_id,starts_at,expires_at")
        .bind(id).bind(request.request_id).bind(request.package_group_id).bind(apsp_protocol::now_timestamp())
        .fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    sqlx::query("INSERT INTO singbox_user_packages(user_id,assignment_id) VALUES($1,$2) ON CONFLICT(user_id) DO UPDATE SET assignment_id=EXCLUDED.assignment_id")
        .bind(id).bind(value.id).execute(&mut *tx).await?;
    super::business::mark_dirty(&mut tx, &servers).await?;
    tx.commit().await?;
    Ok(Json(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_noncanonical_or_overflowing_quota() {
        let mut request = PackageRequest {
            name: "Plan".into(),
            monthly_bytes: None,
            reset_day: 31,
            reset_hour: 0,
            reset_minute: 0,
            timezone: "Asia/Taipei".into(),
            duration_days: 30,
        };
        for invalid in ["0", "-1", "1.5", "01", "+1", "18446744073709551616"] {
            request.monthly_bytes = Some(invalid.into());
            assert!(validate(&request).is_err(), "{invalid}");
        }
        request.monthly_bytes = Some(u64::MAX.to_string());
        assert!(validate(&request).is_ok());
    }
}
