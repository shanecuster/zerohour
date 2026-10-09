//! Times of day: parsing what the user types ("2pm", "14:30"), showing a time in the user's
//! 12- or 24-hour style, and short "in 3h" / "overdue 45m" texts.
//!
//! Like the date code, nothing here reads the clock: "now" is always a parameter. Times are
//! *naive local* times (`NaiveTime` / `NaiveDateTime`): wall-clock values with no time zone.
//! Comparing two of them needs no zone math, so a daylight-saving change can't break anything;
//! at worst an "in 3h" on that one day is an hour off.

use chrono::{NaiveDateTime, NaiveTime, Timelike};

/// How times are shown. Saved in settings as `"12h"` (the default) or `"24h"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeFormat {
    /// "2:00 PM".
    #[default]
    Hour12,
    /// "14:00".
    Hour24,
}

impl TimeFormat {
    /// The text stored in settings.
    pub fn as_str(self) -> &'static str {
        match self {
            TimeFormat::Hour12 => "12h",
            TimeFormat::Hour24 => "24h",
        }
    }

    /// Reads the saved text; anything unknown (or nothing saved) means 12-hour.
    pub fn parse(s: Option<&str>) -> TimeFormat {
        match s {
            Some("24h") => TimeFormat::Hour24,
            _ => TimeFormat::Hour12,
        }
    }

    /// The other style.
    pub fn toggled(self) -> TimeFormat {
        match self {
            TimeFormat::Hour12 => TimeFormat::Hour24,
            TimeFormat::Hour24 => TimeFormat::Hour12,
        }
    }
}

/// A time in the chosen style: "2:00 PM" / "14:00".
pub fn format_time(time: NaiveTime, format: TimeFormat) -> String {
    match format {
        // `%-I` = hour 1-12 without a leading zero, `%p` = AM/PM.
        TimeFormat::Hour12 => time.format("%-I:%M %p").to_string(),
        TimeFormat::Hour24 => time.format("%H:%M").to_string(),
    }
}

/// A very short time for narrow screens: "2PM", "2:30PM", or "14:30".
pub fn format_time_short(time: NaiveTime, format: TimeFormat) -> String {
    match format {
        TimeFormat::Hour12 if time.minute() == 0 => time.format("%-I%p").to_string(),
        TimeFormat::Hour12 => time.format("%-I:%M%p").to_string(),
        TimeFormat::Hour24 => time.format("%H:%M").to_string(),
    }
}

/// Parses a time the user typed. Empty means "no time" (`Ok(None)`).
///
/// Accepted (case and spaces don't matter): `14:30`, `9:05`, `14`, `1430`, `2pm`, `2 pm`,
/// `2:30pm`, `2:30 p.m.`, `12am` (midnight), `12pm` / `noon` (midday), `midnight`.
pub fn parse_time_input(input: &str) -> Result<Option<NaiveTime>, String> {
    // Lowercase, and drop spaces and dots so "2:30 P.M." becomes "2:30pm".
    let s: String = input
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '.')
        .collect();
    if s.is_empty() {
        return Ok(None);
    }
    let bad = || "not a time; try 2pm, 2:30 pm or 14:30".to_string();
    match s.as_str() {
        "noon" => return Ok(NaiveTime::from_hms_opt(12, 0, 0)),
        "midnight" => return Ok(NaiveTime::from_hms_opt(0, 0, 0)),
        _ => {}
    }

    // Split off an am/pm suffix if there is one.
    let (digits, suffix) = if let Some(rest) = s.strip_suffix("am") {
        (rest, Some(false))
    } else if let Some(rest) = s.strip_suffix("pm") {
        (rest, Some(true))
    } else {
        (s.as_str(), None)
    };

    // Hours and minutes: "14:30", "14", or "1430" / "930".
    let (hour, minute): (u32, u32) = if let Some((h, m)) = digits.split_once(':') {
        (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?)
    } else if digits.len() <= 2 {
        (digits.parse().map_err(|_| bad())?, 0)
    } else if digits.len() <= 4 && digits.chars().all(|c| c.is_ascii_digit()) {
        let (h, m) = digits.split_at(digits.len() - 2);
        (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?)
    } else {
        return Err(bad());
    };

    // Apply am/pm: 12am is 0:00, 12pm is 12:00, 1pm..11pm add 12. Hours above 12 can't
    // have a suffix ("14pm" makes no sense).
    let hour = match suffix {
        None => hour,
        Some(_) if hour == 0 || hour > 12 => return Err(bad()),
        Some(false) => hour % 12,
        Some(true) => hour % 12 + 12,
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
        .map(Some)
        .ok_or_else(bad)
}

