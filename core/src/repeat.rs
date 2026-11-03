//! Date math: repeating dates, countdowns, and parsing dates the user types.
//!
//! This is the most error-prone part of the app, so it is written plainly and tested hard
//! (see the `tests` module at the bottom). Two ground rules:
//!
//! 1. **`today` is always a parameter.** Nothing in here reads the clock. The caller (the UI)
//!    decides what "today" is. That makes every function deterministic: the same inputs always
//!    give the same answer, so tests can pretend it's any day we like (leap days, New Year's Eve).
//! 2. **Every occurrence is computed from the original anchor date.** A monthly repeat that
//!    started on Jan 31 is "anchor + 1 month", "anchor + 2 months", ... — never "previous
//!    occurrence + 1 month". That way Jan 31 -> Feb 28 -> Mar 31: the short month clamps the
//!    day, but the next month goes back to the 31st instead of drifting to the 28th forever.
//!
//! The one rule without a fixed step is **weekday sets** (e.g. Mon–Thu): occurrences are
//! simply every date on one of those days, so there's no anchor to drift from. The next
//! occurrence is found by looking at most 7 days ahead.
//!
//! **Nth weekdays** (the 4th Thursday of November, the 1st Friday of every month) work the same
//! way: every date that matches the rule is an occurrence, whatever the anchor. The next one is
//! in the target's month (or the rule's month in the target's year), or else the month (or
//! year) after.

use chrono::{Datelike, Month, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Weekday};

use crate::model::{Item, Kind, Ordinal, RepeatRule, Unit};

/// The size of one repeat step, normalized to either "some days" or "some months".
///
/// Weeks are just 7 days and years are just 12 months, so every rule reduces to one of these
/// two shapes. Days and months need different math because months have different lengths.
#[derive(Debug, Clone, Copy)]
enum Step {
    Days(i64),
    Months(u32),
}

/// Turns a repeat rule into its step size, or `None` if the rule doesn't repeat by a fixed
/// step (no repeat at all, a weekday set, or an Nth weekday of a month, which
/// [`first_on_or_after`] handles first).
fn step_of(rule: RepeatRule) -> Option<Step> {
    match rule {
        RepeatRule::None | RepeatRule::Weekdays(_) | RepeatRule::NthWeekday { .. } => None,
        RepeatRule::Daily => Some(Step::Days(1)),
        RepeatRule::Weekly => Some(Step::Days(7)),
        RepeatRule::Monthly => Some(Step::Months(1)),
        RepeatRule::Yearly => Some(Step::Months(12)),
        RepeatRule::EveryN { n, unit } => {
            // "Every 0 days" would never move forward (an infinite loop waiting to happen),
            // so we quietly treat 0 as 1. The add form also refuses 0.
            let n = n.max(1);
            Some(match unit {
                Unit::Days => Step::Days(i64::from(n)),
                Unit::Weeks => Step::Days(i64::from(n) * 7),
                Unit::Months => Step::Months(n),
            })
        }
    }
}

/// Adds whole months to a date, clamping the day to the end of shorter months.
///
/// Examples: Jan 31 + 1 month = Feb 28 (or Feb 29 in a leap year); Feb 29 + 12 months =
/// Feb 28 in a non-leap year. chrono's `checked_add_months` does exactly this clamping.
fn add_months(date: NaiveDate, months: u32) -> NaiveDate {
    // `checked_add_months` returns `None` only if the result is past the year 262,143.
    // Rather than crash in that absurd case, we use the latest date chrono can represent.
    date.checked_add_months(Months::new(months))
        .unwrap_or(NaiveDate::MAX)
}

/// Adds a number of days to a date (saturating at chrono's max date instead of panicking).
fn add_days(date: NaiveDate, days: i64) -> NaiveDate {
    date.checked_add_signed(TimeDelta::days(days))
        .unwrap_or(NaiveDate::MAX)
}

/// The date of the `nth` `weekday` in a month, e.g. the 4th Thursday of November 2026
/// (Nov 26). `None` only for an impossible year or month.
///
/// The 1st to 4th always exist (every month has at least 28 days, so 4 of each weekday);
/// "last" is the 5th when the month has one, else the 4th.
fn nth_weekday_in(year: i32, month: u32, nth: Ordinal, weekday: Weekday) -> Option<NaiveDate> {
    let n = match nth {
        Ordinal::First => 1,
        Ordinal::Second => 2,
        Ordinal::Third => 3,
        Ordinal::Fourth => 4,
        Ordinal::Last => {
            // Start on the month's last day (the day before the 1st of the next month) and
            // step back to the weekday: at most 6 steps.
            let first = NaiveDate::from_ymd_opt(year, month, 1)?;
            let mut day = first.checked_add_months(Months::new(1))?.pred_opt()?;
            while day.weekday() != weekday {
                day = day.pred_opt()?;
            }
            return Some(day);
        }
    };
    // chrono finds "the nth such weekday of the month" for us (n = 1..=5; `None` if the
    // month has no such day, which can't happen for 1..=4).
    NaiveDate::from_weekday_of_month_opt(year, month, weekday, n)
}

/// The first date on or after `target` that is the `nth` `weekday` of `month` (yearly), or of
/// any month (`month = None`). Saturates at chrono's max date instead of panicking.
fn nth_weekday_on_or_after(
    nth: Ordinal,
    weekday: Weekday,
    month: Option<Month>,
    target: NaiveDate,
) -> NaiveDate {
    // The (year, month) pairs to try, in order: this one, then the next. One of the two is
    // always on or after the target: the next month's (or year's) date is past the target's
    // whole month (or year).
    let tries = match month {
        Some(month) => {
            let m = month.number_from_month();
            [(target.year(), m), (target.year().saturating_add(1), m)]
        }
        None => {
            let (y, m) = (target.year(), target.month());
            // December rolls into January of the next year.
            let next = if m == 12 {
                (y.saturating_add(1), 1)
            } else {
                (y, m + 1)
            };
            [(y, m), next]
        }
    };
    // `find_map` runs the closure on each pair and stops at the first `Some`.
    tries
        .into_iter()
        .find_map(|(y, m)| nth_weekday_in(y, m, nth, weekday).filter(|date| *date >= target))
        .unwrap_or(NaiveDate::MAX)
}

