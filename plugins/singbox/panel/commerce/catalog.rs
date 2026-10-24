use super::*;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};

#[derive(Clone, Deserialize, Serialize, sqlx::FromRow)]
pub(super) struct Offer {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub package_group_id: i64,
    pub package_snapshot: Value,
    pub policy_group_ids: Vec<i64>,
    pub price_cents: i64,
    pub duration_months: Option<i32>,
    pub enabled: bool,
    pub renewable: bool,
    pub capacity: Option<i32>,
    pub version: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    name: String,
    description: String,
    package_group_id: i64,
    policy_group_ids: Vec<i64>,
    price_cents: i64,
    duration_months: Option<i32>,
    enabled: bool,
    renewable: bool,
    capacity: Option<i32>,
    version: Option<i64>,
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Offer>>> {
    administrator(&state, &headers, false).await?;
    Ok(Json(
        sqlx::query_as("SELECT * FROM singbox_commerce_offers ORDER BY id")
            .fetch_all(&state.pool)
            .await?,
    ))
}
pub(super) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> ApiResult<Json<Offer>> {
    let actor = administrator(&state, &headers, true).await?;
    Ok(Json(save(&state, &actor, None, input).await?))
}
pub(super) async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Input>,
) -> ApiResult<Json<Offer>> {
    let actor = administrator(&state, &headers, true).await?;
    Ok(Json(save(&state, &actor, Some(id), input).await?))
}
async fn save(state: &AppState, actor: &str, id: Option<i64>, input: Input) -> ApiResult<Offer> {
    let name = crate::business::name(&input.name)?;
    let description = text(&input.description, 4000, false)?;
    if !(0..=100_000_000).contains(&input.price_cents)
        || input
            .duration_months
            .is_some_and(|n| !(1..=1200).contains(&n))
        || input
            .capacity
            .is_some_and(|n| !(1..=1_000_000).contains(&n))
        || input.policy_group_ids.len() > 100
    {
        return Err(bad("销售价格、周期或容量无效"));
    }
    let mut groups = input.policy_group_ids;
    groups.sort_unstable();
    groups.dedup();
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM singbox_policy_groups WHERE id=ANY($1)")
            .bind(&groups)
            .fetch_one(&mut *tx)
            .await?;
    if count != groups.len() as i64 {
        return Err(bad("策略组不存在"));
    }
    let package:Value=sqlx::query_scalar("SELECT jsonb_build_object('name',name,'monthly_bytes',monthly_bytes::text,'reset_day',reset_day,'reset_hour',reset_hour,'reset_minute',reset_minute,'timezone',timezone,'duration_days',duration_days) FROM singbox_package_groups WHERE id=$1 AND deleted_at IS NULL")
        .bind(input.package_group_id).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    let query = if id.is_some() {
        "UPDATE singbox_commerce_offers SET name=$1,description=$2,package_group_id=$3,package_snapshot=$4,policy_group_ids=$5,price_cents=$6,duration_months=$7,enabled=$8,renewable=$9,capacity=$10,updated_at=$11,version=version+1 WHERE id=$12 AND version=$13 RETURNING *"
    } else {
        "INSERT INTO singbox_commerce_offers(name,description,package_group_id,package_snapshot,policy_group_ids,price_cents,duration_months,enabled,renewable,capacity,created_at,updated_at) SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$11 WHERE $12::bigint IS NULL AND $13::bigint IS NULL RETURNING *"
    };
    let offer: Offer = sqlx::query_as(query)
        .bind(name)
        .bind(description)
        .bind(input.package_group_id)
        .bind(package)
        .bind(groups)
        .bind(input.price_cents)
        .bind(input.duration_months)
        .bind(input.enabled)
        .bind(input.renewable)
        .bind(input.capacity)
        .bind(now_timestamp())
        .bind(id)
        .bind(input.version)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| conflict("销售套餐已变化，请刷新后再保存"))?;
    event(
        &mut tx,
        None,
        actor,
        "offer_saved",
        &offer.id.to_string(),
        json!({"version":offer.version}),
    )
    .await?;
    tx.commit().await?;
    Ok(offer)
}

