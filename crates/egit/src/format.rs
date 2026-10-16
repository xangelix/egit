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
    let (year, month, day) = civil_from_days(unix.checked_div(86_400).unwrap_or(0));
    format!("{year:04}-{month:02}-{day:02}")
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard Hinnant's algorithm,
/// for days on or after the epoch). The values involved stay far below `u64`'s range.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let shifted = days.wrapping_add(719_468);
    let era = shifted.checked_div(146_097).unwrap_or(0);
    let day_of_era = shifted.wrapping_sub(era.wrapping_mul(146_097));
    let year_of_era = day_of_era
        .wrapping_sub(day_of_era.checked_div(1_460).unwrap_or(0))
        .wrapping_add(day_of_era.checked_div(36_524).unwrap_or(0))
        .wrapping_sub(day_of_era.checked_div(146_096).unwrap_or(0))
        .checked_div(365)
        .unwrap_or(0);
    let day_of_year = day_of_era.wrapping_sub(
        year_of_era
            .wrapping_mul(365)
            .wrapping_add(year_of_era.checked_div(4).unwrap_or(0))
            .wrapping_sub(year_of_era.checked_div(100).unwrap_or(0)),
    );
    let month_index = day_of_year
        .wrapping_mul(5)
        .wrapping_add(2)
        .checked_div(153)
        .unwrap_or(0);
    let day = day_of_year
        .wrapping_sub(
            month_index
                .wrapping_mul(153)
                .wrapping_add(2)
                .checked_div(5)
                .unwrap_or(0),
        )
        .wrapping_add(1);
    let month = if month_index < 10 {
        month_index.wrapping_add(3)
    } else {
        month_index.wrapping_sub(9)
    };
    let year = year_of_era
        .wrapping_add(era.wrapping_mul(400))
        .wrapping_add(u64::from(month <= 2));
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
}
