//! Sample data for `zerohour --demo`, so the layout can be judged (and screenshotted) right away.
//!
//! Everything here is fictional: no real people, places, employers or servers. All dates are
//! relative to "now", so the demo always looks current:
//!
//! - eight open tasks with a mix of priorities: one due later today at a set time, one overdue,
//!   one repeating Mon–Thu at 2:00 PM, one with notes;
//! - five upcoming events: Christmas (yearly), a trip, a concert, a product launch, and a
//!   monthly book club. None of them is in the past;
//! - twelve weeks of completion history for the stats screen: busier and quieter weeks, a
//!   current streak of exactly [`STREAK_DAYS`] days, a few late completions, and a priority mix
//!   of roughly 40% Medium, 35% High and 25% Low.
//!
//! `main.rs` only ever seeds an in-memory database (or a file the person named with `--db`), and
//! demo mode never syncs, so none of this can reach the real database, server or keyring.

use anyhow::Result;
use chrono::{
    DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Timelike, Utc,
};
use zerohour_core::db::Db;
use zerohour_core::model::{Completion, DaySet, Item, Kind, Priority, RepeatRule, Unit, new_id};
use zerohour_core::stats::{WEEKS_SHOWN, week_start};

/// Length of the "current streak" the stats screen shows: the last this-many days (ending
/// yesterday) all have completions, and the day before them has none.
pub const STREAK_DAYS: i64 = 9;

/// How busy each of the twelve weeks in the history is, oldest week first. Multiplied by
/// [`DAY_SHAPE`] to get completions per day, so the weekly bar chart has some ups and downs.
const WEEK_LOAD: [u32; WEEKS_SHOWN] = [3, 5, 2, 4, 6, 3, 5, 8, 6, 3, 7, 5];

/// How busy each weekday is, Monday first. Sundays are always zero (a day off), which also keeps
/// every older streak shorter than the current one.
const DAY_SHAPE: [u32; 7] = [3, 4, 3, 4, 2, 1, 0];

/// Priorities handed out to completed tasks in turn: 8 Medium, 7 High, 5 Low out of every 20,
/// i.e. 40% / 35% / 25%. Interleaved so neighbouring days get a mix.
const PRIORITY_CYCLE: [Priority; 20] = {
    // `use` inside a block only applies to that block; it keeps the table below readable.
    use Priority::{High as H, Low as L, Medium as M};
    [M, H, M, L, H, M, H, L, M, H, M, H, L, M, H, M, L, H, M, L]
};

/// Every this-many completions, one was done after its due date ("late" on the stats screen).
const LATE_EVERY: usize = 16;

/// Generic titles for the completed tasks in the history.
const HISTORY_TITLES: [&str; 10] = [
    "Take out recycling",
    "Reply to emails",
    "Grocery shopping",
    "Pay phone bill",
    "Clean the kitchen",
    "Review pull request",
    "Go for a run",
    "Update budget spreadsheet",
    "Call the dentist",
    "Water the plants",
];

/// One sample task: (title, priority, due date, due time, repeat, notes).
/// A `type` alias just gives a long type a short name.
type SampleTask = (
    &'static str,
    Priority,
    Option<NaiveDate>,
    Option<NaiveTime>,
    RepeatRule,
    Option<&'static str>,
);

/// One sample event: (title, date, repeat, notes).
type SampleEvent = (&'static str, NaiveDate, RepeatRule, Option<&'static str>);

