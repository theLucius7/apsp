use crate::error::ApiResult;

/// Whether the runtime is currently configured on a server and when it last carried traffic.
pub struct ActivityEvidence {
    pub configured: bool,
    pub last_positive_at: Option<i64>,
}

pub async fn runtime_activity_on(
    connection: &mut sqlx::PgConnection,
    server_id: i64,
    checked_at: i64,
) -> ApiResult<ActivityEvidence> {
    let configured = crate::settings::is_configured_on(&mut *connection, server_id).await?;
    let last_positive_at: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(period_end) FROM usage_records WHERE server_id=$1 AND (uplink>0 OR downlink>0) AND period_end<=$2",
    )
    .bind(server_id)
    .bind(checked_at.saturating_add(60))
    .fetch_one(&mut *connection)
    .await?;
    Ok(ActivityEvidence {
        configured,
        last_positive_at,
    })
}
