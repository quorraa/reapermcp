//! ISO-8601 UTC timestamps, `std`-only.
//!
//! Everything is second-precision UTC in the shape `YYYY-MM-DDTHH:MM:SSZ`.
//! The calendar arithmetic is Howard Hinnant's days-from-civil / civil-from-days
//! algorithm on the proleptic Gregorian calendar, so leap years are exact.
//! Leap seconds are deliberately ignored: the mapping is the POSIX one, where
//! every day is exactly 86400 seconds.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds in a day (POSIX: leap seconds are not represented).
const SECS_PER_DAY: i64 = 86_400;

/// Returns the current time as an ISO-8601 UTC string, e.g. `2026-07-26T18:51:19Z`.
pub fn now_iso8601() -> String {
    iso8601_from_unix(unix_now())
}

/// Returns the current Unix timestamp in whole seconds.
///
/// Returns a negative value for clocks set before 1970; never panics.
pub fn unix_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// Formats a Unix timestamp as an ISO-8601 UTC string with second precision.
pub fn iso8601_from_unix(secs: i64) -> String {
    let days = secs.div_euclid(SECS_PER_DAY);
    let rem = secs.rem_euclid(SECS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    if (0..=9999).contains(&y) {
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
    } else {
        // Outside the four-digit range ISO-8601 wants an explicit sign; keep
        // the rest of the layout identical so parsing stays symmetrical.
        format!("{y:+05}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
    }
}

/// Parses an ISO-8601 UTC timestamp into a Unix timestamp in whole seconds.
///
/// Accepted forms:
///
/// * `YYYY-MM-DDTHH:MM:SS` optionally followed by a zone
/// * fractional seconds, `.` or `,` separated, any number of digits (truncated)
/// * zone designators `Z`, `z`, `+HH:MM`, `-HH:MM`, `+HHMM`, `-HHMM`, `+HH`, `-HH`
/// * a missing zone, which is interpreted as UTC
/// * `t` in place of the date/time separator `T`, or a single space
///
/// Returns `None` for anything malformed or out of calendar range.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let year = parse_uint(&b[0..4])? as i64;
    if b[4] != b'-' {
        return None;
    }
    let month = parse_uint(&b[5..7])? as i64;
    if b[7] != b'-' {
        return None;
    }
    let day = parse_uint(&b[8..10])? as i64;
    if !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let hour = parse_uint(&b[11..13])? as i64;
    if b[13] != b':' {
        return None;
    }
    let minute = parse_uint(&b[14..16])? as i64;
    if b[16] != b':' {
        return None;
    }
    let second = parse_uint(&b[17..19])? as i64;

    if !(1..=12).contains(&month) {
        return None;
    }
    if day < 1 || day > days_in_month(year, month as u32) as i64 {
        return None;
    }
    // Second 60 is accepted (and treated as an ordinary second) so that leap
    // second timestamps from other systems do not hard-fail.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut i = 19;
    if i < b.len() && (b[i] == b'.' || b[i] == b',') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
    }

    let offset = parse_offset(&b[i..])?;

    let days = days_from_civil(year, month as u32, day as u32);
    let local = days
        .checked_mul(SECS_PER_DAY)?
        .checked_add(hour * 3600 + minute * 60 + second)?;
    local.checked_sub(offset)
}

/// Parses a trailing zone designator into an offset in seconds east of UTC.
fn parse_offset(b: &[u8]) -> Option<i64> {
    if b.is_empty() {
        return Some(0);
    }
    if b.len() == 1 && (b[0] == b'Z' || b[0] == b'z') {
        return Some(0);
    }
    let sign = match b[0] {
        b'+' => 1i64,
        b'-' => -1i64,
        _ => return None,
    };
    let rest = &b[1..];
    let (hh, mm) = match rest.len() {
        2 => (parse_uint(rest)? as i64, 0i64),
        4 => (
            parse_uint(&rest[0..2])? as i64,
            parse_uint(&rest[2..4])? as i64,
        ),
        5 if rest[2] == b':' => (
            parse_uint(&rest[0..2])? as i64,
            parse_uint(&rest[3..5])? as i64,
        ),
        _ => return None,
    };
    if hh > 23 || mm > 59 {
        return None;
    }
    Some(sign * (hh * 3600 + mm * 60))
}

/// Parses an all-ASCII-digit byte slice as an unsigned integer.
fn parse_uint(b: &[u8]) -> Option<u64> {
    if b.is_empty() {
        return None;
    }
    let mut v: u64 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(u64::from(c - b'0'))?;
    }
    Some(v)
}

