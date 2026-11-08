//! Whether a server's traffic runtime is configured and when it last carried traffic,
//! read before an intrusive diagnostic runs. Answered by the runtime's owner and
//! connected by the application.

use crate::{
    error::{ApiError, ApiResult},
    ports::{BoxFuture, Port},
};

/// Runtime evidence; silence does not establish idleness.
pub struct ActivityEvidence {
    pub configured: bool,
    pub last_positive_at: Option<i64>,
}

pub trait RuntimeActivity: Send + Sync {
    fn activity_on<'a>(
        &'a self,
        connection: &'a mut sqlx::PgConnection,
        server_id: i64,
        checked_at: i64,
    ) -> BoxFuture<'a, ApiResult<ActivityEvidence>>;
}

static ACTIVITY: Port<dyn RuntimeActivity> = Port::new();

pub fn install(activity: &'static dyn RuntimeActivity) {
    ACTIVITY.install(activity);
}

pub async fn runtime_activity_on(
    connection: &mut sqlx::PgConnection,
    server_id: i64,
    checked_at: i64,
) -> ApiResult<ActivityEvidence> {
    match ACTIVITY.get() {
        Some(activity) => {
            activity
                .activity_on(connection, server_id, checked_at)
                .await
        }
        None => Err(ApiError::Conflict(
            "运行时活动证据提供方未登记，不能判断当前负载".into(),
        )),
    }
}
