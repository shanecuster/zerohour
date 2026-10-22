//! What the main screen shows, worked out once for every UI.
//!
//! The TUI and the Android app draw the lists differently (a ratatui table vs. Compose rows),
//! but *what* they show must be the same: which tasks and events are listed and in what order,
//! the "due in 3d" / "78 days" texts, which color slot each text uses, and the tagline under
//! the banner. All of that lives here, so the two apps can never drift apart.
//!
//! Nothing in this file draws anything or knows about colors: it returns plain text plus a
//! [`Slot`] (the *name* of a theme color). Each UI turns the slot into a color with
//! [`Theme::slot`](crate::theme::Theme::slot).
//!
//! Like the rest of `core`, every function takes "today" / "now" as a parameter and never
//! reads the clock, so the tests can pretend it's any moment they like.

use std::cmp::Ordering;

use chrono::{Datelike, NaiveDate, NaiveDateTime};

use crate::clock::{TimeFormat, format_time, format_time_short, relative_text};
use crate::icons::{Icon, IconMode};
use crate::model::{Item, Kind, Priority};
use crate::repeat::{days_until, event_is_visible, event_target, is_overdue};
use crate::theme::Slot;

/// A piece of text plus how to show it: which theme color, and whether it's bold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// The text itself, e.g. "due in 3d".
    pub text: String,
    /// The theme color slot to draw it in.
    pub slot: Slot,
    /// Drawn bold (only "today!" on an event's countdown, so it jumps out).
    pub bold: bool,
}

impl Label {
    /// A label in normal weight. `impl Into<String>` accepts both `&str` and `String`.
    fn plain(text: impl Into<String>, slot: Slot) -> Label {
        Label {
            text: text.into(),
            slot,
            bold: false,
        }
    }
}

// ---- the lists ---------------------------------------------------------------------------

/// The tasks to list: open (not completed) tasks only, sorted for the screen.
///
/// Completed tasks stay in the database as history (and count in the stats), but aren't listed.
/// Order: highest priority first; then soonest due date (no date last); on the same day, timed
/// tasks first, earliest first; then by title, ignoring upper/lower case.
pub fn open_tasks(items: &[Item]) -> Vec<Item> {
    let mut tasks: Vec<Item> = items
        .iter()
        .filter(|i| i.kind == Kind::Task && !i.is_completed())
        .cloned()
        .collect();
    // `sort_by` takes a function comparing two items. `then_with` only looks at the next rule
    // when the previous one is a tie. Rust's sort is *stable*: items that compare equal keep
    // the order they came in (the database's creation order).
    tasks.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| some_first(a.date, b.date))
            .then_with(|| some_first(a.due_time, b.due_time))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    tasks
}

/// Compares two optional values: both present → by value; a present one comes before a
/// missing one ("no due date" sorts last).
///
/// `<T: Ord>` makes this *generic*: it works for any type that can be ordered (dates, times).
fn some_first<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// An event plus its precomputed countdown, so drawing doesn't redo date math.
#[derive(Debug, Clone)]
pub struct EventRow {
    /// The event itself.
    pub item: Item,
    /// The date being counted down to (rolled forward if repeating).
    pub target: Option<NaiveDate>,
    /// Days from today to `target`: 0 = today, negative = already passed.
    pub days: Option<i64>,
}

/// The events to list, with their countdowns, sorted soonest first.
///
/// Passed one-off events are left out unless they were marked "keep after pass", or
/// `show_passed` is on (the TUI's `p` key), so you can still edit or delete them. Passed events
/// go to the bottom, the most recently passed first; events without a date go last.
pub fn event_rows(items: &[Item], today: NaiveDate, show_passed: bool) -> Vec<EventRow> {
    let mut events: Vec<EventRow> = items
        .iter()
        .filter(|i| i.kind == Kind::Event)
        .filter(|i| show_passed || event_is_visible(i, today))
        .map(|i| {
            let target = event_target(i, today);
            EventRow {
                item: i.clone(),
                target,
                days: target.map(|t| days_until(t, today)),
            }
        })
        .collect();
    // `sort_by_key` sorts by a computed key; tuples compare element by element, so the first
    // number picks the group (upcoming, passed, no date) and the second orders inside it.
    events.sort_by_key(|e| match e.days {
        Some(d) if d >= 0 => (0, d),
        Some(d) => (1, -d),
        None => (2, 0),
    });
    events
}

