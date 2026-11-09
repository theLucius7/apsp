//! Expiries that other owners observe, listed in the ledger for reference only:
//! they have no price and are never renewed. External proxy sources report theirs to
//! the traffic service, which comes before costs in the dependency order.

use apsp_panel_host::{AppState, error::ApiResult};
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

pub async fn renewal_references(state: &AppState) -> ApiResult<Vec<RenewalReference>> {
    Ok(
        apsp_plugin_singbox::subscription_sources::renewal_references(state)
            .await?
            .into_iter()
            .map(|source| RenewalReference {
                kind: source.kind,
                id: source.id,
                name: source.name,
                expires_at: source.expires_at,
                used_bytes: source.used_bytes,
                total_bytes: source.total_bytes,
                reported_at: source.reported_at,
                path: source.path,
                capability: source.capability,
            })
            .collect(),
    )
}