/// A short countdown from `now` to `due`: "in 45m", "in 3h", "in 2d"; or "45m", "3h", "2d"
/// overdue when `due` has passed. Returns (text, is_overdue).
///
/// Units round *down* (2h 59m shows "2h"), and anything under a minute away counts as "in 0m",
/// so a task never reads "overdue" before its time.
pub fn relative_text(due: NaiveDateTime, now: NaiveDateTime) -> (String, bool) {
    let minutes = (due - now).num_minutes();
    let overdue = due < now;
    let amount = minutes.unsigned_abs();
    let text = if amount < 60 {
        format!("{amount}m")
    } else if amount < 24 * 60 {
        format!("{}h", amount / 60)
    } else {
        format!("{}d", amount / (24 * 60))
    };
    (text, overdue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn parses_common_time_styles() {
        let cases = [
            ("14:30", t(14, 30)),
            ("9:05", t(9, 5)),
            ("14", t(14, 0)),
            ("1430", t(14, 30)),
            ("930", t(9, 30)),
            ("2pm", t(14, 0)),
            ("2 PM", t(14, 0)),
            ("2:30pm", t(14, 30)),
            ("2:30 p.m.", t(14, 30)),
            ("11am", t(11, 0)),
            ("12am", t(0, 0)),
            ("12pm", t(12, 0)),
            ("noon", t(12, 0)),
            ("Midnight", t(0, 0)),
            ("0:00", t(0, 0)),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_time_input(input), Ok(Some(expected)), "{input}");
        }
        assert_eq!(parse_time_input("  "), Ok(None));
    }

    #[test]
    fn rejects_nonsense_times() {
        for input in ["25:00", "14pm", "0am", "2:60", "abc", "12345", "2:3x"] {
            assert!(parse_time_input(input).is_err(), "{input}");
        }
    }

    #[test]
    fn formats_in_both_styles() {
        assert_eq!(format_time(t(14, 0), TimeFormat::Hour12), "2:00 PM");
        assert_eq!(format_time(t(0, 5), TimeFormat::Hour12), "12:05 AM");
        assert_eq!(format_time(t(14, 0), TimeFormat::Hour24), "14:00");
        assert_eq!(format_time_short(t(14, 0), TimeFormat::Hour12), "2PM");
        assert_eq!(format_time_short(t(14, 30), TimeFormat::Hour12), "2:30PM");
        assert_eq!(format_time_short(t(9, 5), TimeFormat::Hour24), "09:05");
        assert_eq!(TimeFormat::parse(None), TimeFormat::Hour12);
        assert_eq!(TimeFormat::parse(Some("24h")), TimeFormat::Hour24);
        assert_eq!(TimeFormat::Hour24.toggled(), TimeFormat::Hour12);
    }

    #[test]
    fn relative_texts() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 9).unwrap();
        let now = day.and_time(t(11, 0));
        assert_eq!(
            relative_text(day.and_time(t(14, 0)), now),
            ("3h".into(), false)
        );
        assert_eq!(
            relative_text(day.and_time(t(13, 59)), now),
            ("2h".into(), false)
        );
        assert_eq!(
            relative_text(day.and_time(t(11, 45)), now),
            ("45m".into(), false)
        );
        assert_eq!(
            relative_text(day.and_time(t(11, 0)), now),
            ("0m".into(), false)
        );
        assert_eq!(
            relative_text(day.and_time(t(9, 0)), now),
            ("2h".into(), true)
        );
        let next_week = NaiveDate::from_ymd_opt(2026, 10, 12)
            .unwrap()
            .and_time(t(14, 0));
        assert_eq!(relative_text(next_week, now), ("3d".into(), false));
    }
}
