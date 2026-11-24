//! Интеграционные тесты форматтеров: USD, длительность, даты.

use std::time::{SystemTime, UNIX_EPOCH};

use km_shared::Lang::{En, Ru};
use km_ui_core::format::{format_date, format_date_at, format_duration, format_usd};

/// 2026-11-20T00:00:00Z (день 20777 от Unix-эпохи).
const NOV_20_2026: i64 = 1_795_132_800;
/// 2026-11-25T00:00:00Z — точка отсчёта для фиксированных тестов.
const NOV_25_2026: i64 = NOV_20_2026 + 5 * 86_400;
/// 2026-01-01T00:00:00Z.
const JAN_1_2026: i64 = 1_767_225_600;

#[test]
fn usd_two_decimals() {
    assert_eq!(format_usd(0.04), "$0.04");
    assert_eq!(format_usd(0.0), "$0.00");
    assert_eq!(format_usd(1.5), "$1.50");
    assert_eq!(format_usd(1234.567), "$1234.57");
}

#[test]
fn usd_non_finite_is_dash() {
    assert_eq!(format_usd(f64::NAN), "—");
    assert_eq!(format_usd(f64::INFINITY), "—");
    assert_eq!(format_usd(f64::NEG_INFINITY), "—");
}

#[test]
fn usd_negative_keeps_sign() {
    // Задокументированный выбор: отрицательные суммы допустимы как есть.
    assert_eq!(format_usd(-1.0), "$-1.00");
}

#[test]
fn duration_pinned() {
    assert_eq!(format_duration(12.0), "0:12");
    assert_eq!(format_duration(65.0), "1:05");
}

#[test]
fn duration_truncates_and_pads() {
    assert_eq!(format_duration(0.0), "0:00");
    assert_eq!(format_duration(12.9), "0:12");
    assert_eq!(format_duration(59.9), "0:59");
    assert_eq!(format_duration(60.0), "1:00");
}

#[test]
fn duration_hours() {
    assert_eq!(format_duration(3600.0), "1:00:00");
    assert_eq!(format_duration(3661.0), "1:01:01");
}

#[test]
fn duration_invalid_is_dash() {
    assert_eq!(format_duration(f64::NAN), "—");
    assert_eq!(format_duration(f64::INFINITY), "—");
    assert_eq!(format_duration(-1.0), "—");
}

#[test]
fn date_today_yesterday_relative_to_real_now() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("системное время после Unix-эпохи")
        .as_secs() as i64;
    assert_eq!(format_date(now, Ru), "сегодня");
    assert_eq!(format_date(now, En), "today");
    assert_eq!(format_date(now - 86_400, Ru), "вчера");
    assert_eq!(format_date(now - 86_400, En), "yesterday");
}

#[test]
fn date_today_yesterday_deterministic() {
    assert_eq!(format_date_at(NOV_25_2026, NOV_25_2026, Ru), "сегодня");
    assert_eq!(format_date_at(NOV_25_2026, NOV_25_2026, En), "today");
    assert_eq!(
        format_date_at(NOV_25_2026 - 86_400, NOV_25_2026, Ru),
        "вчера"
    );
    assert_eq!(
        format_date_at(NOV_25_2026 - 86_400, NOV_25_2026, En),
        "yesterday"
    );
}

#[test]
fn date_older_shows_day_and_short_month() {
    let noon = NOV_20_2026 + 43_200;
    assert_eq!(format_date_at(noon, NOV_25_2026, Ru), "20 ноя");
    assert_eq!(format_date_at(noon, NOV_25_2026, En), "Nov 20");
}

#[test]
fn date_other_year_includes_year() {
    // 2025-11-20 (2026 не високосный, минус 365 дней).
    let nov_20_2025 = NOV_20_2026 - 365 * 86_400 + 43_200;
    assert_eq!(format_date_at(nov_20_2025, NOV_25_2026, Ru), "20 ноя 2025");
    assert_eq!(format_date_at(nov_20_2025, NOV_25_2026, En), "Nov 20, 2025");
}

#[test]
fn date_epoch() {
    // 1970-01-01 при now 1970-01-03 — тот же год, без года в строке.
    assert_eq!(format_date_at(0, 2 * 86_400, Ru), "1 янв");
    assert_eq!(format_date_at(0, 2 * 86_400, En), "Jan 1");
}

#[test]
fn date_leap_day() {
    // 2024-02-29 (високосный), now — 2024-03-05.
    let feb_29 = 1_709_164_800;
    let mar_5 = feb_29 + 5 * 86_400;
    assert_eq!(format_date_at(feb_29, mar_5, Ru), "29 фев");
    assert_eq!(format_date_at(feb_29, mar_5, En), "Feb 29");
}

#[test]
fn date_all_month_names() {
    // Первое число каждого месяца 2026 (невисокосный), now — 31 декабря.
    let offsets = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let ru = [
        "1 янв", "1 фев", "1 мар", "1 апр", "1 мая", "1 июн", "1 июл", "1 авг", "1 сен", "1 окт",
        "1 ноя", "1 дек",
    ];
    let en = [
        "Jan 1", "Feb 1", "Mar 1", "Apr 1", "May 1", "Jun 1", "Jul 1", "Aug 1", "Sep 1", "Oct 1",
        "Nov 1", "Dec 1",
    ];
    let now = JAN_1_2026 + 364 * 86_400 + 43_200;
    for (i, &offset) in offsets.iter().enumerate() {
        let day = JAN_1_2026 + offset * 86_400 + 43_200;
        assert_eq!(format_date_at(day, now, Ru), ru[i], "месяц {}", i + 1);
        assert_eq!(format_date_at(day, now, En), en[i], "month {}", i + 1);
    }
}
