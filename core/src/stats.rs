//! Statistics: totals, streaks, weekly counts, and overdue tasks.
//!
//! Everything (except the overdue count) is computed from the completion log, never stored.
//! That means a new kind of statistic is just a new function here — no database migration.
//!
//! [`compute`] takes all its inputs as parameters (the log, the items, today's date, the time
//! zone) and returns a plain [`Stats`] struct. No clock, no database, no UI — so it's easy to
//! test and the Android app can call it unchanged.

use std::collections::BTreeSet;

use chrono::{Datelike, NaiveDate, TimeDelta, TimeZone, Utc};

use crate::model::{Completion, Item, Priority};
use crate::repeat::is_overdue;

/// How many weeks the "completions per week" chart covers.
pub const WEEKS_SHOWN: usize = 12;

/// Number of completions in one week (weeks start on Monday).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeekCount {
    /// The Monday that starts this week.
    pub week_start: NaiveDate,
    /// Completions during that Monday–Sunday.
    pub count: u32,
}

/// Completions split by the task's priority at the time it was completed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PriorityBreakdown {
    pub low: u32,
    pub medium: u32,
    pub high: u32,
}

/// Every statistic the stats screen shows.
///
/// `#[derive(Default)]` gives a "zero" value (all counts 0, empty list) for free.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// Completions ever.
    pub total_all: u32,
    /// Completions this week (Monday to today).
    pub total_week: u32,
    /// Completions this calendar month.
    pub total_month: u32,
    /// The last [`WEEKS_SHOWN`] weeks, oldest first; the last entry is the current week.
    pub per_week: Vec<WeekCount>,
    /// Consecutive days with at least one completion, ending today (or yesterday — see below).
    pub current_streak: u32,
    /// The longest such run ever.
    pub longest_streak: u32,
    /// Open tasks whose due date is before today.
    pub overdue_count: u32,
    /// Completions that happened on or before the due date (or had no due date).
    pub on_time: u32,
    /// Completions that happened after the due date.
    pub late: u32,
    /// Completions by priority.
    pub by_priority: PriorityBreakdown,
}

impl Stats {
    /// Share of completions done on time, from 0.0 to 1.0, or `None` if nothing's done yet
    /// (avoids dividing by zero and showing a meaningless "0%").
    pub fn on_time_rate(&self) -> Option<f64> {
        let total = self.on_time + self.late;
        if total == 0 {
            None
        } else {
            Some(f64::from(self.on_time) / f64::from(total))
        }
    }
}

/// The Monday on or before `date`.
pub fn week_start(date: NaiveDate) -> NaiveDate {
    // `num_days_from_monday()` is 0 for Monday ... 6 for Sunday.
    date - TimeDelta::days(i64::from(date.weekday().num_days_from_monday()))
}

/// Computes all statistics.
///
/// - `completions`: the whole completion log (`Db::list_completions`).
/// - `items`: all non-deleted items (`Db::list_items`), used for the overdue count.
/// - `today`: the user's local date.
/// - `tz`: the user's time zone, used to decide which *local* day a completion happened on.
///   Completions are stored in UTC; finishing a task at 11pm in Denver is already "tomorrow" in
///   UTC, and it should count for today.
///
/// `<Tz: TimeZone>` makes this function *generic*: it works with any type that implements the
/// `TimeZone` trait (a trait is a set of abilities a type promises to have). The app passes
/// `chrono::Local`; tests pass `Utc` so results don't depend on the machine running them.
pub fn compute<Tz: TimeZone>(
    completions: &[Completion],
    items: &[Item],
    today: NaiveDate,
    tz: &Tz,
) -> Stats {
    let mut stats = Stats::default();

    // Convert each completion's UTC timestamp to the local calendar day it happened on.
    // This is an *iterator chain*: `.iter()` yields each element, `.map(...)` transforms it,
    // and `.collect()` gathers the results into a Vec. Nothing runs until `collect`.
    let days: Vec<NaiveDate> = completions
        .iter()
        .map(|c| local_day(c.completed_at, tz))
        .collect();

    // ---- totals --------------------------------------------------------------------------
    let this_week = week_start(today);
    stats.total_all = count_u32(days.len());
    stats.total_week = count_u32(
        days.iter()
            .filter(|&&d| d >= this_week && d <= today)
            .count(),
    );
    stats.total_month = count_u32(
        days.iter()
            .filter(|d| d.year() == today.year() && d.month() == today.month() && **d <= today)
            .count(),
    );

    // ---- per week (bar chart) ------------------------------------------------------------
    // Build the 12 Mondays, oldest first, then count completions falling in each week.
    stats.per_week = (0..WEEKS_SHOWN)
        .rev() // 11, 10, ..., 0 weeks ago -> oldest first
        .map(|weeks_ago| {
            let start = this_week - TimeDelta::weeks(weeks_ago as i64);
            let end = start + TimeDelta::days(7);
            let count = days.iter().filter(|&&d| d >= start && d < end).count();
            WeekCount {
                week_start: start,
                count: count_u32(count),
            }
        })
        .collect();

    // ---- streaks -------------------------------------------------------------------------
    // A BTreeSet keeps unique values in sorted order: exactly "the distinct days with at
    // least one completion, earliest first".
    let unique_days: BTreeSet<NaiveDate> = days.iter().copied().collect();
    stats.longest_streak = longest_run(&unique_days);
    stats.current_streak = current_run(&unique_days, today);

    // ---- on time vs late, and priorities ------------------------------------------------
    for c in completions {
        if c.was_overdue {
            stats.late += 1;
        } else {
            stats.on_time += 1;
        }
        match c.priority_snapshot {
            Priority::Low => stats.by_priority.low += 1,
            Priority::Medium => stats.by_priority.medium += 1,
            Priority::High => stats.by_priority.high += 1,
        }
    }

    // ---- overdue (from the live items, not the log) -------------------------------------
    stats.overdue_count = count_u32(
        items
            .iter()
            .filter(|i| !i.deleted && is_overdue(i, today))
            .count(),
    );

    stats
}