pub(super) async fn snapshot(
    tx: &mut Tx<'_>,
    id: i64,
    version: i64,
    beneficiary: Option<i64>,
) -> ApiResult<Offer> {
    let offer: Offer =
        sqlx::query_as("SELECT * FROM singbox_commerce_offers WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    compatibility::writable(tx).await?;
    compatibility::mapped(tx, offer.package_group_id).await?;
    if !offer.enabled || offer.version != version {
        return Err(conflict("销售套餐或价格已变化，请刷新后确认"));
    }
    let same:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_user_packages p JOIN singbox_package_assignments a ON a.id=p.assignment_id WHERE p.user_id=$1 AND a.package_group_id=$2)").bind(beneficiary).bind(offer.package_group_id).fetch_one(&mut **tx).await?;
    if same && !offer.renewable {
        return Err(conflict("此套餐已停止续费"));
    }
    if let Some(capacity) = offer.capacity {
        let used:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM singbox_user_packages p JOIN singbox_package_assignments a ON a.id=p.assignment_id JOIN users u ON u.id=p.user_id WHERE a.package_group_id=$1 AND a.expires_at>$2 AND u.deleted_at IS NULL AND ($3::bigint IS NULL OR p.user_id<>$3)) + (SELECT count(*) FROM singbox_commerce_invitations WHERE state='available' AND (snapshot->>'package_group_id')::bigint=$1)").bind(offer.package_group_id).bind(now_timestamp()).bind(beneficiary).fetch_one(&mut **tx).await?;
        if used >= i64::from(capacity) {
            return Err(conflict("套餐名额已用完"));
        }
    }
    Ok(offer)
}

pub(super) async fn deliver(
    tx: &mut Tx<'_>,
    user: i64,
    request: Uuid,
    offer: &Offer,
) -> ApiResult<i64> {
    compatibility::writable(tx).await?;
    compatibility::mapped(tx, offer.package_group_id).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM singbox_policy_groups WHERE id=ANY($1)")
            .bind(&offer.policy_group_ids)
            .fetch_one(&mut **tx)
            .await?;
    if count != offer.policy_group_ids.len() as i64 {
        return Err(conflict("套餐关联的策略组已移除，需要管理员恢复后交付"));
    }
    let now = now_timestamp();
    let previous:Option<(i64,i64)>=sqlx::query_as("SELECT a.package_group_id,a.expires_at FROM singbox_user_packages p JOIN singbox_package_assignments a ON a.id=p.assignment_id WHERE p.user_id=$1").bind(user).fetch_optional(&mut **tx).await?;
    let anchor = previous
        .filter(|(id, _)| *id == offer.package_group_id)
        .map(|(_, expiry)| expiry.max(now))
        .unwrap_or(now);
    let p = &offer.package_snapshot;
    let expires:i64=sqlx::query_scalar("SELECT CASE WHEN $2::integer IS NULL THEN $1::bigint+$3::bigint*86400 ELSE EXTRACT(EPOCH FROM ((to_timestamp($1) AT TIME ZONE $4)+make_interval(months=>$2)) AT TIME ZONE $4)::bigint END")
        .bind(anchor).bind(offer.duration_months).bind(p["duration_days"].as_i64().ok_or_else(|| bad("套餐快照不完整"))?).bind(p["timezone"].as_str().ok_or_else(|| bad("套餐时区缺失"))?).fetch_one(&mut **tx).await?;
    let assignment:i64=sqlx::query_scalar("INSERT INTO singbox_package_assignments(user_id,request_id,package_group_id,package_name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,starts_at,expires_at) VALUES($1,$2,$3,$4,$5::text::numeric,$6,$7,$8,$9,$10,$11) RETURNING id")
        .bind(user).bind(request).bind(offer.package_group_id).bind(&offer.name).bind(p["monthly_bytes"].as_str())
        .bind(p["reset_day"].as_i64().unwrap_or(1) as i32).bind(p["reset_hour"].as_i64().unwrap_or(0) as i32).bind(p["reset_minute"].as_i64().unwrap_or(0) as i32).bind(p["timezone"].as_str().unwrap_or("UTC")).bind(now).bind(expires).fetch_one(&mut **tx).await?;
    sqlx::query("INSERT INTO singbox_user_packages(user_id,assignment_id) VALUES($1,$2) ON CONFLICT(user_id) DO UPDATE SET assignment_id=EXCLUDED.assignment_id").bind(user).bind(assignment).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM singbox_user_policies WHERE user_id=$1 AND commerce_grant AND NOT manual_grant").bind(user).execute(&mut **tx).await?;
    sqlx::query("UPDATE singbox_user_policies SET commerce_grant=FALSE WHERE user_id=$1")
        .bind(user)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO singbox_user_policies(user_id,group_id,manual_grant,commerce_grant) SELECT $1,unnest($2::bigint[]),FALSE,TRUE ON CONFLICT(user_id,group_id) DO UPDATE SET commerce_grant=TRUE").bind(user).bind(&offer.policy_group_ids).execute(&mut **tx).await?;
    sqlx::query("UPDATE singbox_members SET state='active',first_purchase_at=COALESCE(first_purchase_at,$2),version=version+1 WHERE user_id=$1").bind(user).bind(now).execute(&mut **tx).await?;
    crate::policies::sync_users(tx, &[user]).await?;
    let servers = crate::business::lock_user_servers(tx, user).await?;
    crate::business::mark_dirty(tx, &servers).await?;
    Ok(assignment)
}
