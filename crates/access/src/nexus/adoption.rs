//! A member becomes an ordinary apsp user without rotating a wire identity or
//! rewriting usage. The first native cycle starts from a transactionally captured
//! meter/ledger anchor; subsequent cycles use the ordinary ledger directly.

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_compiler::Node;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adopt {
    expected_digest: String,
    compared_formats: Vec<String>,
    comparison_confirmed: bool,
    old_subscription_route_ready: bool,
}

struct Prepared {
    view: Value,
    nodes: Vec<Node>,
    digest: String,
    blockers: Vec<String>,
}

async fn prepared(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: i64,
) -> ApiResult<Prepared> {
    let identity = super::bridge::original_identity(tx, user)
        .await?
        .ok_or_else(|| ApiError::Conflict("此用户没有迁移身份".into()))?;
    let (mut view, mut nodes) = crate::subscriptions::native_preview(tx, user).await?;
    let mut blockers = Vec::new();
    let now = apsp_protocol::now_timestamp();
    let adopted: Option<String> =
        sqlx::query_scalar("SELECT preview_digest FROM singbox_native_adoptions WHERE user_id=$1")
            .bind(user)
            .fetch_optional(&mut **tx)
            .await?;
    let state_row = sqlx::query("SELECT p.assignment_id,a.starts_at,a.expires_at,a.monthly_bytes::text AS monthly_bytes,a.reset_day,a.reset_hour,a.reset_minute,a.timezone,m.observed_at,m.last_reset_at,m.next_reset_at,b.subscription_base,b.last_sync_at,b.pending_generation,b.native_control_ready,u.subscription_token FROM users u JOIN singbox_nexus_identities i ON i.user_id=u.id LEFT JOIN singbox_user_packages p ON p.user_id=u.id LEFT JOIN singbox_package_assignments a ON a.id=p.assignment_id LEFT JOIN singbox_nexus_metering m ON m.user_id=u.id LEFT JOIN singbox_nexus_bridge b ON b.import_id=i.import_id AND b.enabled WHERE u.id=$1 AND u.deleted_at IS NULL")
        .bind(user).fetch_one(&mut **tx).await?;
    let assignment: Option<i64> = state_row.get("assignment_id");
    if adopted.is_none() {
        if state_row.get::<Option<bool>, _>("native_control_ready") != Some(true) {
            blockers.push("请先更新并运行支持原生接管确认的兼容同步工具".into());
        }
        if (assignment.is_some()
            && state_row
                .get::<Option<i64>, _>("observed_at")
                .is_none_or(|at| at < now - 120 || at > now + 30))
            || state_row
                .get::<Option<i64>, _>("last_sync_at")
                .is_none_or(|at| at < now - 120)
        {
            blockers.push("兼容通道或用量读数已过期，请先完成一次同步".into());
        }
        if state_row
            .get::<Option<uuid::Uuid>, _>("pending_generation")
            .is_some()
        {
            blockers.push("兼容通道还有未确认的成员交换".into());
        }
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_usage_deliveries WHERE user_id=$1 AND delivered_at IS NULL)")
            .bind(user).fetch_one(&mut **tx).await?;
        if pending {
            blockers.push("仍有未确认投递的用量，请先排空后接管".into());
        }
    }
    let bounds = (
        view["entitlement"]["cycle_start"].as_i64(),
        view["entitlement"]["next_reset"].as_i64(),
    );
    if assignment.is_some()
        && !matches!(bounds, (Some(start), Some(end)) if start <= now && end > start && end < 253402300799 && (end > now || state_row.get::<Option<i64>,_>("expires_at").is_some_and(|at| at<=now)))
    {
        blockers.push("当前用量周期没有有效的结束时间".into());
    }
    let allowed = view["entitlement"]["allowed"].as_bool().unwrap_or(false);
    // Every old line the user's original group can render must have a healthy apsp
    // runtime. Taking over a subset would silently remove the remaining old lines.
    let group: Option<i64> = sqlx::query_scalar("SELECT singbox_member_legacy_group($1)")
        .bind(user)
        .fetch_one(&mut **tx)
        .await?;
    let missing = apsp_traffic::nexus_nodes::unmanaged_group_lines(tx, group).await?;
    if !missing.is_empty() {
        blockers.push(format!(
            "{} 条原线路尚未由健康的 apsp 运行时接管",
            missing.len()
        ));
    }
    let grants: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT pn.node_id FROM singbox_user_policies up JOIN singbox_policy_nodes pn ON pn.group_id=up.group_id WHERE up.user_id=$1 ORDER BY pn.node_id")
        .bind(user).fetch_all(&mut **tx).await?;
    let published: std::collections::BTreeSet<i64> = nodes.iter().map(|n| n.id).collect();
    if allowed && grants.iter().any(|node| !published.contains(node)) {
        blockers.push("策略组还有未应用或不可订阅的节点，请先修复部署".into());
    }
    if allowed && nodes.is_empty() {
        blockers.push("没有可比较的已应用节点".into());
    }
    if !allowed {
        nodes = apsp_traffic::nexus_nodes::adoption_nodes(tx, &grants, user, identity).await?;
    }
    let token: String = state_row.get("subscription_token");
    let base: Option<String> = state_row.get("subscription_base");
    let own: Option<String> = sqlx::query_scalar("SELECT CASE WHEN d.enabled AND h.state='ready' THEN 'https://'||d.host_prefix||h.slug||'.'||d.zone_name||'/'||h.path_token END FROM singbox_subscription_hosts h CROSS JOIN singbox_subscription_domain d WHERE h.user_id=$1")
        .bind(user).fetch_optional(&mut **tx).await?.flatten();
    let old_url = own
        .or_else(|| base.as_ref().map(|base| format!("{base}{token}")))
        .unwrap_or_else(|| format!("{}/sub/{token}", state.config.public_url));
    // Live counters and observation times deliberately do not invalidate a preview.
    // They are captured again under the usage-table locks when adoption commits.
    let digest = crate::auth::hash_token(&serde_json::to_string(&json!({
        "user":user,"identity":identity,"assignment":assignment,
        "starts_at":state_row.get::<Option<i64>,_>("starts_at"),
        "expires_at":state_row.get::<Option<i64>,_>("expires_at"),
        "monthly_bytes":state_row.get::<Option<String>,_>("monthly_bytes"),
        "reset_day":state_row.get::<Option<i32>,_>("reset_day"),
        "reset_hour":state_row.get::<Option<i32>,_>("reset_hour"),
        "reset_minute":state_row.get::<Option<i32>,_>("reset_minute"),
        "timezone":state_row.get::<Option<String>,_>("timezone"),
        "cycle_start":bounds.0,"next_reset":bounds.1,"grants":grants,
        "status":view["entitlement"]["status"],"credential_nodes":nodes,
        "formats":view["formats"],"old_subscription_url":old_url
    }))?);
    view["digest"] = json!(digest);
    view["adopted"] = json!(adopted.is_some());
    view["blockers"] = json!(blockers);
    view["old_subscription_url"] = json!(old_url);
    view["route_confirmation_required"] = json!(true);
    view["blocked"] = json!(!allowed);
    view["comparison_modes"] = if allowed {
        json!(["singbox", "clash", "links"])
    } else {
        json!(["blocked"])
    };
    Ok(Prepared {
        view,
        nodes,
        digest,
        blockers,
    })
}

