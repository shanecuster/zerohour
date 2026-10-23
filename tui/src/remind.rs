//! `zerohour remind`: shows the reminders that are due now, each one once, then exits.
//!
//! A systemd user timer runs the command every minute (`zerohour remind --print-units` prints
//! the unit files). Each run:
//! 1. asks core's planner (`zerohour_core::reminders::plan`) which reminders fell in the last
//!    [`GRACE_MINUTES`] minutes, using this device's reminder settings;
//! 2. skips the ones this device already showed (the never-synced `reminders_sent` table);
//! 3. shows the others through a [`Notifier`] and records each one only *after* it was shown,
//!    so a failed send (no notification daemon yet, no D-Bus session) is tried again next run;
//! 4. forgets records older than [`KEEP_DAYS`] days.
//!
//! **Why a window instead of "exactly now".** The timer can run a little late, and the
//! computer may have been asleep. Looking back a few minutes catches a reminder a late run
//! would otherwise miss; stopping at [`GRACE_MINUTES`] means waking up after days away
//! doesn't pop up every reminder you missed (the phone covered those).
//!
//! Nothing here reads the clock or starts a process: the times and the notifier are passed in,
//! so the tests run at fixed moments with a fake notifier.

use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use zerohour_core::clock::{TimeFormat, format_time};
use zerohour_core::db::{Db, DbError, settings_keys};
use zerohour_core::reminders::{self, Reminder, ReminderSettings};

/// How far back a run looks for reminders it hasn't shown yet. A reminder that is older than
/// this when the computer is next awake is skipped for good.
pub const GRACE_MINUTES: i64 = 15;

/// How many days of `reminders_sent` records are kept. Only the last [`GRACE_MINUTES`] are
/// needed to avoid showing a reminder twice; the rest is there for looking back when
/// debugging (`sqlite3 ... "SELECT * FROM reminders_sent"`).
pub const KEEP_DAYS: i64 = 7;

/// How far ahead `--dry-run` lists upcoming reminders.
pub const PREVIEW_HOURS: i64 = 24;

/// Something that can show a notification: `notify-send` in the real command, a fake that
/// records calls in the tests.
///
/// A *trait* is a set of methods a type promises to have, like an interface in other
/// languages. Code written against `&mut dyn Notifier` works with any type that implements
/// it; `dyn` means "decided at run time".
pub trait Notifier {
    /// Shows one notification. `Err` carries a short reason for the log ("no notification
    /// service"); the reminder is then not recorded, so the next run tries again.
    fn send(&mut self, title: &str, body: &str) -> Result<(), String>;
}

/// What one run did, for the command's output and for the tests.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The reminders shown (and recorded) in this run.
    pub sent: Vec<Reminder>,
    /// The reminders that could not be shown, with the reason. Not recorded, so tried again.
    pub failed: Vec<(Reminder, String)>,
    /// How many due reminders were skipped because this device already showed them.
    pub already_sent: usize,
    /// How many old records were forgotten.
    pub pruned: usize,
}

/// The local wall-clock window a run looks at: after `.0`, up to and including `.1`.
///
/// - `local_now`: the local date and time now.
/// - `local_grace_ago`: the local date and time it was [`GRACE_MINUTES`] ago, computed from
///   the real moment (UTC minus the grace) by the caller.
///
/// Usually the two give the same start, `local_now - grace`. They differ only when the
/// wall clock jumped in between, and the earlier of the two is taken:
/// - **spring forward** (01:59 → 03:00): at 03:05 the clock reads 02:50 for 15 minutes ago,
///   which never existed, but the real 15 minutes ago was 01:50. Starting from 01:50 keeps a
///   02:30 reminder (which the planner places in that gap) from being skipped.
/// - **fall back** (01:59 → 01:00): at 01:05 the real 15 minutes ago reads 01:50, *after*
///   now; starting from 00:50 instead keeps the window from being empty. Reminders it covers a
///   second time were already recorded, so they are not shown twice.
pub fn window(
    local_now: NaiveDateTime,
    local_grace_ago: NaiveDateTime,
) -> (NaiveDateTime, NaiveDateTime) {
    let grace = TimeDelta::minutes(GRACE_MINUTES);
    let start = local_grace_ago.min(local_now - grace);
    (start, local_now)
}

