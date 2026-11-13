//! Provider-reported expiry of subscription sources for the costs and renewals
//! ledger (ADR 0097). The values come from `subscription-userinfo` and are
//! shown for reference only; the ledger never prices or renews a source.
use crate::{AppState, error::ApiResult};
use serde::Serialize;
use sqlx::Row;

/// Provider-reported expiry of one subscription source and where it is managed, listed in
/// the costs ledger for reference only.
#[derive(Clone, Serialize)]
pub struct RenewalReference {
    /// Kind of reference, stable for clients (`proxy_source`).
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
    let rows = sqlx::query(
        "SELECT id,name,(traffic->>'expire')::bigint AS expire,\
         (traffic->>'upload')::bigint+(traffic->>'download')::bigint AS used,\
         (traffic->>'total')::bigint AS total,(traffic->>'updated_at')::bigint AS reported \
         FROM singbox_ordered_subscription_sources \
         WHERE deleted_at IS NULL AND NOT archived AND jsonb_typeof(traffic->'expire')='number' \
         ORDER BY id LIMIT 1000",
    )
    .fetch_all(&state.pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(RenewalReference {
                kind: "proxy_source",
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                expires_at: row.try_get("expire")?,
                used_bytes: row.try_get("used")?,
                total_bytes: row.try_get("total")?,
                reported_at: row.try_get("reported")?,
                path: "/nodes?view=sources".into(),
                capability: "proxy:read",
            })
        })
        .collect()
}