/// The local calendar date of a UTC timestamp in the given time zone.
fn local_day<Tz: TimeZone>(ts: chrono::DateTime<Utc>, tz: &Tz) -> NaiveDate {
    ts.with_timezone(tz).date_naive()
}

/// Longest run of consecutive days in the (sorted) set.
fn longest_run(days: &BTreeSet<NaiveDate>) -> u32 {
    let mut longest = 0;
    let mut run = 0;
    let mut previous: Option<NaiveDate> = None;
    for &day in days {
        // Continue the run if this day is exactly one after the previous; otherwise restart.
        run = match previous {
            Some(prev) if day - prev == TimeDelta::days(1) => run + 1,
            _ => 1,
        };
        longest = longest.max(run);
        previous = Some(day);
    }
    longest
}

/// The streak that is still "alive" today.
///
/// If you've completed something today, the streak counts back from today. If you haven't
/// *yet*, it counts back from yesterday — the day isn't over, so the streak isn't broken.
fn current_run(days: &BTreeSet<NaiveDate>, today: NaiveDate) -> u32 {
    let yesterday = today - TimeDelta::days(1);
    let mut day = if days.contains(&today) {
        today
    } else if days.contains(&yesterday) {
        yesterday
    } else {
        return 0;
    };
    let mut run = 0;
    while days.contains(&day) {
        run += 1;
        day -= TimeDelta::days(1);
    }
    run
}

