//! Provider-reported expiry of subscription sources for the costs and renewals
//! ledger (ADR 0097). The values come from `subscription-userinfo` and are
//! shown for reference only; the ledger never prices or renews a source.
use crate::{AppState, error::ApiResult};
use apsp_panel_host::plugin_api::RenewalReference;
use sqlx::Row;

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
                path: "/plugins/sing-box/nodes?view=sources".into(),
                capability: "proxy:read",
            })
        })
        .collect()
}