// ---- tagline -----------------------------------------------------------------------------

/// The soonest *upcoming* event in a sorted event list, as (title, days until it).
///
/// Passed events (shown with `p`) have negative days and are skipped. The list from
/// [`event_rows`] is already soonest first, so this is the first upcoming one.
/// The returned `&str` borrows the title from `events`, so no text is copied.
pub fn next_upcoming(events: &[EventRow]) -> Option<(&str, i64)> {
    // `find_map` walks the list and returns the first `Some(...)` the closure produces.
    events.iter().find_map(|row| match row.days {
        Some(days) if days >= 0 => Some((row.item.title.as_str(), days)),
        _ => None,
    })
}

/// The tagline under the banner: "Christmas is today!", "1 day to Christmas", "77 days to
/// Christmas", or the date (e.g. "Fri Oct 09 2026") when there's no upcoming event.
///
/// `next` is the soonest upcoming event as (title, days until it); see [`next_upcoming`].
pub fn tagline_text(next: Option<(&str, i64)>, today: NaiveDate) -> String {
    match next {
        Some((title, 0)) => format!("{title} is today!"),
        Some((title, 1)) => format!("1 day to {title}"),
        Some((title, days)) => format!("{days} days to {title}"),
        None => long_date(today),
    }
}

/// A full date as shown in the header and the tagline: "Fri Oct 09 2026".
pub fn long_date(date: NaiveDate) -> String {
    date.format("%a %b %d %Y").to_string()
}

// ---- row texts ---------------------------------------------------------------------------

/// A date for a list column: "Fri Dec 25" this year, "Oct 07 2027" in other years (so a
/// rolled-over event can't be mistaken for this year's). Empty when there's no date.
pub fn date_text(date: Option<NaiveDate>, today: NaiveDate) -> String {
    date.map(|d| {
        let format = if d.year() == today.year() {
            "%a %b %d"
        } else {
            "%b %d %Y"
        };
        d.format(format).to_string()
    })
    .unwrap_or_default()
}

/// A task's title as listed: repeating tasks get the repeat icon after the title.
pub fn task_title(task: &Item, mode: IconMode) -> String {
    let mut title = task.title.clone();
    if task.repeat.is_repeating() {
        title.push(' ');
        title.push_str(Icon::Repeat.get(mode));
    }
    title
}

/// The color slot for a task's title: `Danger` when overdue, otherwise normal text.
pub fn task_title_slot(task: &Item, now: NaiveDateTime) -> Slot {
    if is_overdue(task, now) {
        Slot::Danger
    } else {
        Slot::Fg
    }
}

/// The color slot for a priority (its icon and its name): High = danger, Medium = warn,
/// Low = muted.
pub fn priority_slot(priority: Priority) -> Slot {
    match priority {
        Priority::High => Slot::Danger,
        Priority::Medium => Slot::Warn,
        Priority::Low => Slot::Muted,
    }
}

/// An event's repeat text, "↻ yearly", or empty for one-off events.
pub fn repeat_text(item: &Item, mode: IconMode) -> String {
    if item.repeat.is_repeating() {
        format!("{} {}", Icon::Repeat.get(mode), item.repeat.label())
    } else {
        String::new()
    }
}