/// The first occurrence of `rule`, starting from `anchor`, that falls on or after `target`.
///
/// Occurrences are `anchor`, `anchor + 1 step`, `anchor + 2 steps`, ... If the anchor itself is
/// already on or after the target, the anchor is the answer. For a non-repeating rule the only
/// occurrence is the anchor, so it is returned as-is (even if it's in the past).
fn first_on_or_after(rule: RepeatRule, anchor: NaiveDate, target: NaiveDate) -> NaiveDate {
    if anchor >= target {
        return anchor;
    }
    // Weekday sets: the first scheduled day on or after the target. (An empty set can't be
    // saved; if one ever appears, it behaves like "doesn't repeat".)
    if let RepeatRule::Weekdays(days) = rule {
        return days.first_on_or_after(target).unwrap_or(anchor);
    }
    // Nth weekdays: the first matching date on or after the target (see the module docs).
    // `{ nth, weekday, month }` in a pattern pulls the three fields out by name.
    if let RepeatRule::NthWeekday {
        nth,
        weekday,
        month,
    } = rule
    {
        return nth_weekday_on_or_after(nth, weekday, month, target);
    }
    // `let ... else` : bind the value if the pattern matches, otherwise run the `else` block
    // (which must leave the function). Here: no step means "doesn't repeat".
    let Some(step) = step_of(rule) else {
        return anchor;
    };

    match step {
        Step::Days(d) => {
            // Days are easy: jump straight there with division instead of looping.
            // We need the smallest k with anchor + k*d >= target, i.e. k = ceil(diff / d).
            let diff = (target - anchor).num_days();
            let k = (diff + d - 1) / d; // integer "round up" division
            add_days(anchor, k * d)
        }
        Step::Months(m) => {
            // Months vary in length, so we estimate how many steps to skip, then walk
            // forward. The estimate lands at or before the target's month, so the loop runs
            // at most about twice.
            let months_diff = (target.year() - anchor.year()) * 12 + target.month() as i32
                - anchor.month() as i32;
            // `as u32` converts number types. months_diff is never negative here because
            // anchor < target, but `.max(0)` makes that obvious to the reader.
            let mut k = (months_diff.max(0) as u32) / m;
            loop {
                // Always compute from the ORIGINAL anchor (see the module docs).
                let candidate = add_months(anchor, k * m);
                if candidate >= target {
                    return candidate;
                }
                k += 1;
            }
        }
    }
}

/// The first occurrence of a repeating date on or after `today`.
///
/// `from_date` is the item's stored date (the anchor). Used to show repeating events: Christmas
/// stored as 2025-12-25 shows as 2026-12-25 once 2025's has passed. For `RepeatRule::None` this
/// returns `from_date` unchanged.
pub fn next_occurrence(rule: RepeatRule, from_date: NaiveDate, today: NaiveDate) -> NaiveDate {
    first_on_or_after(rule, from_date, today)
}

/// The due date for the *next instance* of a repeating task that was just completed.
///
/// Returns the first occurrence that is strictly after BOTH the old due date and today:
/// - Completed on time or late: you've done today's (or an earlier) occurrence, so the next
///   one is the first occurrence after today. (A daily task done today is next due tomorrow.)
/// - Completed early: a weekly task due next Monday, done on Thursday, is next due the Monday
///   after that — not the same Monday again.
///
/// A repeating task with no due date uses today as its anchor. Returns `None` for non-repeating
/// rules, because those don't create a new instance.
pub fn next_task_date(
    rule: RepeatRule,
    old_date: Option<NaiveDate>,
    today: NaiveDate,
) -> Option<NaiveDate> {
    if !rule.is_repeating() {
        return None;
    }
    let anchor = old_date.unwrap_or(today);
    // "Strictly after X" is the same as "on or after the day after X".
    let after = anchor.max(today);
    Some(first_on_or_after(rule, anchor, add_days(after, 1)))
}

/// The due date for the next instance of a repeating task that was just completed, taking
/// the task's due *time* into account. `now` is the local date and time of completion.
///
/// - **No due time:** exactly [`next_task_date`] (date-only rules, unchanged).
/// - **With a due time:** the first occurrence whose date + time is strictly after BOTH the
///   old due date + time and `now`. So a daily 2 PM task that was due yesterday and is done
///   today at 10 AM is next due *today* at 2 PM (that occurrence is still ahead); done at
///   3 PM, it's next due tomorrow. Done early (before its due time), it moves past the one
///   you just did, same as the date-only rule.
pub fn next_instance_date(
    rule: RepeatRule,
    old_date: Option<NaiveDate>,
    old_time: Option<NaiveTime>,
    now: NaiveDateTime,
) -> Option<NaiveDate> {
    let today = now.date();
    // `let (Some(a), Some(b)) = ... else` : continue only if BOTH are present.
    let (Some(anchor), Some(time)) = (old_date, old_time) else {
        return next_task_date(rule, old_date, today);
    };
    if !rule.is_repeating() {
        return None;
    }
    // The moment the next occurrence must come after.
    let after = anchor.and_time(time).max(now);
    // The first occurrence on or after that day; if its time on that day isn't later than
    // `after` (e.g. it's today but the time has passed), take the following occurrence.
    let candidate = first_on_or_after(rule, anchor, after.date());
    if candidate.and_time(time) > after {
        Some(candidate)
    } else {
        Some(first_on_or_after(rule, anchor, add_days(candidate, 1)))
    }
}

/// The local date and time a task is due, if it has both a due date and a due time.
/// (`?` on an `Option` returns `None` early if that part is missing.)
pub fn due_datetime(item: &Item) -> Option<NaiveDateTime> {
    Some(item.date?.and_time(item.due_time?))
}

/// For rules that only land on certain days: the first scheduled day on or after `date`.
/// A Saturday start for a Mon–Thu rule becomes the following Monday; Nov 19 2026 for "4th Thu
/// of Nov" becomes Nov 26. Every other rule returns `date` unchanged.
///
/// The add/edit form uses this to move an unscheduled start date forward when saving (and to
/// preview that), and to fill in a missing date.
pub fn scheduled_on_or_after(rule: RepeatRule, date: NaiveDate) -> NaiveDate {
    match rule {
        RepeatRule::Weekdays(days) => days.first_on_or_after(date).unwrap_or(date),
        RepeatRule::NthWeekday {
            nth,
            weekday,
            month,
        } => nth_weekday_on_or_after(nth, weekday, month, date),
        _ => date,
    }
}

