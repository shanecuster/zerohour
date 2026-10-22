//! Month-grid calendar math for the date picker: which dates go in which cell, and how the
//! cursor moves. Pure functions, no drawing, so the TUI and the Android app can share them.
//!
//! A month is shown as a grid of 6 weeks × 7 days. Six rows always fit any month (a 31-day
//! month starting on the last day of the week spans 6 weeks), and a fixed size keeps the popup
//! from jumping around as you page through months. Cells before the 1st and after the last day
//! show the neighboring months' dates, dimmed by the UI.

use chrono::{Datelike, Months, NaiveDate, TimeDelta, Weekday};

/// Which day a week starts on in the calendar. Saved in settings as `"sunday"` / `"monday"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WeekStart {
    /// Sunday first (the default, as in US calendars).
    #[default]
    Sunday,
    /// Monday first (ISO 8601, common in Europe).
    Monday,
}

impl WeekStart {
    /// The text stored in settings.
    pub fn as_str(self) -> &'static str {
        match self {
            WeekStart::Sunday => "sunday",
            WeekStart::Monday => "monday",
        }
    }

    /// Reads the saved text; anything unknown (or nothing saved) means the default, Sunday.
    pub fn parse(s: Option<&str>) -> WeekStart {
        match s {
            Some("monday") => WeekStart::Monday,
            _ => WeekStart::Sunday,
        }
    }

    /// The other choice.
    pub fn toggled(self) -> WeekStart {
        match self {
            WeekStart::Sunday => WeekStart::Monday,
            WeekStart::Monday => WeekStart::Sunday,
        }
    }

    /// The first weekday of the week.
    fn first_day(self) -> Weekday {
        match self {
            WeekStart::Sunday => Weekday::Sun,
            WeekStart::Monday => Weekday::Mon,
        }
    }

    /// The seven weekdays in display order, starting with this week start. Used for the day
    /// checkboxes in the add/edit form, so they follow the same order as the calendar.
    pub fn weekdays(self) -> [Weekday; 7] {
        // `use Weekday::*` lets us write `Sun` instead of `Weekday::Sun` inside this function.
        use Weekday::*;
        match self {
            WeekStart::Sunday => [Sun, Mon, Tue, Wed, Thu, Fri, Sat],
            WeekStart::Monday => [Mon, Tue, Wed, Thu, Fri, Sat, Sun],
        }
    }

    /// Two-letter column headers, in display order (e.g. `["Su", "Mo", ...]`).
    pub fn day_labels(self) -> [&'static str; 7] {
        match self {
            WeekStart::Sunday => ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"],
            WeekStart::Monday => ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"],
        }
    }
}

/// The 6 × 7 grid of dates for the month containing `date`. Row 0, column 0 is the first day
/// of the week that contains the 1st of the month.
pub fn month_grid(date: NaiveDate, week_start: WeekStart) -> [[NaiveDate; 7]; 6] {
    let first = first_of_month(date);
    // How many days the 1st is after the start of its week (0..=6).
    // `days_since` counts forward from one weekday to another, wrapping around the week.
    let offset = first.weekday().days_since(week_start.first_day());
    let start = add_days(first, -i64::from(offset));
    // Fill row by row: cell (row, col) is `row * 7 + col` days after the grid's start.
    // `std::array::from_fn` builds a fixed-size array by calling the closure for each index.
    std::array::from_fn(|row| std::array::from_fn(|col| add_days(start, (row * 7 + col) as i64)))
}

/// The 1st of the month containing `date`.
pub fn first_of_month(date: NaiveDate) -> NaiveDate {
    // `with_day(1)` can't fail: every month has a 1st. The fallback is just for the type.
    date.with_day(1).unwrap_or(date)
}

/// True if `date` is in the same month (and year) as `month_of`.
pub fn same_month(date: NaiveDate, month_of: NaiveDate) -> bool {
    date.year() == month_of.year() && date.month() == month_of.month()
}