/// Fills the database with sample tasks, events, and completion history.
///
/// `now` is the local date and time, passed in (rather than read from the clock) so tests can
/// seed the demo as of any moment they like.
pub fn seed(db: &Db, now: NaiveDateTime) -> Result<()> {
    let today = now.date();
    let created = local_to_utc(now);
    // A tiny helper: today + n days (negative n = in the past).
    let day = |n: i64| today + TimeDelta::days(n);

    // ---- tasks ---------------------------------------------------------------------------
    let due_today = later_today(now);
    let report_time = report_time();
    let tasks: [SampleTask; 8] = [
        (
            "Send project update",
            Priority::High,
            Some(due_today.date()),
            Some(due_today.time()),
            RepeatRule::None,
            None,
        ),
        // The one overdue task: it was due yesterday.
        (
            "Pay electricity bill",
            Priority::High,
            Some(day(-1)),
            None,
            RepeatRule::None,
            None,
        ),
        (
            "Submit weekly report",
            Priority::Medium,
            Some(next_report_day(now)),
            Some(report_time),
            RepeatRule::Weekdays(DaySet::MON_THU),
            None,
        ),
        (
            "Book a cabin for the lake trip",
            Priority::Medium,
            Some(day(6)),
            None,
            RepeatRule::None,
            Some("Two nights, close to the trailhead. Check the cancellation policy first."),
        ),
        (
            "Renew car insurance",
            Priority::High,
            Some(day(4)),
            None,
            RepeatRule::None,
            None,
        ),
        (
            "Water the plants",
            Priority::Low,
            Some(day(2)),
            None,
            RepeatRule::Weekly,
            None,
        ),
        (
            "Back up laptop",
            Priority::Medium,
            Some(day(12)),
            None,
            RepeatRule::EveryN {
                n: 2,
                unit: Unit::Weeks,
            },
            None,
        ),
        (
            "Finish book club novel",
            Priority::Low,
            None,
            None,
            RepeatRule::None,
            None,
        ),
    ];
    for (title, priority, date, due_time, repeat, notes) in tasks {
        let mut item = Item::new(Kind::Task, title, created);
        item.priority = priority;
        item.date = date;
        item.due_time = due_time;
        item.repeat = repeat;
        // `Option::map` turns `Some(&str)` into `Some(String)` and leaves `None` alone.
        item.notes = notes.map(str::to_string);
        db.insert_item(&item)?;
    }

    // ---- events --------------------------------------------------------------------------
    // Every event, repeating or not, is stored at its NEXT occurrence, so no date is in the past.
    let events: [SampleEvent; 5] = [
        ("Christmas", next_christmas(today), RepeatRule::Yearly, None),
        ("Product launch", day(5), RepeatRule::None, None),
        ("Book club night", day(8), RepeatRule::Monthly, None),
        ("Jazz concert in the park", day(13), RepeatRule::None, None),
        (
            "Weekend trip to the lake",
            day(26),
            RepeatRule::None,
            Some("Pack the hiking boots and a rain jacket"),
        ),
    ];
    for (title, date, repeat, notes) in events {
        let mut item = Item::new(Kind::Event, title, created);
        item.date = Some(date);
        item.repeat = repeat;
        item.notes = notes.map(str::to_string);
        db.insert_item(&item)?;
    }

    // ---- completion history --------------------------------------------------------------
    // Walk every day from the Monday that starts the oldest week on the chart up to yesterday.
    // (Nothing is completed "today": a completion later than `now` would be in the future.)
    let first_day = week_start(today) - TimeDelta::weeks(WEEKS_SHOWN as i64 - 1);
    let mut n = 0usize;
    let mut date = first_day;
    while date < today {
        for k in 0..completions_on(date, today, first_day) {
            let completed_at = local_to_utc(date.and_time(afternoon(k)));
            let priority = PRIORITY_CYCLE[n % PRIORITY_CYCLE.len()];
            let title = HISTORY_TITLES[n % HISTORY_TITLES.len()];
            let late = n % LATE_EVERY == 5;

            // The completed task itself (hidden from the list, kept as history). A late one
            // was due two days before it was done.
            let mut item = Item::new(Kind::Task, title, completed_at);
            item.priority = priority;
            item.date = Some(if late {
                date - TimeDelta::days(2)
            } else {
                date
            });
            item.completed_at = Some(completed_at);
            db.insert_item(&item)?;
            // ...and its entry in the completion log, which is what stats read.
            db.insert_completion(&Completion {
                id: new_id(),
                item_id: item.id,
                completed_at,
                title_snapshot: title.to_string(),
                priority_snapshot: priority,
                was_overdue: late,
            })?;
            n += 1;
        }
        date += TimeDelta::days(1);
    }
    Ok(())
}

/// How many tasks were completed on `date` in the demo history.
///
/// The last [`STREAK_DAYS`] days before `today` always get one or two (the live streak), the day
/// before them gets none (so the streak is exactly that long), and older days follow
/// `WEEK_LOAD × DAY_SHAPE`.
fn completions_on(date: NaiveDate, today: NaiveDate, first_day: NaiveDate) -> u32 {
    let days_ago = (today - date).num_days();
    if days_ago <= STREAK_DAYS {
        // Alternate 1, 2, 1, 2... so the streak days aren't all identical.
        1 + (days_ago % 2) as u32
    } else if days_ago == STREAK_DAYS + 1 {
        0
    } else {
        // Which of the twelve weeks this is (0 = oldest), and which weekday (0 = Monday).
        let week = ((date - first_day).num_days() / 7) as usize;
        let weekday = date.weekday().num_days_from_monday() as usize;
        WEEK_LOAD[week] * DAY_SHAPE[weekday] / 8
    }
}

