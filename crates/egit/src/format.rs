//! Text for people: paths with `~` for the home folder, and times.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Now, in seconds since the Unix epoch.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// `path` with the home folder shortened to `~`.
pub(crate) fn display_path(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

/// How long ago `then` was, roughly ("just now", "5 minutes ago", "yesterday"), or its date
/// beyond a week.
pub(crate) fn ago(then: u64, now: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    let elapsed = now.saturating_sub(then);
    let count = |unit: u64| elapsed.checked_div(unit).unwrap_or(0);
    let plural = |count: u64, unit: &str| {
        if count == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{count} {unit}s ago")
        }
    };
    match elapsed {
        0..MINUTE => "just now".to_owned(),
        MINUTE..HOUR => plural(count(MINUTE), "minute"),
        HOUR..DAY => plural(count(HOUR), "hour"),
        DAY..172_800 => "yesterday".to_owned(),
        172_800..604_800 => plural(count(DAY), "day"),
        _ => date(then),
    }
}

/// A Unix time's date, `YYYY-MM-DD` (UTC).
pub(crate) fn date(unix: u64) -> String {
    let days = i64::try_from(unix).unwrap_or(i64::MAX).div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// A time as someone saw it: `YYYY-MM-DD HH:MM` in their time zone, `offset_minutes` from
/// UTC (as `git log` shows a commit's dates).
pub(crate) fn date_time(unix: i64, offset_minutes: i32) -> String {
    let local = unix.saturating_add(i64::from(offset_minutes).saturating_mul(60));
    let (year, month, day) = civil_from_days(local.div_euclid(86_400));
    let minutes = local.rem_euclid(86_400).div_euclid(60);
    let (hour, minute) = (minutes.div_euclid(60), minutes.rem_euclid(60));
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// A size in bytes, in the largest unit it fills: `512 bytes`, `3.2 MB` (decimal units, as
/// file managers show them).
pub(crate) fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1_000 {
        return format!("{bytes} bytes");
    }
    let mut divisor = 1_000_u64;
    let mut unit = 0;
    while unit < UNITS.len().saturating_sub(1) && bytes.checked_div(divisor).unwrap_or(0) >= 1_000 {
        divisor = divisor.saturating_mul(1_000);
        unit = unit.saturating_add(1);
    }
    let tenths = bytes
        .checked_div(divisor.checked_div(10).unwrap_or(1))
        .unwrap_or(0);
    format!(
        "{}.{} {}",
        tenths.checked_div(10).unwrap_or(0),
        tenths.checked_rem(10).unwrap_or(0),
        UNITS.get(unit).copied().unwrap_or("PB")
    )
}

/// `n` with its thousands apart: `1,485,335`.
pub(crate) fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut grouped = String::with_capacity(digits.len().saturating_mul(2));
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && digits.len().saturating_sub(index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
/// Day counts from Unix times in seconds keep every step far inside `i64`'s range.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days.wrapping_add(719_468);
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = day_of_era
        .wrapping_sub(day_of_era.div_euclid(1_460))
        .wrapping_add(day_of_era.div_euclid(36_524))
        .wrapping_sub(day_of_era.div_euclid(146_096))
        .div_euclid(365);
    let day_of_year = day_of_era.wrapping_sub(
        year_of_era
            .wrapping_mul(365)
            .wrapping_add(year_of_era.div_euclid(4))
            .wrapping_sub(year_of_era.div_euclid(100)),
    );
    let month_index = day_of_year.wrapping_mul(5).wrapping_add(2).div_euclid(153);
    let day = day_of_year
        .wrapping_sub(month_index.wrapping_mul(153).wrapping_add(2).div_euclid(5))
        .wrapping_add(1);
    let month = if month_index < 10 {
        month_index.wrapping_add(3)
    } else {
        month_index.wrapping_sub(9)
    };
    let year = year_of_era
        .wrapping_add(era.wrapping_mul(400))
        .wrapping_add(i64::from(month <= 2));
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_the_home_folder() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(
            display_path(Path::new("/home/me/src/egit"), home),
            "~/src/egit"
        );
        assert_eq!(display_path(Path::new("/home/me"), home), "~");
        assert_eq!(
            display_path(Path::new("/home/meow/x"), home),
            "/home/meow/x"
        );
        assert_eq!(display_path(Path::new("/srv/repo"), None), "/srv/repo");
    }

    #[test]
    fn says_how_long_ago() {
        let now = 1_800_000_000;
        assert_eq!(ago(now, now), "just now");
        assert_eq!(ago(now - 59, now), "just now");
        assert_eq!(ago(now - 60, now), "1 minute ago");
        assert_eq!(ago(now - 5 * 60, now), "5 minutes ago");
        assert_eq!(ago(now - 3600, now), "1 hour ago");
        assert_eq!(ago(now - 23 * 3600, now), "23 hours ago");
        assert_eq!(ago(now - 30 * 3600, now), "yesterday");
        assert_eq!(ago(now - 3 * 86_400, now), "3 days ago");
        assert_eq!(ago(now - 30 * 86_400, now), date(now - 30 * 86_400));
        assert_eq!(ago(now + 100, now), "just now");
    }

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_792_022_400), "2026-10-15");
        assert_eq!(date(4_107_542_400), "2100-03-01");
    }

    #[test]
    fn dates_and_times_in_their_own_time_zone() {
        assert_eq!(date_time(0, 0), "1970-01-01 00:00");
        // 2026-10-15 23:30 UTC, seen from UTC+2 and UTC-5.
        let unix = 1_792_022_400 + 23 * 3600 + 30 * 60;
        assert_eq!(date_time(unix, 0), "2026-10-15 23:30");
        assert_eq!(date_time(unix, 120), "2026-10-16 01:30");
        assert_eq!(date_time(unix, -300), "2026-10-15 18:30");
        assert_eq!(date_time(-1, 0), "1969-12-31 23:59");
        assert_eq!(date_time(-86_400 * 366, 0), "1968-12-31 00:00");
    }

    #[test]
    fn sizes_in_the_largest_unit_they_fill() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1_000), "1.0 KB");
        assert_eq!(size(3_250_000), "3.2 MB");
        assert_eq!(size(999_999_999), "999.9 MB");
        assert_eq!(size(268_435_456), "268.4 MB");
        assert_eq!(size(u64::MAX), "18446.7 PB");
    }

    #[test]
    fn counts_with_thousands_apart() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_000), "1,000");
        assert_eq!(count(1_485_335), "1,485,335");
        assert_eq!(count(u64::MAX), "18,446,744,073,709,551,615");
    }
}