/// Moves a date by whole months (negative = back), keeping the day of the month where
/// possible and clamping it at the end of shorter months: Jan 31 + 1 month = Feb 28/29.
pub fn add_months(date: NaiveDate, months: i32) -> NaiveDate {
    let result = if months >= 0 {
        date.checked_add_months(Months::new(months.unsigned_abs()))
    } else {
        date.checked_sub_months(Months::new(months.unsigned_abs()))
    };
    // Only fails beyond year ±262,143; stay put rather than crash.
    result.unwrap_or(date)
}

/// Moves a date by whole days (negative = back). Stays put at chrono's limits instead of
/// panicking.
pub fn add_days(date: NaiveDate, days: i64) -> NaiveDate {
    date.checked_add_signed(TimeDelta::days(days))
        .unwrap_or(date)
}

/// A month's title for the popup, e.g. "October 2026".
pub fn month_title(date: NaiveDate) -> String {
    date.format("%B %Y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn october_2026_with_sunday_and_monday_starts() {
        // Oct 1, 2026 is a Thursday.
        let sunday = month_grid(d(2026, 10, 15), WeekStart::Sunday);
        assert_eq!(sunday[0][0], d(2026, 9, 27), "the Sunday before");
        assert_eq!(sunday[0][4], d(2026, 10, 1), "Thursday column");
        assert_eq!(sunday[5][6], d(2026, 11, 7));

        let monday = month_grid(d(2026, 10, 15), WeekStart::Monday);
        assert_eq!(monday[0][0], d(2026, 9, 28), "the Monday before");
        assert_eq!(monday[0][3], d(2026, 10, 1));
    }

    #[test]
    fn a_month_starting_on_the_week_start_begins_in_the_first_cell() {
        // Nov 1, 2026 is a Sunday.
        assert_eq!(
            month_grid(d(2026, 11, 20), WeekStart::Sunday)[0][0],
            d(2026, 11, 1)
        );
        // Jun 1, 2026 is a Monday.
        assert_eq!(
            month_grid(d(2026, 6, 1), WeekStart::Monday)[0][0],
            d(2026, 6, 1)
        );
    }

    #[test]
    fn six_rows_fit_every_month() {
        // Every month from 2024 to 2031: the 1st and the last day are both in the grid, and
        // the grid is 42 consecutive days.
        for year in 2024..2032 {
            for month in 1..=12 {
                let first = d(year, month, 1);
                let last = add_days(add_months(first, 1), -1);
                for start in [WeekStart::Sunday, WeekStart::Monday] {
                    let grid = month_grid(first, start);
                    let cells: Vec<NaiveDate> = grid.iter().flatten().copied().collect();
                    assert!(cells.contains(&first) && cells.contains(&last));
                    for pair in cells.windows(2) {
                        assert_eq!(pair[1], add_days(pair[0], 1));
                    }
                    assert_eq!(cells[0].weekday(), start.first_day());
                }
            }
        }
    }

    #[test]
    fn month_moves_clamp_the_day() {
        assert_eq!(add_months(d(2026, 1, 31), 1), d(2026, 2, 28));
        assert_eq!(add_months(d(2028, 1, 31), 1), d(2028, 2, 29), "leap year");
        assert_eq!(add_months(d(2026, 3, 31), -1), d(2026, 2, 28));
        assert_eq!(
            add_months(d(2026, 12, 15), 1),
            d(2027, 1, 15),
            "year rollover"
        );
        assert_eq!(add_months(d(2026, 1, 15), -1), d(2025, 12, 15));
    }

    #[test]
    fn helpers() {
        assert_eq!(first_of_month(d(2026, 10, 9)), d(2026, 10, 1));
        assert!(same_month(d(2026, 10, 1), d(2026, 10, 31)));
        assert!(!same_month(d(2026, 10, 1), d(2025, 10, 1)));
        assert_eq!(month_title(d(2026, 10, 9)), "October 2026");
        assert_eq!(WeekStart::Sunday.day_labels()[0], "Su");
        assert_eq!(WeekStart::Monday.day_labels()[6], "Su");
        assert_eq!(WeekStart::parse(None), WeekStart::Sunday);
        assert_eq!(WeekStart::parse(Some("monday")), WeekStart::Monday);
        assert_eq!(WeekStart::Monday.toggled(), WeekStart::Sunday);
    }
}