/// The 12/24-hour choice the TUI saved (`T`), so the texts ("Due 2:00 PM") match the screen.
fn time_format(db: &Db) -> Result<TimeFormat, DbError> {
    Ok(TimeFormat::parse(
        db.get_setting(settings_keys::TIME_FORMAT)?.as_deref(),
    ))
}

/// Reads the reminder settings and the 12/24-hour setting, then plans the reminders after
/// `from` and up to `until`. Shared by [`run_once`] and [`preview`].
fn plan_between(
    db: &Db,
    from: NaiveDateTime,
    until: NaiveDateTime,
) -> Result<Vec<Reminder>, DbError> {
    let settings = ReminderSettings::load(db)?;
    let time_format = time_format(db)?;
    let items = db.list_items()?;
    // The planner returns what fires strictly after its "now" argument: passing the window's
    // start there gives exactly the reminders inside the window.
    Ok(reminders::plan(&items, from, until, &settings, time_format))
}

/// One run of `zerohour remind`: shows each due, not-yet-shown reminder once and records it.
///
/// - `local_now` / `local_grace_ago`: see [`window`].
/// - `sent_at`: the moment now in UTC, stored with each record.
///
/// Returns an error only when the database fails; a failed *send* is part of the [`Outcome`].
pub fn run_once(
    db: &Db,
    notifier: &mut dyn Notifier,
    local_now: NaiveDateTime,
    local_grace_ago: NaiveDateTime,
    sent_at: DateTime<Utc>,
) -> Result<Outcome, DbError> {
    let (start, end) = window(local_now, local_grace_ago);
    let mut outcome = Outcome::default();

    for reminder in plan_between(db, start, end)? {
        let key = reminder.key();
        if db.reminder_was_sent(&key)? {
            outcome.already_sent += 1;
            continue;
        }
        // `match` on the `Result`: record only what was really shown.
        match notifier.send(&reminder.title, &reminder.body) {
            Ok(()) => {
                db.record_reminder_sent(&key, reminder.fire_at, sent_at)?;
                outcome.sent.push(reminder);
            }
            Err(reason) => outcome.failed.push((reminder, reason)),
        }
    }

    outcome.pruned = db.prune_reminders_sent(local_now - TimeDelta::days(KEEP_DAYS))?;
    Ok(outcome)
}

/// What `zerohour remind --dry-run` shows. Nothing is sent or recorded.
#[derive(Debug)]
pub struct Preview {
    /// The reminders a run would look at right now, each with "already shown?".
    pub due_now: Vec<(Reminder, bool)>,
    /// The reminders planned for the next [`PREVIEW_HOURS`] hours.
    pub upcoming: Vec<Reminder>,
    /// The reminder settings in use (so "nothing planned" can say "reminders are off").
    pub settings: ReminderSettings,
    /// The window a run would look at now (see [`window`]).
    pub window: (NaiveDateTime, NaiveDateTime),
    /// 12- or 24-hour times, for printing.
    pub time_format: TimeFormat,
}

/// Works out the [`Preview`] for the same window a real run would use.
pub fn preview(
    db: &Db,
    local_now: NaiveDateTime,
    local_grace_ago: NaiveDateTime,
) -> Result<Preview, DbError> {
    let (start, end) = window(local_now, local_grace_ago);
    let mut due_now = Vec::new();
    for reminder in plan_between(db, start, end)? {
        let shown = db.reminder_was_sent(&reminder.key())?;
        due_now.push((reminder, shown));
    }
    let upcoming = plan_between(db, end, end + TimeDelta::hours(PREVIEW_HOURS))?;
    Ok(Preview {
        due_now,
        upcoming,
        settings: ReminderSettings::load(db)?,
        window: (start, end),
        time_format: time_format(db)?,
    })
}