/// How many days from `today` until `date`. Positive = future, 0 = today, negative = past.
pub fn days_until(date: NaiveDate, today: NaiveDate) -> i64 {
    // Subtracting two dates gives a `TimeDelta`; `.num_days()` turns it into a whole number.
    (date - today).num_days()
}

/// The date an event should currently count down to, or `None` if it has no date.
///
/// Repeating events roll forward to their next occurrence on or after today automatically —
/// nothing is stored or clicked. One-off events just show their stored date.
pub fn event_target(item: &Item, today: NaiveDate) -> Option<NaiveDate> {
    // `.map(|d| ...)` transforms the value inside an Option if there is one.
    // `|d| expr` is a closure: a small unnamed function taking `d`.
    item.date.map(|d| next_occurrence(item.repeat, d, today))
}

/// Should this event be shown on the main screen today?
///
/// Repeating events always show (they've rolled forward). A one-off event shows until its day
/// is over; after that, only if the user ticked "keep after pass".
pub fn event_is_visible(item: &Item, today: NaiveDate) -> bool {
    match event_target(item, today) {
        Some(date) => date >= today || item.keep_after_pass,
        None => true, // An event without a date is a data problem; show it so it can be fixed.
    }
}

/// True if `item` is an open task that is past due at local time `now`.
///
/// A task is overdue from the day *after* its due date: the whole due day counts as on time,
/// with or without a due time. A task due at 2:00 PM is not late at 2:30 PM; it turns overdue
/// at midnight, like a task with no time. (The due time still matters for reminders, which
/// fire at it, and for the "in 3h" countdown; it just doesn't make a task late.)
///
/// `now` is a full date-time so callers don't need to know this rule only looks at the date.
pub fn is_overdue(item: &Item, now: NaiveDateTime) -> bool {
    if item.kind != Kind::Task || item.is_completed() {
        return false;
    }
    // `Option::is_some_and` is true only for `Some(date)` where the closure says yes; a task
    // without a date (`None`) is never overdue.
    item.date.is_some_and(|date| date < now.date())
}

/// Parses a date the user typed into the add form.
///
/// Accepted forms (case doesn't matter, surrounding spaces are ignored):
/// - empty            -> no date (`Ok(None)`)
/// - `2026-12-25`     -> that exact date
/// - `12/25/2026`     -> US-style month/day/year
/// - `12-25`, `12/25` -> the next Dec 25 on or after today
/// - `today`, `tomorrow`
/// - `+3`, `+3d`, `+2w`, `+1m`, `+1y` -> that many days/weeks/months/years from today
/// - `mon` ... `sun` (or full names) -> the next such weekday after today
///
/// `Result<T, E>` is Rust's "this can fail" type: `Ok(value)` or `Err(error)`. Here the error
/// is a friendly message the UI can show directly.
pub fn parse_date_input(input: &str, today: NaiveDate) -> Result<Option<NaiveDate>, String> {
    let s = input.trim().to_lowercase();
    if s.is_empty() {
        return Ok(None);
    }

    // Words first.
    match s.as_str() {
        "today" | "tod" => return Ok(Some(today)),
        "tomorrow" | "tom" | "tmr" => return Ok(Some(add_days(today, 1))),
        _ => {}
    }

    // Relative offsets: "+3", "+3d", "+2w", "+1m", "+1y".
    if let Some(rest) = s.strip_prefix('+') {
        // Split "12w" into the digits "12" and the unit "w".
        let digits_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (num, unit) = rest.split_at(digits_end);
        let n: u32 = num.parse().map_err(|_| bad_date_message())?;
        let date = match unit {
            "" | "d" => add_days(today, i64::from(n)),
            "w" => add_days(today, i64::from(n) * 7),
            "m" => add_months(today, n),
            "y" => add_months(today, n.saturating_mul(12)),
            _ => return Err(bad_date_message()),
        };
        return Ok(Some(date));
    }

    // Weekday names: the next one strictly after today ("fri" on a Friday = next Friday;
    // type "today" if you mean today).
    if let Ok(weekday) = s.parse::<Weekday>() {
        let mut date = add_days(today, 1);
        while date.weekday() != weekday {
            date = add_days(date, 1);
        }
        return Ok(Some(date));
    }

    // Full dates: ISO (2026-12-25) or US style (12/25/2026).
    for format in ["%Y-%m-%d", "%m/%d/%Y"] {
        if let Ok(date) = NaiveDate::parse_from_str(&s, format) {
            return Ok(Some(date));
        }
    }

    // Month and day only (12-25 or 12/25): the next one on or after today.
    // `if let A && let B` is a "let chain": every pattern must match for the block to run.
    let parts: Vec<&str> = s.split(['-', '/']).collect();
    if let [m, d] = parts.as_slice()
        && let (Ok(month), Ok(day)) = (m.parse::<u32>(), d.parse::<u32>())
    {
        // Try this year, then the following years. Checking several years handles Feb 29,
        // which only exists in leap years.
        for year in today.year()..=today.year() + 8 {
            if let Some(date) = NaiveDate::from_ymd_opt(year, month, day)
                && date >= today
            {
                return Ok(Some(date));
            }
        }
    }

    Err(bad_date_message())
}