/// Authenticated shadow output from the existing subscription compiler, with a
/// stable comparison digest. It does not switch the existing subscription address.
pub async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user): Path<i64>,
) -> ApiResult<Json<Value>> {
    let administrator = crate::secret_access::require_reveal(&state, &headers).await?;
    let mut tx = apsp_traffic::read_snapshot(&state.pool).await?;
    let prepared = prepared(&state, &mut tx, user).await?;
    for server in crate::user_limits::servers(&mut tx, user).await? {
        crate::control_center::require_server(&state, &headers, server, "proxy:write").await?;
    }
    tx.commit().await?;
    let mut audit = state.pool.begin().await?;
    crate::operations_workflows::event(
        &mut audit,
        Some(administrator),
        Some(user),
        "security_native_subscription_preview",
        json!({"credential_values_recorded":false}),
    )
    .await?;
    audit.commit().await?;
    Ok(Json(prepared.view))
}

/// Commits the checked identity/renderer transition and its current usage anchor in
/// one transaction. A concurrent or repeated submission cannot apply it twice.
pub async fn adopt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user): Path<i64>,
    Json(input): Json<Adopt>,
) -> ApiResult<Json<Value>> {
    let administrator = crate::secret_access::require_reveal(&state, &headers).await?;
    if !input.comparison_confirmed
        || !input.old_subscription_route_ready
        || input.compared_formats.is_empty()
        || input.compared_formats.len() > 3
        || input.expected_digest.len() != 64
        || !input
            .expected_digest
            .bytes()
            .all(|ch| ch.is_ascii_hexdigit())
    {
        return Err(ApiError::BadRequest(
            "请先比较实际客户端格式，并确认旧订阅入口已代理到 apsp".into(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout='5s'")
        .execute(&mut *tx)
        .await?;
    apsp_traffic::business::lock_publication(&mut tx).await?;
    crate::locks::DEVICE_EXCHANGE.lock(&mut tx).await?;
    // Acquire the ledger lock before the user row: ingest takes a ledger lock and
    // then a foreign-key KEY SHARE lock on users, so the reverse order deadlocks.
    sqlx::query("LOCK TABLE usage_records,singbox_usage_daily,singbox_nexus_usage_deliveries IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *tx).await?;
    crate::proxy_users::lock_user(&mut tx, user).await?;
    let servers = crate::user_limits::servers(&mut tx, user).await?;
    for &server in &servers {
        crate::control_center::require_server(&state, &headers, server, "proxy:write").await?;
    }
    if let Some((digest, at)) = sqlx::query_as::<_, (String, i64)>(
        "SELECT preview_digest,adopted_at FROM singbox_native_adoptions WHERE user_id=$1",
    )
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?
    {
        if digest != input.expected_digest {
            return Err(ApiError::Conflict("此用户已按另一份预览接管".into()));
        }
        tx.commit().await?;
        return Ok(Json(
            json!({"user_id":user,"adopted":true,"adopted_at":at,"already_applied":true}),
        ));
    }
    // Ingest and the daily-rollup trigger must finish before capturing the anchor;
    // no late INSERT can slip between the meter read and the adoption marker.
    let prepared = prepared(&state, &mut tx, user).await?;
    if prepared.digest != input.expected_digest {
        return Err(ApiError::Conflict(
            "身份、套餐、策略或已应用配置已变化，请重新比较预览".into(),
        ));
    }
    if !prepared.blockers.is_empty() {
        return Err(ApiError::Conflict(prepared.blockers.join("；")));
    }
    if input.compared_formats.iter().any(|format| {
        prepared.view["formats"].get(format).is_none()
            && !(format == "blocked" && prepared.view["blocked"] == true)
    }) {
        return Err(ApiError::BadRequest(
            "已比较格式必须来自当前可生成的原生订阅".into(),
        ));
    }
    let now = apsp_protocol::now_timestamp();
    let inserted = sqlx::query("INSERT INTO singbox_native_adoptions(user_id,assignment_id,adopted_at,cycle_start,next_reset,used_bytes,ledger_bytes,preview_digest) SELECT $1,p.assignment_id,$2,e.cycle_start,e.next_reset,CASE WHEN meter.user_id IS NOT NULL THEN meter.upload+meter.download+GREATEST(COALESCE(singbox_usage_window($1,e.cycle_start,e.next_reset),0)-COALESCE((SELECT SUM(d.uplink+d.downlink) FROM singbox_nexus_usage_deliveries d WHERE d.user_id=$1 AND d.period_end>e.cycle_start AND d.period_end<=e.next_reset),0),0) ELSE COALESCE(singbox_usage_window($1,e.cycle_start,e.next_reset),0) END,COALESCE(singbox_usage_charged($1,e.cycle_start,e.next_reset),0),$3 FROM users u LEFT JOIN singbox_user_packages p ON p.user_id=u.id JOIN singbox_entitlements($2) e ON e.user_id=u.id LEFT JOIN singbox_nexus_metering meter ON meter.user_id=u.id WHERE u.id=$1")
        .bind(user).bind(now).bind(&prepared.digest).execute(&mut *tx).await?.rows_affected();
    if inserted != 1 {
        return Err(ApiError::Conflict("当前套餐与周期不可接管".into()));
    }
    // The native access rows reuse exactly what an applied runtime already accepts.
    for node in &prepared.nodes {
        for access in node.users.iter().filter(|access| access.user_id == user) {
            sqlx::query("INSERT INTO accesses(user_id,node_id,uuid,stat_name,credential) VALUES($1,$2,$3,$4,$5) ON CONFLICT(user_id,node_id) DO UPDATE SET uuid=EXCLUDED.uuid,credential=EXCLUDED.credential,stat_name=EXCLUDED.stat_name")
                .bind(user).bind(node.id).bind(access.uuid).bind(apsp_compiler::stat_name(user,node.id))
                .bind(&access.credential).execute(&mut *tx).await?;
        }
    }
    crate::policies::sync_users(&mut tx, &[user]).await?;
    apsp_traffic::business::mark_dirty(&mut tx, &servers).await?;
    crate::operations_workflows::event(&mut tx, Some(administrator), Some(user), "native_subscription_adopted", json!({"digest":prepared.digest,"compared_formats":input.compared_formats,"server_ids":servers,"credential_values_recorded":false})).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"user_id":user,"adopted":true,"adopted_at":now,"already_applied":false}),
    ))
}

/// The bridge can stop only after every identity and non-staged member has been
/// adopted and all in-flight exchanges have completed. Historical rows stay intact.
pub async fn retire(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let administrator = crate::control_center::require_owner(&state, &headers).await?;
    crate::control_center::require_recent_proof(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    apsp_traffic::business::lock_publication(&mut tx).await?;
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM singbox_legacy_served_users() served JOIN users u ON u.id=served.user_id WHERE u.deleted_at IS NULL")
        .fetch_one(&mut *tx).await?;
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_usage_deliveries WHERE delivered_at IS NULL) OR EXISTS(SELECT 1 FROM singbox_nexus_bridge WHERE pending_generation IS NOT NULL) OR EXISTS(SELECT 1 FROM singbox_native_adoptions WHERE legacy_retired_at IS NULL)")
        .fetch_one(&mut *tx).await?;
    if remaining > 0 || busy {
        return Err(ApiError::Conflict(format!(
            "仍有 {remaining} 位用户依赖兼容通道，或有交换未完成"
        )));
    }
    sqlx::query("UPDATE singbox_nexus_bridge SET enabled=FALSE WHERE enabled")
        .execute(&mut *tx)
        .await?;
    crate::operations_workflows::event(
        &mut tx,
        Some(administrator),
        None,
        "native_control_plane_retired_bridge",
        json!({"historical_rows_retained":true}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"retired":true,"stop_compatibility_service":true,"historical_rows_retained":true}),
    ))
}
