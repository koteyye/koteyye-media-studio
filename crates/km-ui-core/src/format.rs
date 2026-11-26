//! Форматтеры значений для UI: суммы USD, длительность и даты.
//!
//! Все функции чистые; [`format_date`] берёт текущий момент внутри,
//! а [`format_date_at`] принимает его явно — для детерминированных
//! тестов и предпросмотра.

use std::time::{SystemTime, UNIX_EPOCH};

use km_shared::Lang;

/// Секунд в минуте.
const SECS_PER_MINUTE: u64 = 60;
/// Секунд в часе.
const SECS_PER_HOUR: u64 = 3600;
/// Секунд в сутках — календарные дни считаются по UTC.
const SECS_PER_DAY: i64 = 86_400;

/// Короткие имена месяцев по-русски; май в родительном падеже,
/// как принято в датах («20 мая»).
const MONTHS_RU: [&str; 12] = [
    "янв", "фев", "мар", "апр", "мая", "июн", "июл", "авг", "сен", "окт", "ноя", "дек",
];

/// Короткие имена месяцев по-английски.
const MONTHS_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Форматирует сумму в USD с двумя знаками после запятой (`$0.04`).
///
/// Не-конечные значения (NaN, бесконечность) заменяются на `"—"`.
/// Отрицательные суммы допустимы и выводятся со знаком (`$-1.00`).
///
/// # Examples
///
/// ```
/// use km_ui_core::format::format_usd;
///
/// assert_eq!(format_usd(0.04), "$0.04");
/// assert_eq!(format_usd(f64::NAN), "—");
/// ```
#[must_use]
pub fn format_usd(v: f64) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    format!("${v:.2}")
}

/// Форматирует длительность в секундах как `M:SS` (`0:12`, `1:05`).
///
/// Дробные секунды отбрасываются. От часа и дольше — `H:MM:SS`.
/// Отрицательные и не-конечные значения заменяются на `"—"`.
///
/// # Examples
///
/// ```
/// use km_ui_core::format::format_duration;
///
/// assert_eq!(format_duration(12.0), "0:12");
/// assert_eq!(format_duration(65.0), "1:05");
/// ```
#[must_use]
pub fn format_duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "—".to_string();
    }
    // Приведение f64→u64 насыщающее: после проверок выше значение
    // неотрицательно и конечно, огромные длительности упрутся в u64::MAX.
    let total = secs as u64;
    let hours = total / SECS_PER_HOUR;
    let minutes = (total % SECS_PER_HOUR) / SECS_PER_MINUTE;
    let seconds = total % SECS_PER_MINUTE;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Форматирует дату относительно текущего момента.
///
/// Сегодня и вчера — словами (`сегодня`/`вчера`, `today`/`yesterday`),
/// остальные даты — день и короткий месяц (`20 ноя`, `Nov 20`);
/// при годе, отличном от текущего, добавляется год.
/// Сравнение календарных дней идёт по UTC.
///
/// # Examples
///
/// ```
/// use km_shared::Lang;
/// use km_ui_core::format::format_date;
/// use std::time::{SystemTime, UNIX_EPOCH};
///
/// let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
/// assert_eq!(format_date(now, Lang::Ru), "сегодня");
/// # Ok::<(), std::time::SystemTimeError>(())
/// ```
#[must_use]
pub fn format_date(unix_secs: i64, lang: Lang) -> String {
    // Часы до Unix-эпохи на целевых платформах невозможны; если это
    // всё же случилось, деградируем до абсолютных дат, а не паникуем.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_date_at(unix_secs, now, lang)
}

/// Как [`format_date`], но текущий момент задаётся явно (Unix-секунды).
///
/// Позволяет детерминированно тестировать и форматировать даты
/// относительно произвольной точки отсчёта. Будущие даты выводятся
/// абсолютной датой.
#[must_use]
pub fn format_date_at(unix_secs: i64, now_secs: i64, lang: Lang) -> String {
    let day = unix_secs.div_euclid(SECS_PER_DAY);
    let now_day = now_secs.div_euclid(SECS_PER_DAY);
    match now_day - day {
        0 => {
            return match lang {
                Lang::Ru => "сегодня",
                Lang::En => "today",
            }
            .to_string();
        }
        1 => {
            return match lang {
                Lang::Ru => "вчера",
                Lang::En => "yesterday",
            }
            .to_string();
        }
        _ => {}
    }

    let (year, month, day_of_month) = civil_from_days(day);
    let (now_year, _, _) = civil_from_days(now_day);
    // civil_from_days гарантирует месяц 1..=12, индекс всегда валиден.
    let month_idx = (month - 1) as usize;
    if year != now_year {
        match lang {
            Lang::Ru => format!("{day_of_month} {} {year}", MONTHS_RU[month_idx]),
            Lang::En => format!("{} {day_of_month}, {year}", MONTHS_EN[month_idx]),
        }
    } else {
        match lang {
            Lang::Ru => format!("{day_of_month} {}", MONTHS_RU[month_idx]),
            Lang::En => format!("{} {day_of_month}", MONTHS_EN[month_idx]),
        }
    }
}

/// Относительная календарная дата в часовом поясе пользователя. Смещения в
/// минутах следуют Date.getTimezoneOffset (UTC минус местное время) и могут
/// различаться для даты записи и сегодняшнего дня.
pub fn format_local_date_at(
    unix_secs: i64,
    now_secs: i64,
    record_offset: i32,
    now_offset: i32,
    lang: Lang,
) -> String {
    format_date_at(
        unix_secs.saturating_sub(i64::from(record_offset) * 60),
        now_secs.saturating_sub(i64::from(now_offset) * 60),
        lang,
    )
}

/// Гражданская дата (год, месяц 1–12, день 1–31) из дней от Unix-эпохи.
///
/// Алгоритм Howard Hinnant (`civil_from_days`): чистая арифметика
/// без таблиц и исключений, корректен для високосных лет и любого
/// дня в диапазоне i64.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Сдвиг эпохи к 0000-03-01 — началу 400-летнего цикла григорианского
    // календаря, где високосные годы идут в конце цикла.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}
