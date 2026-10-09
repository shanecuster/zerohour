//! Sample data for `zerohour --demo`, so the layout can be judged right away.
//!
//! Dates are relative to today, so the demo always shows a realistic mix: something due soon,
//! something overdue, an event happening today, one that rolled over to next year, and about
//! ten weeks of completion history for the stats screen.

use anyhow::Result;
use chrono::{DateTime, Datelike, Local, NaiveDate, TimeDelta, Timelike, Utc};
use zerohour_core::db::Db;
use zerohour_core::model::{Completion, Item, Kind, Priority, RepeatRule, Unit, new_id};

/// One sample task: (title, priority, due date, repeat, notes).
/// A `type` alias just gives a long type a short name.
type SampleTask = (
    &'static str,
    Priority,
    Option<NaiveDate>,
    RepeatRule,
    Option<&'static str>,
);

/// Fills the database with sample tasks, events, and completion history.
pub fn seed(db: &Db, today: NaiveDate) -> Result<()> {
    let now = Utc::now();
    // A tiny helper: today + n days (negative n = in the past).
    let day = |n: i64| today + TimeDelta::days(n);

    // ---- tasks ---------------------------------------------------------------------------
    // (title, priority, due date, repeat, notes)
    let tasks: [SampleTask; 7] = [
        (
            "Renew car registration",
            Priority::High,
            Some(day(3)),
            RepeatRule::None,
            None,
        ),
        (
            "Order printer filament",
            Priority::Medium,
            Some(day(9)),
            RepeatRule::None,
            Some("PETG, black, 2 spools"),
        ),
        (
            "File expense report",
            Priority::High,
            Some(day(-2)),
            RepeatRule::None,
            None,
        ),
        (
            "Water the plants",
            Priority::Low,
            Some(today),
            RepeatRule::Weekly,
            Some("Complete me: a new instance appears for next week"),
        ),
        (
            "Back up laptop",
            Priority::Medium,
            Some(day(12)),
            RepeatRule::Monthly,
            Some("External drive + offsite copy"),
        ),
        (
            "Rotate SSH keys",
            Priority::High,
            Some(day(45)),
            RepeatRule::EveryN {
                n: 6,
                unit: Unit::Months,
            },
            None,
        ),
        (
            "Read Rust book chapter 10",
            Priority::Low,
            None,
            RepeatRule::None,
            None,
        ),
    ];
    for (title, priority, date, repeat, notes) in tasks {
        let mut item = Item::new(Kind::Task, title, now);
        item.priority = priority;
        item.date = date;
        item.repeat = repeat;
        item.notes = notes.map(str::to_string);
        db.insert_item(&item)?;
    }

    // ---- events --------------------------------------------------------------------------
    // Christmas is stored as LAST year's date on purpose: it rolls forward on its own.
    let last_christmas = NaiveDate::from_ymd_opt(today.year() - 1, 12, 25).unwrap_or(today); // Dec 25 always exists, so the fallback never happens.
    // (title, date, repeat, keep after pass, notes)
    let events: [(&str, NaiveDate, RepeatRule, bool, Option<&str>); 6] = [
        ("Christmas", last_christmas, RepeatRule::Yearly, false, None),
        (
            "Trip to Colorado",
            day(41),
            RepeatRule::None,
            false,
            Some("Pack the hiking boots"),
        ),
        ("Launch day", today, RepeatRule::None, false, None),
        // Was yesterday and repeats yearly: shows ~364 days until next year's.
        (
            "Mom's birthday",
            day(-1),
            RepeatRule::Yearly,
            false,
            Some("Rolled over: it was yesterday"),
        ),
        ("Conference talk", day(5), RepeatRule::None, true, None),
        (
            "Pay rent",
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap_or(today),
            RepeatRule::Monthly,
            false,
            None,
        ),
    ];
    // One task with a due time, a few hours from now (on the hour), to show "in 3h".
    // `with_minute(0)` etc. return an Option; zero always exists, so the fallback never runs.
    let in_three_hours = Local::now().naive_local() + TimeDelta::hours(3);
    let soon = in_three_hours
        .with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(in_three_hours);
    let mut standup = Item::new(Kind::Task, "Team standup", now);
    standup.priority = Priority::Medium;
    standup.date = Some(soon.date());
    standup.due_time = Some(soon.time());
    standup.repeat = RepeatRule::Daily;
    db.insert_item(&standup)?;

    for (title, date, repeat, keep, notes) in events {
        let mut item = Item::new(Kind::Event, title, now);
        item.date = Some(date);
        item.repeat = repeat;
        item.keep_after_pass = keep;
        item.notes = notes.map(str::to_string);
        db.insert_item(&item)?;
    }

    // ---- completion history --------------------------------------------------------------
    // "Tasks done per day" over the last ~80 days: a daily pattern scaled by a per-week
    // "busyness" so the weekly bar chart has some shape. The last five days all have
    // completions, so the demo shows a live streak.
    const PATTERN: [u32; 10] = [1, 2, 0, 1, 3, 0, 1, 2, 1, 0];
    const WEEK_BUSYNESS: [u32; 12] = [2, 3, 1, 0, 2, 4, 3, 1, 2, 4, 2, 3];
    const TITLES: [&str; 6] = [
        "Take out recycling",
        "Patch home server",
        "Call the dentist",
        "Review pull request",
        "Clean 3D printer bed",
        "Update password manager",
    ];
    let mut n = 0usize;
    for days_ago in 1..=80i64 {
        let per_day = if days_ago <= 5 {
            1 + (days_ago % 2) as u32
        } else {
            let week = (days_ago as usize / 7) % WEEK_BUSYNESS.len();
            PATTERN[days_ago as usize % 10] * WEEK_BUSYNESS[week] / 2
        };
        for k in 0..per_day {
            let date = day(-days_ago);
            let completed_at = local_afternoon(date, k);
            let priority = Priority::ALL[n % 3];
            let title = TITLES[n % TITLES.len()];

            // The completed task itself (hidden from the list, kept as history)...
            let mut item = Item::new(Kind::Task, title, completed_at);
            item.priority = priority;
            item.date = Some(date);
            item.completed_at = Some(completed_at);
            db.insert_item(&item)?;
            // ...and its entry in the completion log, which is what stats read.
            db.insert_completion(&Completion {
                id: new_id(),
                item_id: item.id,
                completed_at,
                title_snapshot: title.to_string(),
                priority_snapshot: priority,
                was_overdue: n % 7 == 3, // roughly one in seven done late
            })?;
            n += 1;
        }
    }
    Ok(())
}

/// 2pm (plus `k` hours) local time on `date`, converted to UTC for storage.
///
/// Going through local time makes sure the completion lands on `date` in YOUR time zone.
fn local_afternoon(date: NaiveDate, k: u32) -> DateTime<Utc> {
    let naive = date.and_hms_opt(14 + k.min(8), 0, 0).unwrap_or_default();
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

    #[test]
    fn demo_data_loads_and_has_a_streak() {
        let db = Db::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        seed(&db, today).unwrap();
        let items = db.list_items().unwrap();
        assert!(items.iter().any(|i| i.kind == Kind::Event));
        let noon = today.and_hms_opt(12, 0, 0).unwrap();
        let stats =
            zerohour_core::stats::compute(&db.list_completions().unwrap(), &items, noon, &Local);
        assert!(stats.current_streak >= 5);
        assert_eq!(stats.overdue_count, 1);
    }
}