/// The "due in 3d" text for a task, and which color slot to draw it in.
///
/// `narrow` gives the short forms ("3d", "tmrw") used under 60 columns and on the phone.
/// Tasks with a due time get the timed forms (see [`timed_due_label`]); a task without a date
/// gets an empty label.
pub fn due_label(
    task: &Item,
    now: NaiveDateTime,
    narrow: bool,
    mode: IconMode,
    time_format: TimeFormat,
) -> Label {
    // `let ... else` : if the pattern doesn't match (no date), run the `else` block, which
    // must leave the function.
    let Some(due) = task.date else {
        return Label::plain("", Slot::Muted);
    };
    let today = now.date();
    let days = days_until(due, today);
    if let Some(time) = task.due_time {
        return timed_due_label(due.and_time(time), days, now, narrow, mode, time_format);
    }
    let overdue = Icon::Overdue.get(mode);
    // Match on (days, narrow) together; guards (`if d > 0`) refine an arm further.
    let text = match (days, narrow) {
        (0, false) => "due today".to_string(),
        (0, true) => "today".to_string(),
        (1, false) => "due tomorrow".to_string(),
        (1, true) => "tmrw".to_string(),
        (d, false) if d > 0 => format!("due in {d}d"),
        (d, true) if d > 0 => format!("{d}d"),
        (d, false) => format!("{overdue} {}d overdue", -d),
        (d, true) => format!("{overdue}{}d", -d),
    };
    let slot = match days {
        d if d < 0 => Slot::Danger,
        0 | 1 => Slot::Warn,
        _ => Slot::Muted,
    };
    Label::plain(text, slot)
}

/// The due text for a task with a due time, e.g. "today 2:00 PM · in 3h", "tomorrow 9:30 AM",
/// "in 3d · 2:00 PM", or "! overdue 2h". Narrow mode keeps just "3h" (today), "2PM"
/// (tomorrow), "3d" or "!2h".
fn timed_due_label(
    due: NaiveDateTime,
    days: i64,
    now: NaiveDateTime,
    narrow: bool,
    mode: IconMode,
    time_format: TimeFormat,
) -> Label {
    let overdue_icon = Icon::Overdue.get(mode);
    let (relative, overdue) = relative_text(due, now);
    let time = format_time(due.time(), time_format);
    let text = match (overdue, days, narrow) {
        (true, _, false) => format!("{overdue_icon} overdue {relative}"),
        (true, _, true) => format!("{overdue_icon}{relative}"),
        (false, 0, false) => format!("today {time} · in {relative}"),
        (false, 0, true) => relative,
        (false, 1, false) => format!("tomorrow {time}"),
        (false, 1, true) => format_time_short(due.time(), time_format),
        (false, d, false) => format!("in {d}d · {time}"),
        (false, d, true) => format!("{d}d"),
    };
    let slot = match (overdue, days) {
        (true, _) => Slot::Danger,
        (false, 0 | 1) => Slot::Warn,
        _ => Slot::Muted,
    };
    Label::plain(text, slot)
}

