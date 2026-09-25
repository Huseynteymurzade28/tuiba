//! The date and time handed to a cartridge's real-time clock.

use chrono::{Datelike, NaiveDateTime, Timelike};
use tuiba_core::DateTime;

/// The local date and time, as the clock chip should report it.
#[must_use]
pub fn now() -> DateTime {
    from_chrono(chrono::Local::now().naive_local())
}

/// Converts a calendar time. The chip counts years 2000–2099; anything
/// outside is clamped into that range rather than wrapped.
#[must_use]
pub fn from_chrono(t: NaiveDateTime) -> DateTime {
    DateTime {
        year: t.year().clamp(2000, 2099) as u16,
        month: t.month() as u8,
        day: t.day() as u8,
        weekday: t.weekday().num_days_from_sunday() as u8,
        hour: t.hour() as u8,
        minute: t.minute() as u8,
        second: t.second() as u8,
    }
}

/// Parses `YYYY-MM-DDTHH:MM:SS` (a space works in place of the `T`).
#[must_use]
pub fn parse(text: &str) -> Option<NaiveDateTime> {
    ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_to_the_chips_fields() {
        let t = parse("2026-09-25T21:07:42").expect("valid");
        assert_eq!(
            from_chrono(t),
            DateTime {
                year: 2026,
                month: 9,
                day: 25,
                weekday: 5,
                hour: 21,
                minute: 7,
                second: 42,
            }
        );
        assert_eq!(parse("2026-09-25 21:07:42"), Some(t));
        assert_eq!(parse("tomorrow"), None);
    }

    #[test]
    fn years_outside_the_chips_range_are_clamped() {
        let t = parse("1999-12-31T23:59:59").expect("valid");
        assert_eq!(from_chrono(t).year, 2000);
    }
}
