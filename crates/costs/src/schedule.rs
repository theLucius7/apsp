use apsp_panel_host::error::{ApiError, ApiResult};
use time::{Date, Duration, Month};

pub fn parse(value: &str) -> ApiResult<Date> {
    if value.len() != 10
        || value.as_bytes()[4] != b'-'
        || value.as_bytes()[7] != b'-'
        || !value
            .bytes()
            .enumerate()
            .all(|(i, c)| [4, 7].contains(&i) || c.is_ascii_digit())
    {
        return Err(ApiError::BadRequest("日期须为 YYYY-MM-DD".into()));
    }
    let year = value[..4]
        .parse::<i32>()
        .map_err(|_| ApiError::BadRequest("年份无效".into()))?;
    let month = value[5..7]
        .parse::<u8>()
        .ok()
        .and_then(|m| Month::try_from(m).ok())
        .ok_or_else(|| ApiError::BadRequest("月份无效".into()))?;
    let day = value[8..10]
        .parse::<u8>()
        .map_err(|_| ApiError::BadRequest("日期无效".into()))?;
    if !(1900..=9998).contains(&year) {
        return Err(ApiError::BadRequest("年份须在 1900–9998 之间".into()));
    }
    Date::from_calendar_date(year, month, day)
        .map_err(|_| ApiError::BadRequest("日期不存在".into()))
}
pub fn occurrence(anchor: Date, cycle: &str, frequency: i32, index: i64) -> Option<Date> {
    let n = index.checked_mul(i64::from(frequency))?;
    if n < 0 {
        return None;
    }
    match cycle {
        "day" => anchor.checked_add(Duration::days(n)),
        "week" => anchor.checked_add(Duration::days(n.checked_mul(7)?)),
        "month" | "year" => {
            let months = n.checked_mul(if cycle == "year" { 12 } else { 1 })?;
            let total = i64::from(anchor.year())
                .checked_mul(12)?
                .checked_add(i64::from(u8::from(anchor.month())) - 1)?
                .checked_add(months)?;
            let year = i32::try_from(total.div_euclid(12)).ok()?;
            let month = Month::try_from((total.rem_euclid(12) + 1) as u8).ok()?;
            (1..=anchor.day())
                .rev()
                .find_map(|day| Date::from_calendar_date(year, month, day).ok())
        }
        _ => None,
    }
}
pub fn at_or_after(anchor: Date, cycle: &str, frequency: i32, from: Date) -> Option<(Date, i64)> {
    if frequency <= 0 {
        return None;
    }
    let rough = match cycle {
        "day" => (from - anchor).whole_days() / i64::from(frequency),
        "week" => (from - anchor).whole_days() / (i64::from(frequency) * 7),
        "month" | "year" => {
            ((i64::from(from.year()) - i64::from(anchor.year())) * 12
                + i64::from(u8::from(from.month()))
                - i64::from(u8::from(anchor.month())))
                / (i64::from(frequency) * if cycle == "year" { 12 } else { 1 })
        }
        _ => return None,
    }
    .max(0);
    for index in rough..=rough + 2 {
        let date = occurrence(anchor, cycle, frequency, index)?;
        if date >= from {
            return Some((date, index));
        }
    }
    None
}
pub fn format(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn month_end_and_leap_day_preserve_original_anchor() {
        let a = parse("2024-01-31").unwrap();
        assert_eq!(format(occurrence(a, "month", 1, 1).unwrap()), "2024-02-29");
        assert_eq!(format(occurrence(a, "month", 1, 2).unwrap()), "2024-03-31");
        let a = parse("2024-02-29").unwrap();
        assert_eq!(format(occurrence(a, "year", 1, 1).unwrap()), "2025-02-28");
        assert_eq!(format(occurrence(a, "year", 1, 4).unwrap()), "2028-02-29");
    }
    #[test]
    fn catchup_keeps_frequency_and_exact_boundary() {
        let a = parse("2020-01-31").unwrap();
        let (date, index) = at_or_after(a, "month", 3, parse("2026-02-01").unwrap()).unwrap();
        assert_eq!(format(date), "2026-04-30");
        assert_eq!(index, 25);
        assert_eq!(at_or_after(a, "month", 3, date).unwrap().0, date);
        assert!(parse("2026-02-29").is_err());
        assert!(parse("2026-2-01").is_err());
        assert!(parse("２０２６-01-01").is_err());
    }
}
