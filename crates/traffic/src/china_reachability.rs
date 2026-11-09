//! Whether a node was recently measured as blocked from China, which the Nexus cutover
//! checks before switching a host (ADR 0128 section 5.5).
//!
//! Observability measures reachability and comes after traffic in the dependency order,
//! so traffic declares this, observability answers and the application connects it.

use apsp_panel_host::ports::{BoxFuture, Port};
use sqlx::PgPool;

use crate::error::{ApiError, ApiResult};

/// The latest measurement of a node's public endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurement {
    /// Whether the state is one of the debounced blocked states.
    pub blocked: bool,
    pub checked_at: Option<i64>,
}

pub trait ChinaReachability: Send + Sync {
    /// The latest measurement of a preserved Nexus node, or `None` when its endpoint is
    /// unknown or was never measured.
    fn nexus_node<'a>(
        &'a self,
        pool: &'a PgPool,
        node: i64,
    ) -> BoxFuture<'a, ApiResult<Option<Measurement>>>;
}

static REACHABILITY: Port<dyn ChinaReachability> = Port::new();

pub fn install(reachability: &'static dyn ChinaReachability) {
    REACHABILITY.install(reachability);
}

pub async fn nexus_node(pool: &PgPool, node: i64) -> ApiResult<Option<Measurement>> {
    REACHABILITY
        .get()
        .ok_or_else(|| {
            ApiError::Conflict("中国可达性的读取接口未登记，不能确认主机是否被阻断".into())
        })?
        .nexus_node(pool, node)
        .await
}
