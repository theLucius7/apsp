//! Servers enter the ledger from their asset settings and are edited there
//! (ADR 0087, 0096). The ledger only reads them: price, currency, billing cycle and
//! expiry stay owned by the server, and expiry reminders stay in monitoring.
use super::*;
use apsp_panel_host::control_center::Principal;
use axum::{Json, extract::State};
use time::{Date, Duration};

#[derive(Clone, Serialize)]
pub(crate) struct ServerRenewal {
    pub id: i64,
    pub name: String,
    /// Ten-thousandths of the currency unit, like manual items; `None` when no
    /// price was recorded.
    pub amount_units: Option<String>,
    pub currency: String,
    /// Zero means a one-off purchase.
    pub cycle_days: i32,
    pub next_renewal: String,
    pub auto_renewal: bool,
}

pub(crate) async fn load(state: &AppState, principal: &Principal) -> ApiResult<Vec<ServerRenewal>> {
    if !principal.allows("servers:read") {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT s.id,s.name,s.asset_settings->>'price' AS price,\
         COALESCE(s.asset_settings->>'currency','CNY') AS currency,\
         COALESCE((s.asset_settings->>'billing_cycle')::int,0) AS cycle_days,\
         COALESCE((s.asset_settings->>'auto_renewal')::boolean,FALSE) AS auto_renewal,\
         (to_timestamp((s.asset_settings->>'expires_at')::bigint) AT TIME ZONE r.timezone)::date::text AS next_renewal \
         FROM servers s CROSS JOIN expense_settings r \
         WHERE r.singleton AND s.deleted_at IS NULL AND jsonb_typeof(s.asset_settings->'expires_at')='number' \
         ORDER BY s.id LIMIT 10000",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut servers = Vec::new();
    for row in rows {
        let id: i64 = row.try_get("id")?;
        if !principal.allows_server(id) {
            continue;
        }
        let price: Option<String> = row.try_get("price")?;
        servers.push(ServerRenewal {
            id,
            name: row.try_get("name")?,
            amount_units: price.as_deref().and_then(price_units),
            currency: row.try_get("currency")?,
            cycle_days: row.try_get("cycle_days")?,
            next_renewal: row.try_get("next_renewal")?,
            auto_renewal: row.try_get("auto_renewal")?,
        });
    }
    Ok(servers)
}

/// Server prices are decimal strings with at most two fraction digits.
fn price_units(price: &str) -> Option<String> {
    let (whole, fraction) = price.split_once('.').unwrap_or((price, ""));
    if whole.is_empty()
        || fraction.len() > 2
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole: i64 = whole.parse().ok()?;
    let fraction: i64 = format!("{fraction:0<2}").parse().ok()?;
    whole
        .checked_mul(10_000)?
        .checked_add(fraction * 100)
        .map(|units| units.to_string())
}

/// Renewal dates in `[from, to]`, assuming the server keeps being renewed every
/// billing cycle after its recorded expiry.
pub(crate) fn dates(server: &ServerRenewal, from: Date, to: Date) -> ApiResult<Vec<Date>> {
    let first = schedule::parse(&server.next_renewal)?;
    if server.cycle_days <= 0 {
        return Ok(if (from..=to).contains(&first) {
            vec![first]
        } else {
            Vec::new()
        });
    }
    let cycle = i64::from(server.cycle_days);
    let mut date = first;
    if date < from {
        let skipped = ((from - date).whole_days() + cycle - 1) / cycle;
        date = date
            .checked_add(Duration::days(skipped * cycle))
            .ok_or_else(|| bad("日期超出范围"))?;
    }
    let mut result = Vec::new();
    while date <= to {
        result.push(date);
        if result.len() > 367 {
            return Err(bad("日历范围过大"));
        }
        let Some(next) = date.checked_add(Duration::days(cycle)) else {
            break;
        };
        date = next;
    }
    Ok(result)
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let principal = principal(&state, &headers, false).await?;
    Ok(Json(json!({"entries":load(&state,&principal).await?})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(next: &str, cycle_days: i32) -> ServerRenewal {
        ServerRenewal {
            id: 1,
            name: "server".into(),
            amount_units: None,
            currency: "USD".into(),
            cycle_days,
            next_renewal: next.into(),
            auto_renewal: false,
        }
    }

    #[test]
    fn prices_convert_exactly_to_ledger_units() {
        assert_eq!(price_units("12.5").as_deref(), Some("125000"));
        assert_eq!(price_units("0.01").as_deref(), Some("100"));
        assert_eq!(price_units("7").as_deref(), Some("70000"));
        for invalid in ["", ".5", "1.234", "-1", "1e3", " 1"] {
            assert_eq!(price_units(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn renewals_repeat_by_cycle_from_the_recorded_expiry() {
        let from = schedule::parse("2026-01-01").unwrap();
        let to = schedule::parse("2026-03-31").unwrap();
        let dates = |s: &ServerRenewal| {
            dates(s, from, to)
                .unwrap()
                .into_iter()
                .map(schedule::format)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            dates(&server("2025-12-20", 30)),
            ["2026-01-19", "2026-02-18", "2026-03-20"]
        );
        assert_eq!(dates(&server("2026-02-01", 0)), ["2026-02-01"]);
        assert!(dates(&server("2025-02-01", 0)).is_empty());
    }
}

/// A plugin-observed expiry (for example a proxy subscription source) shown
/// for reference only: it has no price and the ledger never renews it.
#[derive(Clone, Serialize)]
pub(crate) struct Reference {
    #[serde(flatten)]
    pub source: apsp_panel_host::plugin_api::RenewalReference,
    /// The expiry as a date in the ledger time zone.
    pub expires_on: Option<String>,
}

pub(crate) async fn references(
    state: &AppState,
    principal: &Principal,
) -> ApiResult<Vec<Reference>> {
    let sources: Vec<_> = apsp_panel_host::plugin_api::renewal_references(state)
        .await?
        .into_iter()
        .filter(|r| principal.allows(r.capability))
        .collect();
    let at: Vec<Option<i64>> = sources.iter().map(|r| r.expires_at).collect();
    let dates: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT CASE WHEN e IS NULL THEN NULL ELSE (to_timestamp(e) AT TIME ZONE r.timezone)::date::text END \
         FROM unnest($1::bigint[]) WITH ORDINALITY AS t(e,n) CROSS JOIN expense_settings r WHERE r.singleton ORDER BY n",
    )
    .bind(&at)
    .fetch_all(&state.pool)
    .await?;
    Ok(sources
        .into_iter()
        .zip(dates)
        .map(|(source, expires_on)| Reference { source, expires_on })
        .collect())
}

pub(super) async fn reference_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let principal = principal(&state, &headers, false).await?;
    Ok(Json(
        json!({"entries":references(&state,&principal).await?}),
    ))
}
