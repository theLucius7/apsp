use crate::{
    AppState, business,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    expected_revision: i64,
    paused: bool,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_capability(&state, &headers, "proxy:read").await?;
    let value = sqlx::query_scalar("SELECT jsonb_build_object('paused',COALESCE(c.paused,false),'revision',COALESCE(c.revision,0),'updated_at',c.updated_at) FROM users u LEFT JOIN singbox_user_controls c ON c.user_id=u.id WHERE u.id=$1 AND u.deleted_at IS NULL")
        .bind(id).fetch_optional(&state.pool).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(value))
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(request): Json<Request>,
) -> ApiResult<Json<Value>> {
    let administrator =
        crate::control_center::require_capability(&state, &headers, "proxy:write").await?;
    crate::control_center::require_recent_proof(&state, &headers).await?;
    if !(0..business::MAX_SAFE_INTEGER).contains(&request.expected_revision) {
        return Err(ApiError::BadRequest("配置版本无效".into()));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    business::lock_user(&mut tx, id).await?;
    let hosts = crate::user_limits::servers(&mut tx, id).await?;
    for host in &hosts {
        crate::control_center::require_server(&state, &headers, *host, "proxy:write").await?;
    }
    let previous: Option<(bool, i64)> = sqlx::query_as(
        "SELECT paused,revision FROM singbox_user_controls WHERE user_id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if previous.map_or(0, |(_, revision)| revision) != request.expected_revision {
        return Err(ApiError::Conflict(
            "状态已被其他操作修改，请刷新后重试".into(),
        ));
    }
    let now = apsp_protocol::now_timestamp();
    sqlx::query("INSERT INTO singbox_user_controls(user_id,paused,revision,updated_at) VALUES($1,$2,$3,$4) ON CONFLICT(user_id) DO UPDATE SET paused=EXCLUDED.paused,revision=EXCLUDED.revision,updated_at=EXCLUDED.updated_at")
        .bind(id).bind(request.paused).bind(request.expected_revision+1).bind(now).execute(&mut *tx).await?;
    business::mark_dirty(&mut tx, &hosts).await?;
    sqlx::query("INSERT INTO singbox_operation_events(administrator_id,user_id,action,detail,created_at) VALUES($1,$2,'user_control_updated',$3,$4)")
        .bind(administrator).bind(id).bind(json!({"paused":request.paused,"previous_paused":previous.is_some_and(|(paused,_)|paused),"server_ids":hosts})).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"paused":request.paused,"revision":request.expected_revision+1,"updated_at":now}),
    ))
}

#[cfg(test)]
mod tests {
    #[sqlx::test(migrations = "../../../crates/panel/migrations")]
    async fn pause_preserves_identity_and_restores_unmetered_access(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<()> {
        let id: i64 = sqlx::query_scalar("INSERT INTO users(name,subscription_token) VALUES('TEST_ONLY pause','TEST_ONLY retained') RETURNING id").fetch_one(&pool).await?;
        sqlx::query(
            "INSERT INTO singbox_user_controls(user_id,paused,updated_at) VALUES($1,true,1)",
        )
        .bind(id)
        .execute(&pool)
        .await?;
        let result: (String, bool) =
            sqlx::query_as("SELECT status,allowed FROM singbox_entitlements(100) WHERE user_id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await?;
        assert_eq!(result, ("paused".into(), false));
        sqlx::query("UPDATE singbox_user_controls SET paused=false WHERE user_id=$1")
            .bind(id)
            .execute(&pool)
            .await?;
        let restored: bool =
            sqlx::query_scalar("SELECT allowed FROM singbox_entitlements(100) WHERE user_id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await?;
        assert!(restored);
        let token: String = sqlx::query_scalar("SELECT subscription_token FROM users WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
        assert_eq!(token, "TEST_ONLY retained");
        Ok(())
    }
}