/// Converts a count (`usize`) to `u32`, saturating instead of wrapping. Nobody will complete
/// four billion tasks, but this keeps the conversion honest without an `unwrap`.
fn count_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, new_id};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    /// A completion at noon UTC on the given date.
    fn done_on(date: NaiveDate, priority: Priority, was_overdue: bool) -> Completion {
        Completion {
            id: new_id(),
            item_id: new_id(),
            completed_at: date.and_hms_opt(12, 0, 0).unwrap().and_utc(),
            title_snapshot: "x".into(),
            priority_snapshot: priority,
            was_overdue,
        }
    }

    fn simple(date: NaiveDate) -> Completion {
        done_on(date, Priority::Medium, false)
    }

    const TODAY: (i32, u32, u32) = (2026, 10, 8); // a Thursday

    fn today() -> NaiveDate {
        d(TODAY.0, TODAY.1, TODAY.2)
    }

    #[test]
    fn empty_log_gives_zeroes() {
        let s = compute(&[], &[], today(), &Utc);
        assert_eq!(s.total_all, 0);
        assert_eq!(s.current_streak, 0);
        assert_eq!(s.longest_streak, 0);
        assert_eq!(s.per_week.len(), WEEKS_SHOWN);
        assert!(s.per_week.iter().all(|w| w.count == 0));
        assert_eq!(s.on_time_rate(), None);
    }

    #[test]
    fn week_starts_on_monday() {
        assert_eq!(week_start(d(2026, 10, 8)), d(2026, 10, 5)); // Thu -> Mon
        assert_eq!(week_start(d(2026, 10, 5)), d(2026, 10, 5)); // Mon -> itself
        assert_eq!(week_start(d(2026, 10, 11)), d(2026, 10, 5)); // Sun -> previous Mon
        assert_eq!(week_start(d(2027, 1, 1)), d(2026, 12, 28)); // across New Year
    }

    #[test]
    fn totals_for_week_and_month() {
        let log = [
            simple(d(2026, 10, 8)), // today
            simple(d(2026, 10, 5)), // Monday this week
            simple(d(2026, 10, 4)), // Sunday last week, but this month
            simple(d(2026, 9, 30)), // last month
        ];
        let s = compute(&log, &[], today(), &Utc);
        assert_eq!(s.total_all, 4);
        assert_eq!(s.total_week, 2);
        assert_eq!(s.total_month, 3);
    }

    #[test]
    fn per_week_buckets_oldest_first() {
        let log = [
            simple(d(2026, 10, 8)),
            simple(d(2026, 10, 6)),
            simple(d(2026, 9, 29)), // last week
            simple(d(2026, 7, 20)), // exactly 11 weeks before this Monday -> first bucket
            simple(d(2026, 7, 19)), // 12 weeks ago -> outside the chart
        ];
        let s = compute(&log, &[], today(), &Utc);
        assert_eq!(s.per_week.len(), 12);
        assert_eq!(s.per_week[11].week_start, d(2026, 10, 5));
        assert_eq!(s.per_week[11].count, 2);
        assert_eq!(s.per_week[10].count, 1);
        assert_eq!(s.per_week[0].week_start, d(2026, 7, 20));
        assert_eq!(s.per_week[0].count, 1);
        let charted: u32 = s.per_week.iter().map(|w| w.count).sum();
        assert_eq!(charted, 4);
    }

    #[test]
    fn streaks_count_consecutive_days() {
        let log = [
            // A 4-day run long ago.
            simple(d(2026, 9, 1)),
            simple(d(2026, 9, 2)),
            simple(d(2026, 9, 3)),
            simple(d(2026, 9, 4)),
            // A gap, then a current 3-day run (two completions on one day count once).
            simple(d(2026, 10, 6)),
            simple(d(2026, 10, 7)),
            simple(d(2026, 10, 7)),
            simple(d(2026, 10, 8)),
        ];
        let s = compute(&log, &[], today(), &Utc);
        assert_eq!(s.longest_streak, 4);
        assert_eq!(s.current_streak, 3);
    }

    #[test]
    fn streak_survives_until_today_ends() {
        // Nothing done today yet, but yesterday and the day before: streak is still 2.
        let log = [simple(d(2026, 10, 6)), simple(d(2026, 10, 7))];
        assert_eq!(compute(&log, &[], today(), &Utc).current_streak, 2);
        // Last completion two days ago: streak broken.
        let log = [simple(d(2026, 10, 6))];
        assert_eq!(compute(&log, &[], today(), &Utc).current_streak, 0);
    }

    #[test]
    fn local_time_zone_decides_the_day() {
        // 03:00 UTC on Monday Oct 5 is still Sunday Oct 4 in UTC-6 (e.g. Denver in summer),
        // which belongs to LAST week.
        let mut c = simple(d(2026, 10, 5));
        c.completed_at = d(2026, 10, 5).and_hms_opt(3, 0, 0).unwrap().and_utc();
        let denver = chrono::FixedOffset::west_opt(6 * 3600).unwrap();
        assert_eq!(compute(&[c.clone()], &[], today(), &Utc).total_week, 1);
        assert_eq!(compute(&[c], &[], today(), &denver).total_week, 0);
    }

    #[test]
    fn on_time_rate_and_priorities() {
        let log = [
            done_on(d(2026, 10, 1), Priority::High, false),
            done_on(d(2026, 10, 2), Priority::High, true),
            done_on(d(2026, 10, 3), Priority::Low, false),
            done_on(d(2026, 10, 4), Priority::Medium, false),
        ];
        let s = compute(&log, &[], today(), &Utc);
        assert_eq!(s.on_time, 3);
        assert_eq!(s.late, 1);
        assert_eq!(s.on_time_rate(), Some(0.75));
        assert_eq!(
            s.by_priority,
            PriorityBreakdown {
                low: 1,
                medium: 1,
                high: 2
            }
        );
    }

    #[test]
    fn overdue_counts_only_open_past_due_tasks() {
        let now = Utc::now();
        let mut overdue = Item::new(Kind::Task, "late", now);
        overdue.date = Some(d(2026, 10, 1));
        let mut due_today = Item::new(Kind::Task, "today", now);
        due_today.date = Some(today());
        let mut done = Item::new(Kind::Task, "done", now);
        done.date = Some(d(2026, 10, 1));
        done.completed_at = Some(now);
        let mut past_event = Item::new(Kind::Event, "event", now);
        past_event.date = Some(d(2026, 10, 1));
        let mut deleted = overdue.clone();
        deleted.deleted = true;

        let items = [overdue, due_today, done, past_event, deleted];
        assert_eq!(compute(&[], &items, today(), &Utc).overdue_count, 1);
    }
}