/// 2:00 PM, plus `k` hours for the k-th completion of a day (capped at 10 PM).
fn afternoon(k: u32) -> NaiveTime {
    // 14..=22 is always a valid hour, so the fallback (midnight) never happens.
    NaiveTime::from_hms_opt(14 + k.min(8), 0, 0).unwrap_or_default()
}

/// 2:00 PM: when "Submit weekly report" is due.
fn report_time() -> NaiveTime {
    // 14:00:00 is a valid time, so the fallback never happens.
    NaiveTime::from_hms_opt(14, 0, 0).unwrap_or_default()
}

/// The next Mon–Thu day the weekly report is due: today if today is Mon–Thu and it's not yet
/// 2:00 PM, otherwise the next Mon–Thu. This keeps the report from showing up as overdue.
fn next_report_day(now: NaiveDateTime) -> NaiveDate {
    let today = now.date();
    let from = if now.time() < report_time() {
        today
    } else {
        today + TimeDelta::days(1)
    };
    // MON_THU is never empty, so `first_on_or_after` always finds a day within a week.
    DaySet::MON_THU.first_on_or_after(from).unwrap_or(from)
}

/// A due time later today, on the hour, roughly two hours from `now` (so the list shows
/// "today 4:00 PM · in 2h"). Late in the evening it becomes 11:59 PM, so it stays today.
fn later_today(now: NaiveDateTime) -> NaiveDateTime {
    let in_two_hours = now + TimeDelta::hours(2);
    // Round down to the hour. `with_minute(0)` etc. return an `Option` (they'd fail for an
    // out-of-range value); zero is always valid, so the fallback never runs.
    let on_the_hour = in_two_hours
        .with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(in_two_hours);
    if on_the_hour.date() == now.date() {
        on_the_hour
    } else {
        // 23:59:00 is a valid time, so the fallback never happens.
        now.date()
            .and_time(NaiveTime::from_hms_opt(23, 59, 0).unwrap_or_default())
    }
}

/// The next Christmas on or after `today` (this year's, or next year's once it has passed).
fn next_christmas(today: NaiveDate) -> NaiveDate {
    // Dec 25 exists in every year, so the fallbacks never happen.
    let this_year = NaiveDate::from_ymd_opt(today.year(), 12, 25).unwrap_or(today);
    if this_year >= today {
        this_year
    } else {
        NaiveDate::from_ymd_opt(today.year() + 1, 12, 25).unwrap_or(today)
    }
}

