//! What the traffic service needs from proxy-user access (ADR 0128 section 5.5): the
//! users' rate and connection limits in a compiled server configuration, the device
//! limits on preserved Nexus hosts and the proxy operation log.
//!
//! Access comes after traffic in the dependency order, so traffic declares this, access
//! answers and the application connects it. Without a connection the caller's
//! transaction fails, so a configuration is never published without the users' limits
//! and an operation is never left out of the log.

use apsp_panel_host::ports::{BoxFuture, Port};
use serde_json::Value;
use sqlx::{Postgres, Transaction};

use crate::error::{ApiError, ApiResult};

pub trait Users: Send + Sync {
    /// The compiled configuration with every user's rate and connection limits applied.
    fn apply_limits<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        config: &'a str,
    ) -> BoxFuture<'a, anyhow::Result<String>>;

    /// Members with a device limit on a preserved Nexus host, and whether the bridge has
    /// recently delivered the limits at all.
    fn nexus_device_limits<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        now: i64,
    ) -> BoxFuture<'a, anyhow::Result<(i64, bool)>>;

    /// Adds an administrator or user action to the proxy operation log.
    fn record_event<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        administrator: Option<i64>,
        user: Option<i64>,
        action: &'a str,
        detail: Value,
    ) -> BoxFuture<'a, ApiResult<()>>;
}

static USERS: Port<dyn Users> = Port::new();

pub fn install(users: &'static dyn Users) {
    USERS.install(users);
}

fn required() -> ApiResult<&'static dyn Users> {
    USERS.get().ok_or_else(|| {
        ApiError::Conflict("代理用户的限速、设备数与运维记录接口未登记，不能发布或记录".into())
    })
}

pub async fn apply_limits(
    tx: &mut Transaction<'_, Postgres>,
    config: &str,
) -> anyhow::Result<String> {
    required()?.apply_limits(tx, config).await
}

pub async fn nexus_device_limits(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    now: i64,
) -> anyhow::Result<(i64, bool)> {
    required()?.nexus_device_limits(tx, server, now).await
}

pub async fn record_event(
    tx: &mut Transaction<'_, Postgres>,
    administrator: Option<i64>,
    user: Option<i64>,
    action: &str,
    detail: Value,
) -> ApiResult<()> {
    required()?
        .record_event(tx, administrator, user, action, detail)
        .await
}

/// For this crate's own tests: users without limits, devices without device limits and
/// an operation log that keeps nothing. The application connects the real answers.
#[cfg(test)]
pub(crate) fn install_without_users() {
    struct WithoutUsers;
    impl Users for WithoutUsers {
        fn apply_limits<'a, 't>(
            &'a self,
            _tx: &'a mut Transaction<'t, Postgres>,
            config: &'a str,
        ) -> BoxFuture<'a, anyhow::Result<String>> {
            Box::pin(async move { Ok(config.to_owned()) })
        }

        fn nexus_device_limits<'a, 't>(
            &'a self,
            _tx: &'a mut Transaction<'t, Postgres>,
            _server: i64,
            _now: i64,
        ) -> BoxFuture<'a, anyhow::Result<(i64, bool)>> {
            Box::pin(async { Ok((0, true)) })
        }

        fn record_event<'a, 't>(
            &'a self,
            _tx: &'a mut Transaction<'t, Postgres>,
            _administrator: Option<i64>,
            _user: Option<i64>,
            _action: &'a str,
            _detail: Value,
        ) -> BoxFuture<'a, ApiResult<()>> {
            Box::pin(async { Ok(()) })
        }
    }
    static WITHOUT_USERS: WithoutUsers = WithoutUsers;
    install(&WITHOUT_USERS);
}