/// The error message shown when a typed date can't be understood.
fn bad_date_message() -> String {
    "Couldn't read that date. Try 2026-12-25, 12/25, +3d, +2w, tomorrow, or fri.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// Shorthand to build a date in tests. `unwrap()` is fine in tests: a bad literal should
    /// fail the test loudly.
    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn event(date: NaiveDate, repeat: RepeatRule) -> Item {
        let mut item = Item::new(Kind::Event, "test", Utc::now());
        item.date = Some(date);
        item.repeat = repeat;
        item
    }

    // ---- next_occurrence ---------------------------------------------------------------

    #[test]
    fn no_repeat_returns_the_date_unchanged() {
        assert_eq!(
            next_occurrence(RepeatRule::None, d(2026, 1, 1), d(2026, 6, 1)),
            d(2026, 1, 1)
        );
    }

    #[test]
    fn future_anchor_is_its_own_next_occurrence() {
        assert_eq!(
            next_occurrence(RepeatRule::Yearly, d(2026, 12, 25), d(2026, 10, 8)),
            d(2026, 12, 25)
        );
    }

    #[test]
    fn christmas_rolls_over_after_it_passes() {
        let christmas = d(2025, 12, 25);
        // On Christmas day itself: 0 days, still this year's.
        assert_eq!(
            next_occurrence(RepeatRule::Yearly, christmas, d(2026, 12, 25)),
            d(2026, 12, 25)
        );
        // Dec 26 2026 -> Dec 25 2027 (2027 is not a leap year): 364 days.
        let next = next_occurrence(RepeatRule::Yearly, christmas, d(2026, 12, 26));
        assert_eq!(next, d(2027, 12, 25));
        assert_eq!(days_until(next, d(2026, 12, 26)), 364);
        // Dec 26 2027 -> Dec 25 2028, and Feb 29 2028 is in between: 365 days.
        let next = next_occurrence(RepeatRule::Yearly, christmas, d(2027, 12, 26));
        assert_eq!(days_until(next, d(2027, 12, 26)), 365);
    }

    #[test]
    fn year_rollover_for_monthly_and_daily() {
        assert_eq!(
            next_occurrence(RepeatRule::Monthly, d(2026, 1, 15), d(2026, 12, 20)),
            d(2027, 1, 15)
        );
        assert_eq!(
            next_occurrence(RepeatRule::Daily, d(2026, 12, 30), d(2027, 1, 2)),
            d(2027, 1, 2)
        );
        assert_eq!(
            next_occurrence(RepeatRule::Weekly, d(2026, 12, 28), d(2027, 1, 1)),
            d(2027, 1, 4)
        );
    }

    #[test]
    fn feb_29_yearly_falls_back_to_feb_28_in_non_leap_years() {
        let leap_day = d(2024, 2, 29);
        assert_eq!(
            next_occurrence(RepeatRule::Yearly, leap_day, d(2025, 1, 1)),
            d(2025, 2, 28)
        );
        assert_eq!(
            next_occurrence(RepeatRule::Yearly, leap_day, d(2027, 3, 1)),
            d(2028, 2, 29)
        );
        // Back on Feb 29 in the next leap year, not stuck on the 28th.
        assert_eq!(
            next_occurrence(RepeatRule::Yearly, leap_day, d(2028, 1, 1)),
            d(2028, 2, 29)
        );
    }

    #[test]
    fn leap_year_rules() {
        // 2000 was a leap year (divisible by 400); 2100 is not (divisible by 100 but not 400).
        let leap_day = d(2000, 2, 29);
        assert_eq!(
            next_occurrence(RepeatRule::Yearly, leap_day, d(2100, 1, 1)),
            d(2100, 2, 28)
        );
        assert_eq!(
            next_occurrence(RepeatRule::Yearly, leap_day, d(2096, 1, 1)),
            d(2096, 2, 29)
        );
    }

    #[test]
    fn month_end_anchoring_does_not_drift() {
        let jan31 = d(2026, 1, 31);
        assert_eq!(
            next_occurrence(RepeatRule::Monthly, jan31, d(2026, 2, 1)),
            d(2026, 2, 28)
        );
        // After Feb, back to the 31st (not the 28th).
        assert_eq!(
            next_occurrence(RepeatRule::Monthly, jan31, d(2026, 3, 1)),
            d(2026, 3, 31)
        );
        // April has 30 days.
        assert_eq!(
            next_occurrence(RepeatRule::Monthly, jan31, d(2026, 4, 1)),
            d(2026, 4, 30)
        );
        assert_eq!(
            next_occurrence(RepeatRule::Monthly, jan31, d(2026, 5, 1)),
            d(2026, 5, 31)
        );
        // Leap year February.
        assert_eq!(
            next_occurrence(RepeatRule::Monthly, d(2028, 1, 31), d(2028, 2, 1)),
            d(2028, 2, 29)
        );
    }

    #[test]
    fn every_n_intervals() {
        let start = d(2026, 1, 1);
        let every3days = RepeatRule::EveryN {
            n: 3,
            unit: Unit::Days,
        };
        assert_eq!(
            next_occurrence(every3days, start, d(2026, 1, 2)),
            d(2026, 1, 4)
        );
        assert_eq!(
            next_occurrence(every3days, start, d(2026, 1, 4)),
            d(2026, 1, 4)
        );

        let every2weeks = RepeatRule::EveryN {
            n: 2,
            unit: Unit::Weeks,
        };
        assert_eq!(
            next_occurrence(every2weeks, start, d(2026, 1, 2)),
            d(2026, 1, 15)
        );
        assert_eq!(
            next_occurrence(every2weeks, start, d(2026, 1, 16)),
            d(2026, 1, 29)
        );

        let every3months = RepeatRule::EveryN {
            n: 3,
            unit: Unit::Months,
        };
        assert_eq!(
            next_occurrence(every3months, d(2026, 1, 31), d(2026, 2, 1)),
            d(2026, 4, 30)
        );
        assert_eq!(
            next_occurrence(every3months, d(2026, 1, 31), d(2026, 5, 1)),
            d(2026, 7, 31)
        );
        assert_eq!(
            next_occurrence(every3months, d(2026, 11, 15), d(2026, 12, 1)),
            d(2027, 2, 15)
        );
    }

    #[test]
    fn every_zero_is_treated_as_every_one() {
        let rule = RepeatRule::EveryN {
            n: 0,
            unit: Unit::Days,
        };
        assert_eq!(
            next_occurrence(rule, d(2026, 1, 1), d(2026, 1, 5)),
            d(2026, 1, 5)
        );
    }

    #[test]
    fn far_past_anchor_is_fast_and_correct() {
        // A daily repeat anchored 100 years ago must not loop 36,500 times (and must be right).
        assert_eq!(
            next_occurrence(RepeatRule::Daily, d(1926, 1, 1), d(2026, 10, 8)),
            d(2026, 10, 8)
        );
        assert_eq!(
            next_occurrence(RepeatRule::Monthly, d(1926, 1, 31), d(2026, 10, 8)),
            d(2026, 10, 31)
        );
    }

    // ---- days_until and events ---------------------------------------------------------

    #[test]
    fn countdown_today_tomorrow_and_yesterday() {
        let today = d(2026, 10, 8);
        assert_eq!(days_until(today, today), 0);
        assert_eq!(days_until(d(2026, 10, 9), today), 1);
        assert_eq!(days_until(d(2026, 10, 7), today), -1);
        assert_eq!(days_until(d(2026, 12, 25), today), 78);
    }

    #[test]
    fn event_exactly_today_shows_zero_days() {
        let today = d(2026, 10, 8);
        let e = event(today, RepeatRule::Yearly);
        assert_eq!(event_target(&e, today), Some(today));
        assert!(event_is_visible(&e, today));
    }

    #[test]
    fn repeating_event_yesterday_rolls_to_next_year() {
        let today = d(2026, 10, 8);
        let e = event(d(2026, 10, 7), RepeatRule::Yearly);
        let target = event_target(&e, today).unwrap();
        assert_eq!(target, d(2027, 10, 7));
        assert_eq!(days_until(target, today), 364);
    }

    #[test]
    fn one_off_event_yesterday_hides_unless_kept() {
        let today = d(2026, 10, 8);
        let mut e = event(d(2026, 10, 7), RepeatRule::None);
        assert!(!event_is_visible(&e, today));
        assert_eq!(days_until(event_target(&e, today).unwrap(), today), -1);
        e.keep_after_pass = true;
        assert!(event_is_visible(&e, today));
    }

    // ---- tasks -------------------------------------------------------------------------

    #[test]
    fn overdue_tasks() {
        // Late in the evening: a date-only task due today is still on time.
        let today = d(2026, 10, 8);
        let now = today.and_hms_opt(23, 59, 0).unwrap();
        let mut t = Item::new(Kind::Task, "t", Utc::now());
        assert!(!is_overdue(&t, now), "no due date is never overdue");
        t.date = Some(today);
        assert!(!is_overdue(&t, now), "due today is not overdue yet");
        t.date = Some(d(2026, 10, 7));
        assert!(is_overdue(&t, now));
        t.completed_at = Some(Utc::now());
        assert!(!is_overdue(&t, now), "completed tasks are never overdue");
    }

    /// A task due on `date` at `h:m`.
    fn timed_task(date: NaiveDate, h: u32, m: u32) -> Item {
        let mut t = Item::new(Kind::Task, "standup", Utc::now());
        t.date = Some(date);
        t.due_time = NaiveTime::from_hms_opt(h, m, 0);
        t
    }

    #[test]
    fn timed_tasks_are_overdue_only_after_their_day() {
        // Due Thursday at 2 PM: on time all Thursday, even after 2 PM; overdue from midnight.
        let today = d(2026, 10, 8);
        let t = timed_task(today, 14, 0);
        let at = |h, m| today.and_hms_opt(h, m, 0).unwrap();
        assert!(!is_overdue(&t, at(13, 59)));
        assert!(!is_overdue(&t, at(14, 0)), "exactly on time is not late");
        assert!(
            !is_overdue(&t, at(14, 1)),
            "the time passing doesn't make it late"
        );
        assert!(!is_overdue(&t, at(23, 59)), "still its due day");
        assert!(is_overdue(&t, d(2026, 10, 9).and_hms_opt(0, 0, 0).unwrap()));
        assert_eq!(due_datetime(&t), Some(at(14, 0)));
    }

    #[test]
    fn timed_tasks_at_the_edges_of_the_day() {
        let today = d(2026, 10, 8);
        let tomorrow = d(2026, 10, 9);
        let at = |day: NaiveDate, h, m| day.and_hms_opt(h, m, 0).unwrap();

        // Due at 11:59 PM: on time at 11:59 PM, overdue one minute later (the next day).
        let last_minute = timed_task(today, 23, 59);
        assert!(!is_overdue(&last_minute, at(today, 23, 59)));
        assert!(is_overdue(&last_minute, at(tomorrow, 0, 0)));

        // Due at 12:00 AM: the time is passed for the whole day, but it's only overdue once
        // the day is over.
        let midnight = timed_task(today, 0, 0);
        assert!(!is_overdue(&midnight, at(today, 0, 0)));
        assert!(!is_overdue(&midnight, at(today, 23, 59)));
        assert!(is_overdue(&midnight, at(tomorrow, 0, 0)));
    }

    #[test]
    fn timed_tasks_on_the_nights_the_clocks_change() {
        // Times here are wall-clock times without a zone, so a daylight-saving change can't
        // move "the end of the day". These use the US dates in 2026.
        let at = |day: NaiveDate, h, m| day.and_hms_opt(h, m, 0).unwrap();

        // Spring forward, Sun Mar 8: 2:30 AM never shows on the clock. A task due then is not
        // late once the clock jumps to 3:00 AM, nor late that evening; overdue on Monday.
        let spring = d(2026, 3, 8);
        let gap = timed_task(spring, 2, 30);
        assert!(!is_overdue(&gap, at(spring, 3, 0)));
        assert!(!is_overdue(&gap, at(spring, 23, 59)));
        assert!(is_overdue(&gap, at(d(2026, 3, 9), 0, 0)));

        // Fall back, Sun Nov 1: 1:30 AM happens twice. Not late at either one, nor that
        // evening; overdue on Monday.
        let fall = d(2026, 11, 1);
        let twice = timed_task(fall, 1, 30);
        assert!(!is_overdue(&twice, at(fall, 1, 30)));
        assert!(!is_overdue(&twice, at(fall, 23, 59)));
        assert!(is_overdue(&twice, at(d(2026, 11, 2), 0, 0)));
    }

    /// Mon–Thu, the example rule from the design.
    fn mon_thu() -> RepeatRule {
        RepeatRule::Weekdays(crate::model::DaySet::MON_THU)
    }

    #[test]
    fn weekday_rule_completed_on_thursday_is_next_due_monday() {
        // Oct 2026: Mon 5, Tue 6, Wed 7, Thu 8, Fri 9, Sat 10, Sun 11, Mon 12.
        let two_pm = NaiveTime::from_hms_opt(14, 0, 0);
        let thu = d(2026, 10, 8);
        // Done Thursday morning (before 2 PM) or afternoon: either way, next is Monday 2 PM.
        for hour in [9, 16] {
            let now = thu.and_hms_opt(hour, 0, 0).unwrap();
            assert_eq!(
                next_instance_date(mon_thu(), Some(thu), two_pm, now),
                Some(d(2026, 10, 12)),
                "done at {hour}:00"
            );
        }
        // The date-only rule agrees.
        assert_eq!(
            next_task_date(mon_thu(), Some(thu), thu),
            Some(d(2026, 10, 12))
        );
    }

    #[test]
    fn weekday_rule_edge_cases() {
        let two_pm = NaiveTime::from_hms_opt(14, 0, 0);
        let at = |day: NaiveDate, h| day.and_hms_opt(h, 0, 0).unwrap();
        let (mon, tue, wed, thu) = (
            d(2026, 10, 5),
            d(2026, 10, 6),
            d(2026, 10, 7),
            d(2026, 10, 8),
        );
        let (sat, sun, next_mon) = (d(2026, 10, 10), d(2026, 10, 11), d(2026, 10, 12));

        // Late: due Mon, done Wed 10 AM -> Wed's 2 PM is still ahead, so Wed (missed Tue is
        // NOT back-filled). Done Wed 3 PM -> Thu.
        assert_eq!(
            next_instance_date(mon_thu(), Some(mon), two_pm, at(wed, 10)),
            Some(wed)
        );
        assert_eq!(
            next_instance_date(mon_thu(), Some(mon), two_pm, at(wed, 15)),
            Some(thu)
        );
        // Without a time, late completion skips to the first scheduled day after today.
        assert_eq!(next_task_date(mon_thu(), Some(mon), wed), Some(thu));
        // Done on a non-scheduled day (late, on Saturday): next Monday.
        assert_eq!(
            next_instance_date(mon_thu(), Some(thu), two_pm, at(sat, 10)),
            Some(next_mon)
        );
        // Done early on Sunday for Monday's: Monday's is done, so Tuesday.
        assert_eq!(
            next_instance_date(mon_thu(), Some(next_mon), two_pm, at(sun, 10)),
            Some(d(2026, 10, 13))
        );
        // Skipped a whole week: one next instance, after today, never a backlog.
        assert_eq!(
            next_task_date(mon_thu(), Some(mon), d(2026, 10, 14)),
            Some(d(2026, 10, 15))
        );
        // A single day behaves like weekly on that day.
        let tuesdays = RepeatRule::Weekdays(crate::model::DaySet::EMPTY.toggled(Weekday::Tue));
        assert_eq!(
            next_task_date(tuesdays, Some(tue), tue),
            Some(d(2026, 10, 13))
        );
        // Events count down to the next scheduled day on or after today.
        assert_eq!(next_occurrence(mon_thu(), mon, sat), next_mon);
        assert_eq!(next_occurrence(mon_thu(), mon, wed), wed);
    }

    #[test]
    fn unscheduled_start_dates_move_forward() {
        assert_eq!(
            scheduled_on_or_after(mon_thu(), d(2026, 10, 10)),
            d(2026, 10, 12)
        );
        assert_eq!(
            scheduled_on_or_after(mon_thu(), d(2026, 10, 7)),
            d(2026, 10, 7)
        );
        assert_eq!(
            scheduled_on_or_after(RepeatRule::Weekly, d(2026, 10, 10)),
            d(2026, 10, 10),
            "other rules are untouched"
        );
    }

    // ---- Nth weekdays --------------------------------------------------------------------

    /// Shorthand for an Nth-weekday rule (`month` 1..=12, or 0 for every month).
    fn nth(ordinal: Ordinal, weekday: Weekday, month: u8) -> RepeatRule {
        RepeatRule::NthWeekday {
            nth: ordinal,
            weekday,
            month: Month::try_from(month).ok(),
        }
    }

    /// The 4th Thursday of November, yearly.
    fn thanksgiving() -> RepeatRule {
        nth(Ordinal::Fourth, Weekday::Thu, 11)
    }

    #[test]
    fn thanksgiving_lands_on_the_right_date_2026_to_2030() {
        let anchor = d(2026, 11, 26);
        let expected = [
            d(2026, 11, 26),
            d(2027, 11, 25),
            d(2028, 11, 23),
            d(2029, 11, 22),
            d(2030, 11, 28),
        ];
        for date in expected {
            // Counted from New Year's Day, and on the day itself: that year's.
            assert_eq!(
                next_occurrence(thanksgiving(), anchor, d(date.year(), 1, 1)),
                date
            );
            assert_eq!(next_occurrence(thanksgiving(), anchor, date), date);
            assert_eq!(date.weekday(), Weekday::Thu);
        }
        // The day after, it rolls to next year's: Nov 27 2026 -> Nov 25 2027, 363 days.
        let next = next_occurrence(thanksgiving(), anchor, d(2026, 11, 27));
        assert_eq!(next, d(2027, 11, 25));
        assert_eq!(days_until(next, d(2026, 11, 27)), 363);
        // Still right from a very old anchor (computed directly, no stepping through years).
        assert_eq!(
            next_occurrence(thanksgiving(), d(1990, 11, 22), d(2030, 1, 1)),
            d(2030, 11, 28)
        );
    }

    #[test]
    fn last_monday_of_may_is_the_4th_or_the_5th() {
        let memorial_day = nth(Ordinal::Last, Weekday::Mon, 5);
        let anchor = d(2026, 5, 25);
        // 2026: May 25 (the 4th Monday). 2027: May 31 (the 5th). 2028: May 29 (the 5th).
        for date in [d(2026, 5, 25), d(2027, 5, 31), d(2028, 5, 29)] {
            assert_eq!(
                next_occurrence(memorial_day, anchor, d(date.year(), 1, 1)),
                date
            );
        }
        // "4th Monday" is not the same rule: May 24 2027, a week before.
        assert_eq!(
            next_occurrence(nth(Ordinal::Fourth, Weekday::Mon, 5), anchor, d(2027, 1, 1)),
            d(2027, 5, 24)
        );
    }

    #[test]
    fn other_holidays() {
        // Labor Day: 1st Monday of September.
        let labor_day = nth(Ordinal::First, Weekday::Mon, 9);
        assert_eq!(
            next_occurrence(labor_day, d(2026, 9, 7), d(2026, 9, 8)),
            d(2027, 9, 6)
        );
        // Mother's Day: 2nd Sunday of May.
        let mothers_day = nth(Ordinal::Second, Weekday::Sun, 5);
        assert_eq!(
            next_occurrence(mothers_day, d(2026, 5, 10), d(2027, 1, 1)),
            d(2027, 5, 9)
        );
        assert_eq!(
            next_occurrence(mothers_day, d(2026, 5, 10), d(2028, 1, 1)),
            d(2028, 5, 14)
        );
    }

    #[test]
    fn a_5th_weekday_exists_only_in_some_months_so_last_is_used_instead() {
        // October 2026 has five Fridays (2, 9, 16, 23, 30); February 2026 has four (it's
        // exactly 28 days and starts on a Sunday). chrono has no 5th Friday in February...
        assert_eq!(
            NaiveDate::from_weekday_of_month_opt(2026, 2, Weekday::Fri, 5),
            None
        );
        // ...which is why the rule has no 5th: "last" picks the 5th or the 4th.
        assert_eq!(
            nth_weekday_in(2026, 10, Ordinal::Last, Weekday::Fri),
            Some(d(2026, 10, 30))
        );
        assert_eq!(
            nth_weekday_in(2026, 2, Ordinal::Last, Weekday::Fri),
            Some(d(2026, 2, 27))
        );
        // The 1st to 4th exist in every month of a few years, including both kinds of Feb.
        for year in 2026..=2029 {
            for month in 1..=12 {
                for weekday in [Weekday::Mon, Weekday::Thu, Weekday::Sun] {
                    for ordinal in Ordinal::ALL {
                        let date = nth_weekday_in(year, month, ordinal, weekday);
                        assert!(
                            date.is_some_and(
                                |date| date.weekday() == weekday && date.month() == month
                            )
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn february_and_leap_years() {
        // Feb 2028 (leap) starts on a Tuesday: its last Tuesday is Feb 29.
        let last_tue_of_feb = nth(Ordinal::Last, Weekday::Tue, 2);
        assert_eq!(
            next_occurrence(last_tue_of_feb, d(2027, 2, 23), d(2028, 1, 1)),
            d(2028, 2, 29)
        );
        // The year after, Feb 2029 ends on a Wednesday: last Tuesday is Feb 27.
        assert_eq!(
            next_occurrence(last_tue_of_feb, d(2027, 2, 23), d(2028, 3, 1)),
            d(2029, 2, 27)
        );
        // Feb 2026 is exactly four weeks from a Sunday: its 4th and last Sunday agree.
        assert_eq!(
            nth_weekday_in(2026, 2, Ordinal::Fourth, Weekday::Sun),
            nth_weekday_in(2026, 2, Ordinal::Last, Weekday::Sun)
        );
    }

    #[test]
    fn every_month_rolls_over_months_and_the_year_end() {
        let first_friday = nth(Ordinal::First, Weekday::Fri, 0);
        let anchor = d(2026, 11, 6);
        assert_eq!(
            next_occurrence(first_friday, anchor, d(2026, 11, 3)),
            anchor
        );
        assert_eq!(
            next_occurrence(first_friday, anchor, d(2026, 11, 7)),
            d(2026, 12, 4)
        );
        // After December's, January's: Jan 1 2027 is a Friday (New Year's Day).
        assert_eq!(
            next_occurrence(first_friday, anchor, d(2026, 12, 5)),
            d(2027, 1, 1)
        );
        // Last Friday of December 2026 is Christmas; the day after, January's.
        let last_friday = nth(Ordinal::Last, Weekday::Fri, 0);
        assert_eq!(
            next_occurrence(last_friday, anchor, d(2026, 12, 26)),
            d(2027, 1, 29)
        );
        // A yearly December rule rolls into next December across the year end.
        let first_sun_dec = nth(Ordinal::First, Weekday::Sun, 12);
        assert_eq!(
            next_occurrence(first_sun_dec, d(2026, 12, 6), d(2026, 12, 31)),
            d(2027, 12, 5)
        );
    }

    #[test]
    fn nth_weekday_tasks_done_early_on_time_or_late() {
        let due = d(2026, 11, 26);
        // On the day, early (a week before) or late (Monday after): next year's.
        for today in [due, d(2026, 11, 19), d(2026, 11, 30)] {
            assert_eq!(
                next_task_date(thanksgiving(), Some(due), today),
                Some(d(2027, 11, 25)),
                "done {today}"
            );
        }
        // Every month, done late in the next month: the first one after today (no backlog).
        let first_friday = nth(Ordinal::First, Weekday::Fri, 0);
        assert_eq!(
            next_task_date(first_friday, Some(d(2026, 11, 6)), d(2026, 12, 10)),
            Some(d(2027, 1, 1))
        );
        // With a due time: due Nov 6 at 2 PM, done Dec 4 at 10 AM -> Dec 4's 2 PM is still
        // ahead, so it's today; done at 3 PM -> January's.
        let two_pm = NaiveTime::from_hms_opt(14, 0, 0);
        let dec_4 = d(2026, 12, 4);
        assert_eq!(
            next_instance_date(
                first_friday,
                Some(d(2026, 11, 6)),
                two_pm,
                dec_4.and_hms_opt(10, 0, 0).unwrap()
            ),
            Some(dec_4)
        );
        assert_eq!(
            next_instance_date(
                first_friday,
                Some(d(2026, 11, 6)),
                two_pm,
                dec_4.and_hms_opt(15, 0, 0).unwrap()
            ),
            Some(d(2027, 1, 1))
        );
    }

    #[test]
    fn nth_weekday_on_the_nights_the_clocks_change() {
        // The 2nd Sunday of March and the 1st Sunday of November are the US clock-change
        // days themselves (Mar 8 and Nov 1 in 2026). The math is on plain dates and times, so
        // a time that never shows (2:30 AM in spring) or shows twice (1:30 AM in fall) is
        // still a normal due time: done just after it, the next one is next year's.
        let spring = nth(Ordinal::Second, Weekday::Sun, 3);
        let fall = nth(Ordinal::First, Weekday::Sun, 11);
        let at = |day: NaiveDate, h, m| day.and_hms_opt(h, m, 0).unwrap();
        assert_eq!(
            next_occurrence(spring, d(2026, 3, 8), d(2027, 1, 1)),
            d(2027, 3, 14)
        );
        assert_eq!(
            next_instance_date(
                spring,
                Some(d(2026, 3, 8)),
                NaiveTime::from_hms_opt(2, 30, 0),
                at(d(2026, 3, 8), 3, 0)
            ),
            Some(d(2027, 3, 14))
        );
        // Fall: done at 1:00 AM (early) or 2:00 AM (after either 1:30) counts as this one,
        // so the next is the 1st Sunday of November 2027.
        for (h, m) in [(1, 0), (2, 0)] {
            assert_eq!(
                next_instance_date(
                    fall,
                    Some(d(2026, 11, 1)),
                    NaiveTime::from_hms_opt(1, 30, 0),
                    at(d(2026, 11, 1), h, m)
                ),
                Some(d(2027, 11, 7)),
                "done at {h}:{m:02}"
            );
        }
    }

    #[test]
    fn unscheduled_start_dates_move_to_the_nth_weekday() {
        // Nov 19 2026 is the 3rd Thursday: saves as the 4th, Nov 26.
        assert_eq!(
            scheduled_on_or_after(thanksgiving(), d(2026, 11, 19)),
            d(2026, 11, 26)
        );
        assert_eq!(
            scheduled_on_or_after(thanksgiving(), d(2026, 11, 26)),
            d(2026, 11, 26)
        );
        // After this year's, next year's.
        assert_eq!(
            scheduled_on_or_after(thanksgiving(), d(2026, 11, 27)),
            d(2027, 11, 25)
        );
        // Every month: Nov 3 2026 -> Fri Nov 6.
        assert_eq!(
            scheduled_on_or_after(nth(Ordinal::First, Weekday::Fri, 0), d(2026, 11, 3)),
            d(2026, 11, 6)
        );
    }

    #[test]
    fn nth_weekday_events_count_down_and_stay_visible() {
        let mut item = event(d(2026, 11, 26), thanksgiving());
        assert_eq!(event_target(&item, d(2026, 11, 3)), Some(d(2026, 11, 26)));
        assert_eq!(event_target(&item, d(2026, 11, 27)), Some(d(2027, 11, 25)));
        assert!(event_is_visible(&item, d(2026, 11, 27)));
        item.repeat = RepeatRule::None;
        assert!(!event_is_visible(&item, d(2026, 11, 27)), "one-off hides");
    }

    #[test]
    fn next_instance_with_a_due_time() {
        let two_pm = NaiveTime::from_hms_opt(14, 0, 0);
        let at = |day: NaiveDate, h| day.and_hms_opt(h, 0, 0).unwrap();
        let (wed, thu, fri) = (d(2026, 10, 7), d(2026, 10, 8), d(2026, 10, 9));

        // Daily 2 PM, due yesterday, done this morning: today's 2 PM is still ahead.
        assert_eq!(
            next_instance_date(RepeatRule::Daily, Some(wed), two_pm, at(thu, 10)),
            Some(thu)
        );
        // Same, but done after 2 PM: tomorrow.
        assert_eq!(
            next_instance_date(RepeatRule::Daily, Some(wed), two_pm, at(thu, 15)),
            Some(fri)
        );
        // Due today at 2 PM, done early at 9 AM: you did today's, so tomorrow.
        assert_eq!(
            next_instance_date(RepeatRule::Daily, Some(thu), two_pm, at(thu, 9)),
            Some(fri)
        );
        // Weekly, done days early: the week after the one you did.
        assert_eq!(
            next_instance_date(RepeatRule::Weekly, Some(fri), two_pm, at(wed, 9)),
            Some(d(2026, 10, 16))
        );
        // No time: the date-only rule, unchanged.
        assert_eq!(
            next_instance_date(RepeatRule::Daily, Some(wed), None, at(thu, 10)),
            next_task_date(RepeatRule::Daily, Some(wed), thu)
        );
        // Not repeating: no next instance.
        assert_eq!(
            next_instance_date(RepeatRule::None, Some(wed), two_pm, at(thu, 10)),
            None
        );
    }

    #[test]
    fn next_task_date_for_on_time_late_and_early_completion() {
        let today = d(2026, 10, 8); // a Thursday
        // Daily task due today, done today -> due tomorrow.
        assert_eq!(
            next_task_date(RepeatRule::Daily, Some(today), today),
            Some(d(2026, 10, 9))
        );
        // Daily task 5 days overdue, done today -> due tomorrow (not today again).
        assert_eq!(
            next_task_date(RepeatRule::Daily, Some(d(2026, 10, 3)), today),
            Some(d(2026, 10, 9))
        );
        // Weekly task due Monday Oct 5, done late on Thursday -> next Monday Oct 12.
        assert_eq!(
            next_task_date(RepeatRule::Weekly, Some(d(2026, 10, 5)), today),
            Some(d(2026, 10, 12))
        );
        // Weekly task due Monday Oct 12, done early on Thursday -> Monday Oct 19.
        assert_eq!(
            next_task_date(RepeatRule::Weekly, Some(d(2026, 10, 12)), today),
            Some(d(2026, 10, 19))
        );
        // Monthly on the 31st keeps its anchor.
        assert_eq!(
            next_task_date(RepeatRule::Monthly, Some(d(2026, 1, 31)), d(2026, 1, 31)),
            Some(d(2026, 2, 28))
        );
        // No due date: anchored on today.
        assert_eq!(
            next_task_date(RepeatRule::Weekly, None, today),
            Some(d(2026, 10, 15))
        );
        // Not repeating: no new instance.
        assert_eq!(next_task_date(RepeatRule::None, Some(today), today), None);
    }

    // ---- parse_date_input --------------------------------------------------------------

    #[test]
    fn parses_typed_dates() {
        let today = d(2026, 10, 8); // Thursday
        let p = |s: &str| parse_date_input(s, today);
        assert_eq!(p(""), Ok(None));
        assert_eq!(p("  "), Ok(None));
        assert_eq!(p("today"), Ok(Some(today)));
        assert_eq!(p("Tomorrow"), Ok(Some(d(2026, 10, 9))));
        assert_eq!(p("2026-12-25"), Ok(Some(d(2026, 12, 25))));
        assert_eq!(p("12/25/2026"), Ok(Some(d(2026, 12, 25))));
        assert_eq!(p("12-25"), Ok(Some(d(2026, 12, 25))));
        assert_eq!(
            p("1/5"),
            Ok(Some(d(2027, 1, 5))),
            "already passed this year -> next year"
        );
        assert_eq!(p("10/8"), Ok(Some(today)), "today counts as on-or-after");
        assert_eq!(
            p("2/29"),
            Ok(Some(d(2028, 2, 29))),
            "next Feb 29 is in 2028"
        );
        assert_eq!(p("+3"), Ok(Some(d(2026, 10, 11))));
        assert_eq!(p("+3d"), Ok(Some(d(2026, 10, 11))));
        assert_eq!(p("+2w"), Ok(Some(d(2026, 10, 22))));
        assert_eq!(p("+1m"), Ok(Some(d(2026, 11, 8))));
        assert_eq!(p("+1y"), Ok(Some(d(2027, 10, 8))));
        assert_eq!(p("fri"), Ok(Some(d(2026, 10, 9))));
        assert_eq!(
            p("thursday"),
            Ok(Some(d(2026, 10, 15))),
            "same weekday means next week"
        );
        assert!(p("someday").is_err());
        assert!(p("13/45").is_err());
        assert!(p("+3x").is_err());
        assert!(p("2026-02-30").is_err());
    }
}
