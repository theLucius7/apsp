//! What resources needs from the traffic service (ADR 0128 section 5.5): the revision of
//! the configuration traffic last published to each server, shown with the server, and the
//! runtimes enabled on a server with the state of their deployments, which a collection
//! migration leaves behind. Traffic keeps both in its own tables; resources declares this,
//! traffic answers and the application connects it.

use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashMap;

use crate::{
    error::{ApiError, ApiResult},
    ports::{BoxFuture, Port},
    servers::Server,
};

/// Traffic's state on one server that a collection migration does not move.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimeReferences {
    /// The runtimes enabled on the server, by the names the console shows ("sing-box").
    pub enabled: Vec<String>,
    /// The deployment state of each module: its target and applied revision and health.
    pub deployments: Vec<Value>,
}

pub trait ServerRuntime: Send + Sync {
    /// The revision last published to each of `servers` that has published one; the others
    /// are at 0.
    fn publication_revisions<'a>(
        &'a self,
        pool: &'a PgPool,
        servers: &'a [i64],
    ) -> BoxFuture<'a, ApiResult<HashMap<i64, i64>>>;

    /// The runtimes enabled on `server` and the state of their deployments.
    fn references<'a>(
        &'a self,
        pool: &'a PgPool,
        server: i64,
    ) -> BoxFuture<'a, ApiResult<RuntimeReferences>>;
}

static RUNTIME: Port<dyn ServerRuntime> = Port::new();

pub fn install(runtime: &'static dyn ServerRuntime) {
    RUNTIME.install(runtime);
}

fn required() -> ApiResult<&'static dyn ServerRuntime> {
    RUNTIME.get().ok_or_else(|| {
        ApiError::Conflict("服务器运行时的提供方未登记，不能显示服务器的配置版本".into())
    })
}

/// Fills each server's `manifest_rev`, the revision last published to it.
pub async fn attach_revisions(pool: &PgPool, servers: &mut [Server]) -> ApiResult<()> {
    let ids: Vec<i64> = servers.iter().map(|server| server.id).collect();
    let revisions = required()?.publication_revisions(pool, &ids).await?;
    for server in servers {
        server.manifest_rev = revisions.get(&server.id).copied().unwrap_or(0);
    }
    Ok(())
}

pub async fn references(pool: &PgPool, server: i64) -> ApiResult<RuntimeReferences> {
    required()?.references(pool, server).await
}
