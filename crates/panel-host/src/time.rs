//! Calendar arithmetic in UTC on the proleptic Gregorian calendar: civil dates to day
//! numbers (days since 1970-01-01) and Unix timestamps to ISO 8601 text. Certificates,
//! exchange rate dates, provider signatures and manifests all read and write dates here.

/// The last second written as ISO 8601 text: 9999-12-31T23:59:59Z.
const LAST_SECOND: i64 = 253_402_300_799;

fn leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The day number of a civil date: days since 1970-01-01, negative before it. None when the
/// month is outside 1–12 or the month does not have the day.
pub fn day_number(year: i64, month: i64, day: i64) -> Option<i64> {
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year(year) => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=last).contains(&day) {
        return None;
    }
    // Days from civil (H. Hinnant): years start in March, so the leap day ends a year.
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

/// The civil date (year, month, day) of a day number.
fn civil(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// `timestamp` as ISO 8601 UTC text, `YYYY-MM-DDTHH:MM:SSZ`, clamped to the years
/// 1970–9999.
pub fn iso8601(timestamp: i64) -> String {
    let timestamp = timestamp.clamp(0, LAST_SECOND);
    let (year, month, day) = civil(timestamp.div_euclid(86_400));
    let second = timestamp.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second / 3600,
        second % 3600 / 60,
        second % 60
    )
}

#[cfg(test)]
mod tests {
    use super::{LAST_SECOND, civil, day_number, iso8601};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn day_numbers_and_dates_agree() {
        assert_eq!(day_number(1970, 1, 1), Some(0));
        assert_eq!(day_number(1969, 12, 31), Some(-1));
        assert_eq!(day_number(2000, 2, 29), Some(11_016));
        assert_eq!(day_number(2026, 10, 1), Some(20_727));
        assert_eq!(day_number(9999, 12, 31), Some(2_932_896));
        for invalid in [
            (2025, 2, 29),
            (1900, 2, 29),
            (2026, 4, 31),
            (2026, 13, 1),
            (2026, 0, 1),
        ] {
            assert_eq!(
                day_number(invalid.0, invalid.1, invalid.2),
                None,
                "{invalid:?}"
            );
        }
        // Every day from 1600 to 2400 round-trips, and consecutive days differ by one.
        let mut previous = day_number(1599, 12, 31).unwrap();
        for days in previous + 1..=day_number(2400, 12, 31).unwrap() {
            let (year, month, day) = civil(days);
            assert_eq!(day_number(year, month, day), Some(days));
            assert_eq!(days, previous + 1);
            previous = days;
        }
    }

    #[test]
    fn iso_text_matches_http_dates() {
        assert_eq!(iso8601(1_790_812_800), "2026-10-01T00:00:00Z");
        assert_eq!(iso8601(-5), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(i64::MAX), "9999-12-31T23:59:59Z");
        let months = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let mut timestamp = 0;
        while timestamp <= LAST_SECOND {
            // "Sun, 06 Nov 1994 08:49:37 GMT"
            let http = httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(timestamp as u64));
            let parts: Vec<_> = http.split_whitespace().collect();
            let month = months.iter().position(|name| *name == parts[2]).unwrap() + 1;
            let expected = format!("{}-{month:02}-{}T{}Z", parts[3], parts[1], parts[4]);
            assert_eq!(iso8601(timestamp), expected, "{timestamp}");
            timestamp += 86_400 * 13 + 3_607;
        }
    }
}