/// The "78 days" text for an event's countdown, and how to show it.
///
/// `days` is from [`EventRow::days`]: `None` means the event has no date (shouldn't happen, but
/// old or synced data might), which shows "no date" in the danger color.
pub fn countdown_label(days: Option<i64>, narrow: bool) -> Label {
    let Some(days) = days else {
        return Label::plain("no date", Slot::Danger);
    };
    let text = match (days, narrow) {
        (0, _) => "today!".to_string(),
        (1, false) => "tomorrow".to_string(),
        (d, false) if d > 0 => format!("{d} days"),
        (d, true) if d > 0 => format!("{d}d"),
        (d, false) => format!("{}d ago", -d),
        (d, true) => format!("-{}d", -d),
    };
    // `1..=7` is an inclusive range pattern: matches 1 through 7.
    let slot = match days {
        0 => Slot::Ok,
        1..=7 => Slot::Warn,
        d if d > 7 => Slot::Event,
        _ => Slot::Muted,
    };
    Label {
        text,
        slot,
        bold: days == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RepeatRule;
    use chrono::{NaiveTime, TimeDelta, Utc};

    /// "Now" for these tests: Fri Oct 9 2026, 00:30.
    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 9)
            .unwrap()
            .and_hms_opt(0, 30, 0)
            .unwrap()
    }

    /// A task due `days` from today (None = no date), at an optional (hour, minute).
    fn task(title: &str, days: Option<i64>, time: Option<(u32, u32)>, p: Priority) -> Item {
        let mut item = Item::new(Kind::Task, title, Utc::now());
        item.date = days.map(|d| now().date() + TimeDelta::days(d));
        item.due_time = time.map(|(h, m)| NaiveTime::from_hms_opt(h, m, 0).unwrap());
        item.priority = p;
        item
    }

    /// A one-off event `days` from today.
    fn event(title: &str, days: i64, keep: bool) -> Item {
        let mut item = Item::new(Kind::Event, title, Utc::now());
        item.date = Some(now().date() + TimeDelta::days(days));
        item.keep_after_pass = keep;
        item
    }

    fn titles(tasks: &[Item]) -> Vec<&str> {
        tasks.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn tasks_sort_by_priority_then_date_then_time_then_title() {
        let mut done = task("done", Some(0), None, Priority::High);
        done.completed_at = Some(Utc::now());
        let items = vec![
            task("low soon", Some(0), None, Priority::Low),
            task("high no date", None, None, Priority::High),
            task("high later", Some(5), None, Priority::High),
            task("high untimed today", Some(0), None, Priority::High),
            task("high 9am today", Some(0), Some((9, 0)), Priority::High),
            task("high 8am today", Some(0), Some((8, 0)), Priority::High),
            task("B same", Some(2), None, Priority::Medium),
            task("a same", Some(2), None, Priority::Medium),
            done,
            event("not a task", 3, false),
        ];
        assert_eq!(
            titles(&open_tasks(&items)),
            [
                "high 8am today",
                "high 9am today",
                "high untimed today",
                "high later",
                "high no date",
                "a same",
                "B same",
                "low soon",
            ]
        );
    }

    #[test]
    fn events_sort_upcoming_then_passed_and_hide_passed_one_offs() {
        let mut yearly = event("yearly, rolled over", -10, false);
        yearly.repeat = RepeatRule::Yearly;
        let items = vec![
            event("in 9", 9, false),
            event("today", 0, false),
            event("passed, hidden", -2, false),
            event("passed long ago, kept", -30, true),
            event("passed recently, kept", -1, true),
            yearly,
        ];
        let today = now().date();
        let rows = event_rows(&items, today, false);
        let order: Vec<&str> = rows.iter().map(|r| r.item.title.as_str()).collect();
        assert_eq!(
            order,
            [
                "today",
                "in 9",
                "yearly, rolled over",
                "passed recently, kept",
                "passed long ago, kept",
            ]
        );
        assert_eq!(rows[2].days, Some(355), "a yearly event rolls to next year");
        // With `p`, the hidden one shows too, among the passed ones.
        let all = event_rows(&items, today, true);
        assert!(all.iter().any(|r| r.item.title == "passed, hidden"));
        assert_eq!(all.len(), items.len());
    }

    #[test]
    fn tagline_wording_and_next_upcoming_skips_passed_events() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 9).unwrap();
        assert_eq!(
            tagline_text(Some(("Launch day", 0)), today),
            "Launch day is today!"
        );
        assert_eq!(
            tagline_text(Some(("Conference", 1)), today),
            "1 day to Conference"
        );
        assert_eq!(
            tagline_text(Some(("Christmas", 77)), today),
            "77 days to Christmas"
        );
        assert_eq!(tagline_text(None, today), "Fri Oct 09 2026");

        let rows = event_rows(
            &[event("kept", -1, true), event("soon", 4, false)],
            now().date(),
            true,
        );
        assert_eq!(next_upcoming(&rows), Some(("soon", 4)));
        assert_eq!(next_upcoming(&[]), None);
    }

    #[test]
    fn untimed_due_labels_wide_and_narrow() {
        let mode = IconMode::Unicode;
        let f = TimeFormat::Hour12;
        let cases = [
            (Some(0), "due today", "today", Slot::Warn),
            (Some(1), "due tomorrow", "tmrw", Slot::Warn),
            (Some(3), "due in 3d", "3d", Slot::Muted),
            (Some(-2), "! 2d overdue", "!2d", Slot::Danger),
        ];
        for (days, wide, narrow, slot) in cases {
            let t = task("t", days, None, Priority::Low);
            assert_eq!(
                due_label(&t, now(), false, mode, f),
                Label::plain(wide, slot)
            );
            assert_eq!(
                due_label(&t, now(), true, mode, f),
                Label::plain(narrow, slot)
            );
        }
        let no_date = task("t", None, None, Priority::Low);
        assert_eq!(due_label(&no_date, now(), false, mode, f).text, "");
    }

    #[test]
    fn timed_due_labels_wide_and_narrow() {
        let mode = IconMode::Unicode;
        let f = TimeFormat::Hour12;
        let cases = [
            (0, (12, 0), "today 12:00 PM · in 11h", "11h", Slot::Warn),
            (1, (12, 0), "tomorrow 12:00 PM", "12PM", Slot::Warn),
            (3, (12, 0), "in 3d · 12:00 PM", "3d", Slot::Muted),
            (-1, (1, 30), "! overdue 23h", "!23h", Slot::Danger),
        ];
        for (days, time, wide, narrow, slot) in cases {
            let t = task("t", Some(days), Some(time), Priority::Low);
            assert_eq!(
                due_label(&t, now(), false, mode, f),
                Label::plain(wide, slot)
            );
            assert_eq!(
                due_label(&t, now(), true, mode, f),
                Label::plain(narrow, slot)
            );
        }
    }

    #[test]
    fn countdown_labels() {
        let today = countdown_label(Some(0), false);
        assert_eq!(
            (today.text.as_str(), today.slot, today.bold),
            ("today!", Slot::Ok, true)
        );
        assert_eq!(
            countdown_label(Some(1), false),
            Label::plain("tomorrow", Slot::Warn)
        );
        assert_eq!(
            countdown_label(Some(1), true),
            Label::plain("1d", Slot::Warn)
        );
        assert_eq!(
            countdown_label(Some(8), false),
            Label::plain("8 days", Slot::Event)
        );
        assert_eq!(
            countdown_label(Some(-3), false),
            Label::plain("3d ago", Slot::Muted)
        );
        assert_eq!(
            countdown_label(Some(-3), true),
            Label::plain("-3d", Slot::Muted)
        );
        assert_eq!(
            countdown_label(None, true),
            Label::plain("no date", Slot::Danger)
        );
    }

    #[test]
    fn small_texts() {
        let today = now().date();
        assert_eq!(date_text(Some(today), today), "Fri Oct 09");
        assert_eq!(
            date_text(NaiveDate::from_ymd_opt(2027, 10, 7), today),
            "Oct 07 2027"
        );
        assert_eq!(date_text(None, today), "");

        let mut t = task("Report", Some(0), None, Priority::High);
        assert_eq!(task_title(&t, IconMode::Ascii), "Report");
        t.repeat = RepeatRule::Daily;
        assert_eq!(task_title(&t, IconMode::Ascii), "Report ~");
        assert_eq!(repeat_text(&t, IconMode::Unicode), "↻ daily");
        assert_eq!(priority_slot(Priority::High), Slot::Danger);
        assert_eq!(
            task_title_slot(&task("x", Some(-1), None, Priority::Low), now()),
            Slot::Danger
        );
        assert_eq!(
            task_title_slot(&task("x", Some(1), None, Priority::Low), now()),
            Slot::Fg
        );
    }
}