impl Preview {
    /// The text `--dry-run` prints: the settings, what a run would show now, and what is
    /// planned next.
    pub fn text(&self) -> String {
        // `format_time` gives "2:00 PM" or "14:00"; this adds the weekday in front.
        let when = |at: NaiveDateTime| {
            format!(
                "{} {}",
                at.format("%a %b %d"),
                format_time(at.time(), self.time_format)
            )
        };
        let on_off = |flag: bool| if flag { "on" } else { "off" };
        let s = &self.settings;
        let mut lines = vec![format!(
            "Reminders: {} (lead time {} min, day time {}, overdue digest {})",
            on_off(s.enabled),
            s.lead_minutes,
            format_time(s.day_time, self.time_format),
            on_off(s.overdue_digest),
        )];
        if !s.enabled {
            lines.push("Switch them on in zerohour with `n` (Reminders).".to_string());
        }
        lines.push(format!(
            "Due now (after {}, up to {}):",
            when(self.window.0),
            when(self.window.1)
        ));
        if self.due_now.is_empty() {
            lines.push("  nothing".to_string());
        }
        for (reminder, shown) in &self.due_now {
            let state = if *shown {
                "already shown"
            } else {
                "would show"
            };
            lines.push(format!(
                "  {}  {}: {}  [{state}]",
                when(reminder.fire_at),
                reminder.title,
                reminder.body
            ));
        }
        lines.push(format!("Next {PREVIEW_HOURS} hours:"));
        if self.upcoming.is_empty() {
            lines.push("  nothing".to_string());
        }
        for reminder in &self.upcoming {
            lines.push(format!(
                "  {}  {}: {}",
                when(reminder.fire_at),
                reminder.title,
                reminder.body
            ));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveTime, TimeZone};
    use zerohour_core::model::{DaySet, Item, Kind, RepeatRule};

    /// A notifier that remembers what it was asked to show, and can be told to fail.
    #[derive(Default)]
    struct FakeNotifier {
        /// (title, body) of every successful send.
        shown: Vec<(String, String)>,
        /// When true, every send fails as if there were no notification daemon.
        broken: bool,
    }

    impl Notifier for FakeNotifier {
        fn send(&mut self, title: &str, body: &str) -> Result<(), String> {
            if self.broken {
                return Err("no notification service".to_string());
            }
            self.shown.push((title.to_string(), body.to_string()));
            Ok(())
        }
    }

    /// A local date and time.
    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    /// A fixed UTC moment for `sent_at` (its value doesn't matter to these tests).
    fn utc() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 26, 20, 0, 0).unwrap()
    }

    /// A database with reminders switched on (lead time 0) and the given tasks.
    fn db_with(items: &[Item]) -> Db {
        let db = Db::open_in_memory().unwrap();
        ReminderSettings {
            enabled: true,
            ..ReminderSettings::default()
        }
        .save(&db)
        .unwrap();
        for item in items {
            db.insert_item(item).unwrap();
        }
        db
    }

    /// The Mon–Thu 2 PM timesheet task, open instance due on Monday 2026-10-26.
    fn timesheet() -> Item {
        let mut item = Item::new(Kind::Task, "Fill Out Your Timesheet", utc());
        item.date = NaiveDate::from_ymd_opt(2026, 10, 26);
        item.due_time = NaiveTime::from_hms_opt(14, 0, 0);
        item.repeat = RepeatRule::Weekdays(DaySet::MON_THU);
        item
    }

    /// One run at local time `now`, on a day without a clock change.
    fn run_at(db: &Db, notifier: &mut FakeNotifier, now: NaiveDateTime) -> Outcome {
        let grace_ago = now - TimeDelta::minutes(GRACE_MINUTES);
        run_once(db, notifier, now, grace_ago, utc()).unwrap()
    }

    #[test]
    fn a_due_reminder_is_shown_once_with_the_planners_texts() {
        let db = db_with(&[timesheet()]);
        let mut fake = FakeNotifier::default();
        // 13:59: not yet.
        assert!(
            run_at(&db, &mut fake, at(2026, 10, 26, 13, 59))
                .sent
                .is_empty()
        );
        // 14:00: shown.
        let first = run_at(&db, &mut fake, at(2026, 10, 26, 14, 0));
        assert_eq!(first.sent.len(), 1);
        assert_eq!(
            fake.shown,
            [(
                "Fill Out Your Timesheet".to_string(),
                "Due 2:00 PM".to_string()
            )]
        );
        // The runs at 14:01 ... 14:14 still see it in their window, but never show it again.
        for minute in 1..=14 {
            let again = run_at(&db, &mut fake, at(2026, 10, 26, 14, minute));
            assert!(again.sent.is_empty());
            assert_eq!(again.already_sent, 1);
        }
        assert_eq!(fake.shown.len(), 1);
    }

    #[test]
    fn a_failed_send_is_not_recorded_and_is_tried_again() {
        let db = db_with(&[timesheet()]);
        let mut fake = FakeNotifier {
            broken: true,
            ..FakeNotifier::default()
        };
        let failed = run_at(&db, &mut fake, at(2026, 10, 26, 14, 0));
        assert_eq!(failed.failed.len(), 1);
        assert_eq!(failed.failed[0].1, "no notification service");
        let key = failed.failed[0].0.key();
        assert!(!db.reminder_was_sent(&key).unwrap());
        // The daemon is back a few minutes later: shown then, and recorded.
        fake.broken = false;
        let retried = run_at(&db, &mut fake, at(2026, 10, 26, 14, 4));
        assert_eq!(retried.sent.len(), 1);
        assert!(db.reminder_was_sent(&key).unwrap());
    }

    #[test]
    fn a_reminder_older_than_the_grace_window_is_skipped() {
        let db = db_with(&[timesheet()]);
        let mut fake = FakeNotifier::default();
        // A run at 14:15 looks at "after 14:00, up to 14:15", which leaves 14:00 itself out...
        assert!(
            run_at(&db, &mut fake, at(2026, 10, 26, 14, 15))
                .sent
                .is_empty()
        );
        // ...while 14:14 is the last run that shows it.
        assert_eq!(
            run_at(&db, &mut fake, at(2026, 10, 26, 14, 14)).sent.len(),
            1
        );
    }

    #[test]
    fn waking_after_days_away_shows_nothing_missed() {
        // Asleep from Monday morning until Thursday at 14:20: Monday, Tuesday, Wednesday and
        // Thursday's 2 PM are all outside the window. Nothing floods in.
        let db = db_with(&[timesheet()]);
        let mut fake = FakeNotifier::default();
        let outcome = run_at(&db, &mut fake, at(2026, 10, 29, 14, 20));
        assert!(outcome.sent.is_empty() && outcome.failed.is_empty());
        assert!(fake.shown.is_empty());
    }

    #[test]
    fn with_reminders_off_nothing_is_shown() {
        let db = db_with(&[timesheet()]);
        ReminderSettings::default().save(&db).unwrap();
        let mut fake = FakeNotifier::default();
        assert!(
            run_at(&db, &mut fake, at(2026, 10, 26, 14, 0))
                .sent
                .is_empty()
        );
        assert!(fake.shown.is_empty());
    }

    #[test]
    fn moving_a_tasks_time_gives_a_new_reminder() {
        let task = timesheet();
        let db = db_with(std::slice::from_ref(&task));
        let mut fake = FakeNotifier::default();
        assert_eq!(
            run_at(&db, &mut fake, at(2026, 10, 26, 14, 0)).sent.len(),
            1
        );
        // Moved to 14:10 after the first reminder: a new key, so a new reminder.
        let mut moved = db.get_item(&task.id).unwrap().unwrap();
        moved.due_time = NaiveTime::from_hms_opt(14, 10, 0);
        db.update_item(&mut moved, utc()).unwrap();
        let outcome = run_at(&db, &mut fake, at(2026, 10, 26, 14, 10));
        assert_eq!(outcome.sent.len(), 1);
        assert_eq!(fake.shown[1].1, "Due 2:10 PM");
    }

    #[test]
    fn a_completed_task_is_not_reminded() {
        let task = timesheet();
        let mut db = db_with(std::slice::from_ref(&task));
        // Completed at 13:58, before its reminder: the open instance is now Tuesday's.
        db.complete_task(&task.id, at(2026, 10, 26, 13, 58), utc())
            .unwrap();
        let mut fake = FakeNotifier::default();
        assert!(
            run_at(&db, &mut fake, at(2026, 10, 26, 14, 0))
                .sent
                .is_empty()
        );
        // Tuesday's instance is reminded on Tuesday.
        assert_eq!(
            run_at(&db, &mut fake, at(2026, 10, 27, 14, 0)).sent.len(),
            1
        );
    }

    #[test]
    fn the_texts_follow_the_24_hour_setting() {
        let db = db_with(&[timesheet()]);
        db.set_setting(settings_keys::TIME_FORMAT, "24h").unwrap();
        let mut fake = FakeNotifier::default();
        run_at(&db, &mut fake, at(2026, 10, 26, 14, 0));
        assert_eq!(fake.shown[0].1, "Due 14:00");
    }

    /// A task due at `h:m` on `date`, no repeat.
    fn one_off(title: &str, date: NaiveDate, h: u32, m: u32) -> Item {
        let mut item = Item::new(Kind::Task, title, utc());
        item.date = Some(date);
        item.due_time = NaiveTime::from_hms_opt(h, m, 0);
        item
    }

    #[test]
    fn a_reminder_in_the_spring_forward_gap_is_still_shown() {
        // US spring forward, 2026-03-08: the clock goes 01:59 -> 03:00, so 02:30 never shows
        // on the clock. The first run after the jump is at 03:00 local; 15 real minutes
        // earlier the clock read 01:45 (not 02:45).
        let day = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let db = db_with(&[one_off("Gap", day, 2, 30)]);
        let mut fake = FakeNotifier::default();
        let outcome = run_once(
            &db,
            &mut fake,
            at(2026, 3, 8, 3, 0),
            at(2026, 3, 8, 1, 45),
            utc(),
        )
        .unwrap();
        assert_eq!(outcome.sent.len(), 1);
        assert_eq!(fake.shown[0].0, "Gap");
    }

    #[test]
    fn a_reminder_in_the_repeated_fall_back_hour_is_shown_once() {
        // US fall back, 2026-11-01: the clock goes 01:59 -> 01:00, so 01:30 happens twice.
        let day = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let db = db_with(&[one_off("Twice?", day, 1, 30)]);
        let mut fake = FakeNotifier::default();
        // First 01:30 (daylight time): shown.
        let first = run_at(&db, &mut fake, at(2026, 11, 1, 1, 30));
        assert_eq!(first.sent.len(), 1);
        // 01:05 after the jump back: 15 real minutes ago the clock read 01:50, after "now".
        // The window must not be empty (start 00:50)...
        let (start, end) = window(at(2026, 11, 1, 1, 5), at(2026, 11, 1, 1, 50));
        assert_eq!(
            (start, end),
            (at(2026, 11, 1, 0, 50), at(2026, 11, 1, 1, 5))
        );
        // ...and the second 01:30 (standard time) is not shown again.
        let second = run_at(&db, &mut fake, at(2026, 11, 1, 1, 30));
        assert!(second.sent.is_empty());
        assert_eq!(second.already_sent, 1);
        assert_eq!(fake.shown.len(), 1);
    }

    #[test]
    fn records_older_than_a_week_are_pruned() {
        let db = db_with(&[timesheet()]);
        let mut fake = FakeNotifier::default();
        run_at(&db, &mut fake, at(2026, 10, 26, 14, 0));
        let key = format!("{}|Due|2026-10-26T14:00", timesheet_id(&db));
        assert!(db.reminder_was_sent(&key).unwrap());
        // Six days later the record is kept; eight days later it is gone.
        assert_eq!(run_at(&db, &mut fake, at(2026, 11, 1, 14, 0)).pruned, 0);
        assert_eq!(run_at(&db, &mut fake, at(2026, 11, 3, 14, 1)).pruned, 1);
        assert!(!db.reminder_was_sent(&key).unwrap());
    }

    /// The id of the (only) timesheet task's first instance in `db`.
    fn timesheet_id(db: &Db) -> String {
        db.list_items().unwrap()[0].id.clone()
    }

    #[test]
    fn the_preview_sends_and_records_nothing() {
        let db = db_with(&[timesheet()]);
        let now = at(2026, 10, 26, 14, 5);
        let preview = preview(&db, now, now - TimeDelta::minutes(GRACE_MINUTES)).unwrap();
        assert_eq!(preview.due_now.len(), 1);
        assert!(!preview.due_now[0].1, "not shown yet");
        assert!(!db.reminder_was_sent(&preview.due_now[0].0.key()).unwrap());
        // The next 24 hours hold Tuesday's 2 PM.
        assert_eq!(preview.upcoming.len(), 1);
        assert_eq!(preview.upcoming[0].fire_at, at(2026, 10, 27, 14, 0));
        assert!(preview.settings.enabled);
        let text = preview.text();
        assert!(text.starts_with("Reminders: on (lead time 0 min, day time 9:00 AM"));
        assert!(
            text.contains("Mon Oct 26 2:00 PM  Fill Out Your Timesheet: Due 2:00 PM  [would show]"),
            "{text}"
        );
        assert!(text.contains("Tue Oct 27 2:00 PM  Fill Out Your Timesheet"));
    }

    #[test]
    fn the_preview_says_how_to_switch_reminders_on() {
        let db = Db::open_in_memory().unwrap();
        let now = at(2026, 10, 26, 14, 5);
        let text = preview(&db, now, now - TimeDelta::minutes(GRACE_MINUTES))
            .unwrap()
            .text();
        assert!(text.starts_with("Reminders: off"));
        assert!(text.contains("`n`"));
    }
}
