use super::*;
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, header},
    response::IntoResponse,
};
use std::collections::BTreeMap;
use time::{Date, Duration};

#[derive(Deserialize)]
pub(super) struct Range {
    from: String,
    to: String,
}
#[derive(Serialize)]
struct Due {
    id: i64,
    name: String,
    date: String,
    amount_units: String,
    currency: String,
    auto_renew: bool,
}
pub(super) fn dates(entry: &Subscription, from: Date, to: Date) -> ApiResult<Vec<Date>> {
    if entry.inactive {
        return Ok(Vec::new());
    }
    let next = schedule::parse(&entry.next_payment)?;
    let anchor = schedule::parse(&entry.anchor_payment)?;
    let end = entry
        .cancellation_date
        .as_deref()
        .map(schedule::parse)
        .transpose()?
        .map(|d| d.min(to))
        .unwrap_or(to);
    if !entry.auto_renew {
        return Ok(if next >= from && next <= end {
            vec![next]
        } else {
            vec![]
        });
    }
    let Some((mut date, mut index)) =
        schedule::at_or_after(anchor, &entry.cycle, entry.frequency, from.max(next))
    else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    while date <= end {
        result.push(date);
        if result.len() > 367 {
            return Err(bad("日历范围过大"));
        }
        index += 1;
        let Some(next) = schedule::occurrence(anchor, &entry.cycle, entry.frequency, index) else {
            break;
        };
        date = next;
    }
    Ok(result)
}
async fn collect(state: &AppState, range: &Range) -> ApiResult<Vec<Due>> {
    let from = schedule::parse(&range.from)?;
    let to = schedule::parse(&range.to)?;
    if to < from || (to - from).whole_days() > 366 {
        return Err(bad("日历范围须在 367 天以内"));
    }
    let rows: Vec<Subscription> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM expense_subscriptions WHERE NOT inactive ORDER BY id LIMIT 10000"
    ))
    .fetch_all(&state.pool)
    .await?;
    let mut events = Vec::new();
    for row in rows {
        for date in dates(&row, from, to)? {
            events.push(Due {
                id: row.id,
                name: row.name.clone(),
                date: schedule::format(date),
                amount_units: row.amount_units.clone(),
                currency: row.currency.clone(),
                auto_renew: row.auto_renew,
            });
            if events.len() > 20000 {
                return Err(bad("事件过多，请缩小日历范围"));
            }
        }
    }
    events.sort_by(|a, b| (&a.date, a.id).cmp(&(&b.date, b.id)));
    Ok(events)
}
pub(super) async fn calendar(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(range): Query<Range>,
) -> ApiResult<Json<Value>> {
    actor(&state, &headers, false).await?;
    Ok(Json(json!({"entries":collect(&state,&range).await?})))
}
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\r', "")
        .replace('\n', "\\n")
        .replace(';', "\\;")
        .replace(',', "\\,")
}
fn fold(line: &str) -> String {
    let mut result = String::new();
    let mut length = 0;
    for c in line.chars() {
        if length + c.len_utf8() > 73 {
            result.push_str("\r\n ");
            length = 1;
        }
        result.push(c);
        length += c.len_utf8();
    }
    result.push_str("\r\n");
    result
}
pub(super) async fn ical(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(range): Query<Range>,
) -> ApiResult<impl IntoResponse> {
    actor(&state, &headers, false).await?;
    let events = collect(&state, &range).await?;
    let mut output="BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//apsp//Expense Calendar//EN\r\nCALSCALE:GREGORIAN\r\n".to_owned();
    for event in events {
        let date = event.date.replace('-', "");
        let tomorrow = schedule::parse(&event.date)?
            .next_day()
            .ok_or_else(|| bad("日历日期超出范围"))?;
        output.push_str("BEGIN:VEVENT\r\n");
        output.push_str(&fold(&format!(
            "UID:expense-{}-{}@apsp.invalid",
            event.id, date
        )));
        output.push_str(&fold(&format!("DTSTAMP:{date}T000000Z")));
        output.push_str(&fold(&format!("DTSTART;VALUE=DATE:{date}")));
        output.push_str(&fold(&format!(
            "DTEND;VALUE=DATE:{}",
            schedule::format(tomorrow).replace('-', "")
        )));
        output.push_str(&fold(&format!("SUMMARY:{}", escape(&event.name))));
        let units = units(&event.amount_units)?;
        output.push_str(&fold(&format!(
            "DESCRIPTION:{} {}.{:04}\\n{}",
            event.currency,
            units / 10000,
            units % 10000,
            if event.auto_renew {
                "自动续费计划；并非付款凭证"
            } else {
                "手动续费"
            }
        )));
        output.push_str("END:VEVENT\r\n");
    }
    output.push_str("END:VCALENDAR\r\n");
    Ok((
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=apsp-expenses.ics",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        output,
    ))
}
pub(super) async fn summary(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    actor(&state, &headers, false).await?;
    let settings=sqlx::query("SELECT base_currency,budget_units::text,(to_timestamp($1) AT TIME ZONE timezone)::date::text AS today FROM expense_settings WHERE singleton").bind(now_timestamp()).fetch_one(&state.pool).await?;
    let today: String = settings.try_get("today")?;
    let start = schedule::parse(&today)?;
    let end = start
        .checked_add(Duration::days(364))
        .ok_or_else(|| bad("日期超出范围"))?;
    let rows: Vec<Subscription> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM expense_subscriptions ORDER BY id LIMIT 10000"
    ))
    .fetch_all(&state.pool)
    .await?;
    let mut totals: BTreeMap<String, i128> = BTreeMap::new();
    let mut categories: BTreeMap<Option<i64>, BTreeMap<String, i128>> = BTreeMap::new();
    for row in &rows {
        let count = dates(row, start, end)?.len();
        let value = i128::from(units(&row.amount_units)?) * count as i128;
        *totals.entry(row.currency.clone()).or_default() += value;
        *categories
            .entry(row.category_id)
            .or_default()
            .entry(row.currency.clone())
            .or_default() += value;
    }
    let exchange = serde_json::to_value(apsp_panel_host::exchange::current(&state.pool).await?)
        .map_err(anyhow::Error::from)?;
    let base: String = settings.try_get("base_currency")?;
    let rates = &exchange["rates"];
    let rate = |code: &str| {
        if code == "CNY" {
            Some(1.0)
        } else {
            rates[code].as_f64().filter(|r| r.is_finite() && *r > 0.0)
        }
    };
    let mut converted = 0.0;
    let mut missing = Vec::new();
    for (code, value) in &totals {
        if code == &base {
            converted += *value as f64 / 10000.0;
        } else if let Some((source, target)) = rate(code).zip(rate(&base)) {
            converted += *value as f64 / 10000.0 / source * target;
        } else {
            missing.push(code.clone());
        }
    }
    let totals: BTreeMap<_, _> = totals
        .iter()
        .map(|(k, v)| (k.clone(), v.to_string()))
        .collect();
    let categories:Vec<Value>=categories.into_iter().map(|(id,values)|json!({"category_id":id,"totals":values.into_iter().map(|(k,v)|(k,v.to_string())).collect::<BTreeMap<_,_>>()})).collect();
    Ok(Json(
        json!({"from":today,"to":schedule::format(end),"active":rows.iter().filter(|r|!r.inactive).count(),"inactive":rows.iter().filter(|r|r.inactive).count(),"totals_units":totals,"categories":categories,"base_currency":base,"budget_units":settings.try_get::<Option<String>,_>("budget_units")?,"converted_yearly":if missing.is_empty(){Some(converted)}else{None},"missing_currencies":missing,"exchange":exchange,"estimate":true}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ical_folding_preserves_unicode_and_prevents_new_properties() {
        let escaped = escape("标题\nBEGIN:VEVENT,注释;\\");
        assert!(!escaped.contains('\n'));
        let line = fold(&format!("SUMMARY:{}", escaped.repeat(20)));
        assert!(line.split("\r\n").all(|l| l.len() <= 75));
        assert_eq!(
            line.replace("\r\n ", "").trim_end_matches("\r\n"),
            format!("SUMMARY:{}", escaped.repeat(20))
        );
    }
}
