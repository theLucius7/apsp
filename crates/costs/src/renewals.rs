//! Expiries that other owners observe, listed in the ledger for reference only:
//! they have no price and are never renewed. Answered by those owners and
//! connected by the application.

use apsp_panel_host::{
    AppState,
    error::ApiResult,
    ports::{BoxFuture, Port},
};
use serde::Serialize;

/// One externally observed expiry, for example a proxy subscription source.
#[derive(Clone, Serialize)]
pub struct RenewalReference {
    /// Kind of reference, stable for clients (for example `proxy_source`).
    pub kind: &'static str,
    pub id: i64,
    pub name: String,
    pub expires_at: Option<i64>,
    pub used_bytes: Option<i64>,
    pub total_bytes: Option<i64>,
    pub reported_at: Option<i64>,
    /// Hash path of the page that manages the referenced object.
    pub path: String,
    /// Capability the caller needs before the reference is shown.
    #[serde(skip)]
    pub capability: &'static str,
}

pub trait RenewalSources: Send + Sync {
    fn renewal_references<'a>(
        &'a self,
        state: &'a AppState,
    ) -> BoxFuture<'a, ApiResult<Vec<RenewalReference>>>;
}

static SOURCES: Port<dyn RenewalSources> = Port::new();

pub fn install(sources: &'static dyn RenewalSources) {
    SOURCES.install(sources);
}

pub async fn renewal_references(state: &AppState) -> ApiResult<Vec<RenewalReference>> {
    match SOURCES.get() {
        Some(sources) => sources.renewal_references(state).await,
        None => Ok(Vec::new()),
    }
}
