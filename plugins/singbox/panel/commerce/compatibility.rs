use super::*;
use axum::{Json, extract::State};

pub(super) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    let value: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(b)-'pending_generation' FROM singbox_nexus_bridge b WHERE singleton",
    )
    .fetch_optional(&state.pool)
    .await?;
    let mappings:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p)||jsonb_build_object('package_name',g.name) FROM singbox_nexus_plan_mappings p JOIN singbox_package_groups g ON g.id=p.package_group_id ORDER BY p.package_group_id").fetch_all(&state.pool).await?;
    Ok(Json(json!({"bridge":value,"mappings":mappings})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Activate {
    import_id: Uuid,
    subscription_base: String,
    confirmed_digest: String,
    confirm_legacy_read_only: bool,
}
pub(super) async fn activate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Activate>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    let base =
        reqwest::Url::parse(&input.subscription_base).map_err(|_| bad("原订阅基础地址无效"))?;
    if !input.confirm_legacy_read_only
        || base.scheme() != "https"
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
        || !base.path().ends_with('/')
    {
        return Err(bad("请确认旧业务只读，并填写以 / 结尾的原 HTTPS 订阅路径"));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let digest: String = sqlx::query_scalar("SELECT digest FROM singbox_nexus_imports WHERE id=$1")
        .bind(input.import_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    if digest != input.confirmed_digest {
        return Err(conflict("请确认待激活快照摘要"));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_bridge)")
        .fetch_one(&mut *tx)
        .await?;
    if exists {
        return Err(conflict("兼容通道已配置，重复激活不会覆盖当前成员资格"));
    }
    let unmapped:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_identities i WHERE i.import_id=$1 AND i.original_plan_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM singbox_nexus_plan_mappings p WHERE p.import_id=i.import_id AND p.legacy_plan_id=i.original_plan_id AND p.legacy_group_id IS NOT NULL))").bind(input.import_id).fetch_one(&mut *tx).await?;
    if unmapped {
        return Err(conflict("原套餐缺少权限组映射，不能激活"));
    }
    // Native members are enrolled too; an active one on an unmapped package would make
    // every later synchronization fail.
    let native_unmapped:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_members m JOIN users u ON u.id=m.user_id JOIN singbox_user_packages up ON up.user_id=m.user_id JOIN singbox_package_assignments a ON a.id=up.assignment_id WHERE m.state='active' AND u.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM singbox_nexus_plan_mappings p WHERE p.import_id=$1 AND p.package_group_id=a.package_group_id AND p.legacy_group_id IS NOT NULL))").bind(input.import_id).fetch_one(&mut *tx).await?;
    if native_unmapped {
        return Err(conflict(
            "已有活跃成员使用未映射原权限组的套餐，请先补充映射或调整套餐后再激活",
        ));
    }
    let members=sqlx::query("SELECT *,quota_bytes::text AS quota_text FROM singbox_nexus_identities WHERE import_id=$1 ORDER BY user_id").bind(input.import_id).fetch_all(&mut *tx).await?;
    for row in members {
        let user: i64 = row.try_get("user_id")?;
        ensure_member(&mut tx, user).await?;
        if let Some(plan) = row.try_get::<Option<i64>, _>("original_plan_id")? {
            let package:i64=sqlx::query_scalar("SELECT package_group_id FROM singbox_nexus_plan_mappings WHERE import_id=$1 AND legacy_plan_id=$2 ORDER BY package_group_id LIMIT 1").bind(input.import_id).bind(plan).fetch_one(&mut *tx).await?;
            let expiry = row
                .try_get::<Option<i64>, _>("expires_at")?
                .unwrap_or(253402300799);
            let assignment:i64=sqlx::query_scalar("INSERT INTO singbox_package_assignments(user_id,request_id,package_group_id,package_name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,starts_at,expires_at) SELECT $1,$2,id,name,$3::text::numeric,reset_day,reset_hour,reset_minute,timezone,$4,$5 FROM singbox_package_groups WHERE id=$6 RETURNING id").bind(user).bind(Uuid::new_v4()).bind(row.try_get::<Option<String>,_>("quota_text")?).bind(now_timestamp().min(expiry.saturating_sub(1))).bind(expiry).bind(package).fetch_one(&mut *tx).await?;
            sqlx::query("INSERT INTO singbox_user_packages(user_id,assignment_id) VALUES($1,$2)")
                .bind(user)
                .bind(assignment)
                .execute(&mut *tx)
                .await?;
        }
        let status: String = row.try_get("original_state")?;
        sqlx::query("UPDATE singbox_members SET state=$2,version=version+1 WHERE user_id=$1 AND state='staged'").bind(user).bind(status).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO singbox_nexus_bridge(singleton,import_id,subscription_base,enabled,activated_at) VALUES(TRUE,$1,$2,TRUE,$3)").bind(input.import_id).bind(base.as_str()).bind(now_timestamp()).execute(&mut *tx).await?;
    event(
        &mut tx,
        None,
        &actor,
        "nexus_compatibility_activated",
        &input.import_id.to_string(),
        json!({"original_links_preserved":true}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"enabled":true,"waiting_for_sync":true})))
}
pub(super) async fn writable(tx: &mut Tx<'_>) -> ApiResult<()> {
    let stale:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_bridge WHERE enabled AND COALESCE(last_sync_at,0)<$1)").bind(now_timestamp()-120).fetch_one(&mut **tx).await?;
    if stale {
        return Err(conflict(
            "原订阅兼容通道尚未完成同步，请稍后再试；当前操作未扣款",
        ));
    }
    Ok(())
}
pub(super) async fn mapped(tx: &mut Tx<'_>, package: i64) -> ApiResult<()> {
    let missing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_bridge b WHERE b.enabled AND NOT EXISTS(SELECT 1 FROM singbox_nexus_plan_mappings p WHERE p.import_id=b.import_id AND p.package_group_id=$1 AND p.legacy_group_id IS NOT NULL))").bind(package).fetch_one(&mut **tx).await?;
    if missing {
        return Err(conflict("此套餐尚未配置原节点权限映射，暂不可购买"));
    }
    Ok(())
}