/// Converts a local wall-clock time to UTC for storage.
///
/// Going through local time makes sure a completion lands on the intended day in YOUR time
/// zone, which is the zone the stats screen uses.
fn local_to_utc(naive: NaiveDateTime) -> DateTime<Utc> {
    // `.earliest()` picks a time even on the rare DST-change hour where a local time is
    // ambiguous; if it doesn't exist at all, treat it as UTC (only a demo).
    naive
        .and_local_timezone(Local)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|| naive.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerohour_core::repeat::is_overdue;
    use zerohour_core::stats::{self, Stats};

    /// A spread of moments to seed the demo at: a Monday morning, a Thursday after the report
    /// is due, a late Sunday evening, the day after Christmas, and New Year's Day.
    fn moments() -> Vec<NaiveDateTime> {
        [
            (2026, 10, 5, 8, 0),
            (2026, 10, 8, 15, 30),
            (2026, 10, 11, 23, 30),
            (2026, 12, 26, 10, 0),
            (2027, 1, 1, 0, 5),
        ]
        .into_iter()
        .map(|(y, m, d, h, min)| {
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, min, 0)
                .unwrap()
        })
        .collect()
    }

    /// Seeds a fresh in-memory database as of `now` and returns its items and stats, computed
    /// exactly the way `App::refresh` does for the stats screen.
    fn seeded(now: NaiveDateTime) -> (Vec<Item>, Vec<Completion>, Stats) {
        let db = Db::open_in_memory().unwrap();
        seed(&db, now).unwrap();
        let items = db.list_items().unwrap();
        let completions = db.list_completions().unwrap();
        let stats = stats::compute(&completions, &items, now, &Local);
        (items, completions, stats)
    }

    #[test]
    fn no_past_events_and_exactly_one_overdue_task() {
        for now in moments() {
            let (items, _, _) = seeded(now);
            let today = now.date();
            let open: Vec<&Item> = items.iter().filter(|i| !i.is_completed()).collect();

            let events: Vec<&&Item> = open.iter().filter(|i| i.kind == Kind::Event).collect();
            assert_eq!(events.len(), 5);
            for event in &events {
                assert!(
                    event.date.unwrap() >= today,
                    "{} is in the past",
                    event.title
                );
            }

            let tasks: Vec<&&Item> = open.iter().filter(|i| i.kind == Kind::Task).collect();
            assert_eq!(tasks.len(), 8);
            let overdue: Vec<_> = tasks.iter().filter(|i| is_overdue(i, now)).collect();
            assert_eq!(overdue.len(), 1, "at {now}");
            // At least one task has a due time later today (two on a Mon–Thu morning, when the
            // weekly report is due at 2 PM too).
            let timed_today = tasks
                .iter()
                .filter(|i| i.date == Some(today) && i.due_time.is_some_and(|t| t > now.time()))
                .count();
            assert!(timed_today >= 1, "at {now}");
            // The weekly report repeats Mon–Thu at 2 PM.
            assert!(tasks.iter().any(|i| {
                i.repeat == RepeatRule::Weekdays(DaySet::MON_THU)
                    && i.due_time == Some(report_time())
            }));
        }
    }

    #[test]
    fn history_spans_the_last_twelve_weeks() {
        for now in moments() {
            let (_, completions, stats) = seeded(now);
            let today = now.date();
            let days: Vec<NaiveDate> = completions
                .iter()
                .map(|c| c.completed_at.with_timezone(&Local).date_naive())
                .collect();
            let oldest_monday = week_start(today) - TimeDelta::weeks(11);
            assert_eq!(*days.iter().min().unwrap(), oldest_monday);
            assert_eq!(*days.iter().max().unwrap(), today - TimeDelta::days(1));
            // All of it lands on the bar chart, and every full past week has a bar.
            let charted: u32 = stats.per_week.iter().map(|w| w.count).sum();
            assert_eq!(charted, stats.total_all);
            assert!(
                stats.per_week[..WEEKS_SHOWN - 1]
                    .iter()
                    .all(|w| w.count > 0)
            );
        }
    }

    #[test]
    fn stats_look_populated() {
        for now in moments() {
            let (_, _, stats) = seeded(now);
            assert_eq!(stats.current_streak, STREAK_DAYS as u32, "at {now}");
            assert_eq!(stats.longest_streak, STREAK_DAYS as u32, "at {now}");
            assert_eq!(stats.overdue_count, 1);
            assert!(
                stats.total_all >= 80,
                "only {} completions",
                stats.total_all
            );

            // A few late ones, but mostly on time.
            assert!(stats.late >= 3);
            assert!(stats.on_time_rate().unwrap() > 0.9);

            // Priority mix within a couple of points of 40% Medium / 35% High / 25% Low.
            let total = f64::from(stats.total_all);
            let p = stats.by_priority;
            for (count, target) in [(p.medium, 0.40), (p.high, 0.35), (p.low, 0.25)] {
                let share = f64::from(count) / total;
                assert!((share - target).abs() < 0.03, "{share} vs {target}");
            }
        }
    }

    #[test]
    fn later_today_stays_today() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 9).unwrap();
        let at = |h, m| date.and_hms_opt(h, m, 0).unwrap();
        assert_eq!(later_today(at(13, 40)), at(15, 0));
        assert_eq!(later_today(at(21, 59)), at(23, 0));
        assert_eq!(later_today(at(22, 30)), at(23, 59));
    }

    #[test]
    fn next_christmas_is_never_in_the_past() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        assert_eq!(next_christmas(d(2026, 10, 9)), d(2026, 12, 25));
        assert_eq!(next_christmas(d(2026, 12, 25)), d(2026, 12, 25));
        assert_eq!(next_christmas(d(2026, 12, 26)), d(2027, 12, 25));
    }

    /// The stats the app itself computes (what the screen draws) match the demo's design.
    #[test]
    fn app_shows_the_same_stats() {
        let db = Db::open_in_memory().unwrap();
        let now = crate::app::local_now();
        seed(&db, now).unwrap();
        let app = crate::app::App::new(db, true, None).unwrap();
        assert_eq!(app.stats.current_streak, STREAK_DAYS as u32);
        assert_eq!(app.stats.overdue_count, 1);
        assert_eq!(app.stats.per_week.len(), WEEKS_SHOWN);
        let (_, _, expected) = seeded(app.now);
        assert_eq!(app.stats.total_all, expected.total_all);
        assert_eq!(app.stats.by_priority, expected.by_priority);
    }
}
