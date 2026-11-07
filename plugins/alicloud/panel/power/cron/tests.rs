use super::Cron;

#[test]
fn expressions_and_calendar_boundaries() {
    // 2026-11-06 00:00 UTC, Friday 08:00 at UTC+08.
    let now = 1_793_923_200;
    let weekdays = Cron::parse("0 8 * * 1-5").unwrap();
    assert_eq!(weekdays.occurrence(now, 480, false), Some(now));
    assert_eq!(weekdays.occurrence(now, 480, true), Some(now + 3 * 86400));
    assert_eq!(
        Cron::parse("*/15 8-9 * * *").unwrap().preview(now, 480)[0],
        now + 900
    );
    assert_eq!(
        Cron::parse("0 0 31 2 *").unwrap().occurrence(now, 0, true),
        None
    );
    assert!(
        Cron::parse("0 0 29 2 *")
            .unwrap()
            .occurrence(now, 0, true)
            .is_some()
    );
    assert_eq!(
        Cron::parse("0 0 * * 7").unwrap().preview(now, 0),
        Cron::parse("0 0 * * 0").unwrap().preview(now, 0)
    );
    // Restricted month-day and weekday are OR, including Monday in November.
    assert_eq!(
        Cron::parse("0 8 1 * 1").unwrap().occurrence(now, 480, true),
        Some(now + 3 * 86400)
    );
    for invalid in [
        "",
        "* * * *",
        "60 * * * *",
        "*/0 * * * *",
        "0 0 * 13 *",
        "0 0 * * MON",
        "0 0 9-1 * *",
        "0 0 * * 8",
        "0 0 * * 1,,2",
    ] {
        assert!(Cron::parse(invalid).is_err(), "{invalid}");
    }
}