/// Returns `true` if `y` is a leap year in the proleptic Gregorian calendar.
pub fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Returns the number of days in `month` (1-12) of year `y`.
///
/// Returns 0 for an out-of-range month.
pub fn days_in_month(y: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian civil date.
///
/// This is Howard Hinnant's `days_from_civil`; the era arithmetic keeps it
/// branch-light and exact for the whole `i64` range we care about.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let m = m as i64;
    let d = d as i64;
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // March-based month [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Proleptic Gregorian civil date `(year, month, day)` from days since 1970-01-01.
///
/// The inverse of [`days_from_civil`].
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (y + i64::from(m <= 2), m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formats_and_parses() {
        assert_eq!(iso8601_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn known_timestamp_round_trips() {
        let s = "2026-07-26T18:51:19Z";
        let t = parse_iso8601(s).expect("parse");
        assert_eq!(iso8601_from_unix(t), s);
        assert_eq!(t, 1_785_091_879);
    }

    #[test]
    fn round_trip_over_many_instants() {
        let mut t = -2_000_000_000i64;
        while t < 4_000_000_000 {
            let s = iso8601_from_unix(t);
            assert_eq!(parse_iso8601(&s), Some(t), "{s}");
            t += 999_983;
        }
    }

    #[test]
    fn leap_day_and_month_boundaries() {
        assert_eq!(
            parse_iso8601("2024-02-29T00:00:00Z")
                .map(iso8601_from_unix)
                .as_deref(),
            Some("2024-02-29T00:00:00Z")
        );
        assert_eq!(parse_iso8601("2023-02-29T00:00:00Z"), None);
        assert!(parse_iso8601("2000-02-29T00:00:00Z").is_some());
        assert_eq!(parse_iso8601("1900-02-29T00:00:00Z"), None);
        assert_eq!(parse_iso8601("2100-02-29T00:00:00Z"), None);
        // Day after 28 Feb in a non-leap year.
        let feb28 = parse_iso8601("2023-02-28T00:00:00Z").expect("parse");
        assert_eq!(iso8601_from_unix(feb28 + 86_400), "2023-03-01T00:00:00Z");
        // Day after 28 Feb in a leap year.
        let feb28l = parse_iso8601("2024-02-28T00:00:00Z").expect("parse");
        assert_eq!(iso8601_from_unix(feb28l + 86_400), "2024-02-29T00:00:00Z");
        // Year rollover.
        let nye = parse_iso8601("2025-12-31T23:59:59Z").expect("parse");
        assert_eq!(iso8601_from_unix(nye + 1), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn every_month_end_is_exact() {
        for y in [1999i64, 2000, 2023, 2024, 2100] {
            for m in 1..=12u32 {
                let last = days_in_month(y, m);
                let s = format!("{y:04}-{m:02}-{last:02}T12:00:00Z");
                let t = parse_iso8601(&s).unwrap_or_else(|| panic!("parse {s}"));
                assert_eq!(iso8601_from_unix(t), s);
                let over = format!("{y:04}-{m:02}-{:02}T12:00:00Z", last + 1);
                assert_eq!(parse_iso8601(&over), None, "{over} should be rejected");
            }
        }
    }

    #[test]
    fn fractional_seconds_are_truncated() {
        let base = parse_iso8601("2026-07-26T18:51:19Z").expect("parse");
        assert_eq!(parse_iso8601("2026-07-26T18:51:19.5Z"), Some(base));
        assert_eq!(parse_iso8601("2026-07-26T18:51:19.123456789Z"), Some(base));
        assert_eq!(parse_iso8601("2026-07-26T18:51:19,25Z"), Some(base));
        assert_eq!(parse_iso8601("2026-07-26T18:51:19."), None);
    }

    #[test]
    fn offsets_are_applied() {
        let utc = parse_iso8601("2026-07-26T18:51:19Z").expect("parse");
        assert_eq!(parse_iso8601("2026-07-26T20:51:19+02:00"), Some(utc));
        assert_eq!(parse_iso8601("2026-07-26T16:51:19-02:00"), Some(utc));
        assert_eq!(parse_iso8601("2026-07-26T20:51:19+0200"), Some(utc));
        assert_eq!(parse_iso8601("2026-07-26T20:51:19+02"), Some(utc));
        // No zone means UTC.
        assert_eq!(parse_iso8601("2026-07-26T18:51:19"), Some(utc));
        assert_eq!(parse_iso8601("2026-07-26t18:51:19z"), Some(utc));
    }

    #[test]
    fn malformed_timestamps_are_rejected() {
        for bad in [
            "",
            "2026",
            "2026-07-26",
            "2026-07-26X18:51:19Z",
            "2026/07/26T18:51:19Z",
            "2026-13-01T00:00:00Z",
            "2026-00-01T00:00:00Z",
            "2026-07-00T00:00:00Z",
            "2026-07-26T24:00:00Z",
            "2026-07-26T18:61:00Z",
            "2026-07-26T18:51:61Z",
            "2026-07-26T18:51:19Q",
            "2026-07-26T18:51:19+2:00",
            "2026-07-26T18:51:19+24:00",
            "2026-07-26T18:51:19+02:60",
            "20x6-07-26T18:51:19Z",
        ] {
            assert_eq!(parse_iso8601(bad), None, "{bad} should be rejected");
        }
    }

    #[test]
    fn leap_second_is_tolerated() {
        assert!(parse_iso8601("2016-12-31T23:59:60Z").is_some());
    }

    #[test]
    fn civil_day_conversions_are_inverse() {
        for z in [-800_000i64, -1, 0, 1, 19_000, 200_000, 2_932_896] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z, "z={z}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn leap_year_rules() {
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2023));
        assert!(!is_leap_year(2100));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2023, 2), 28);
        assert_eq!(days_in_month(2023, 13), 0);
    }

    #[test]
    fn now_is_recent_and_well_formed() {
        let now = unix_now();
        assert!(now > 1_700_000_000, "clock looks wrong: {now}");
        let s = now_iso8601();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z'));
        assert_eq!(parse_iso8601(&s), Some(now));
    }

    #[test]
    fn pre_epoch_times_format_correctly() {
        assert_eq!(iso8601_from_unix(-1), "1969-12-31T23:59:59Z");
        assert_eq!(parse_iso8601("1969-12-31T23:59:59Z"), Some(-1));
        assert_eq!(iso8601_from_unix(-86_400), "1969-12-31T00:00:00Z");
    }
}
