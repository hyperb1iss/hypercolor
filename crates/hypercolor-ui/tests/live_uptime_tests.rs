use hypercolor_ui::live_uptime::{advanced_uptime_seconds, format_uptime};

#[test]
fn advanced_uptime_adds_whole_elapsed_seconds() {
    assert_eq!(advanced_uptime_seconds(18, 1_000.0, 1_000.0), 18);
    assert_eq!(advanced_uptime_seconds(18, 1_000.0, 1_999.0), 18);
    assert_eq!(advanced_uptime_seconds(18, 1_000.0, 2_000.0), 19);
    assert_eq!(advanced_uptime_seconds(18, 1_000.0, 3_301_000.0), 3_318);
}

#[test]
fn advanced_uptime_ignores_a_clock_reading_before_the_fetch() {
    assert_eq!(advanced_uptime_seconds(42, 5_000.0, 4_000.0), 42);
}

#[test]
fn advanced_uptime_saturates_instead_of_wrapping() {
    assert_eq!(advanced_uptime_seconds(u64::MAX, 0.0, 10_000.0), u64::MAX);
}

#[test]
fn format_uptime_shows_seconds_under_a_minute() {
    assert_eq!(format_uptime(0), "0s");
    assert_eq!(format_uptime(59), "59s");
}

#[test]
fn format_uptime_shows_minutes_under_an_hour() {
    assert_eq!(format_uptime(60), "1m");
    assert_eq!(format_uptime(3_599), "59m");
}

#[test]
fn format_uptime_shows_hours_and_minutes_from_an_hour() {
    assert_eq!(format_uptime(3_600), "1h 0m");
    assert_eq!(format_uptime(3_661), "1h 1m");
    assert_eq!(format_uptime(90_000), "25h 0m");
}
