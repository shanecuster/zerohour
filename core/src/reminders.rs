//! Reminders: which notifications are due, and when.
//!
//! This module only *plans*. It looks at the items and the time and says "remind about X at
//! 14:00". It never reads the clock, never touches the screen and never sends anything: the
//! Android app turns the plan into alarms and notifications (and a desktop `zerohour remind`
//! command may do the same later). Keeping the planning here means every app reminds about the
//! same things at the same times, and the rules can be tested with fixed instants.
//!
//! **Times are floating local wall-clock times.** A task due at 2 PM is reminded at 2 PM on
//! whatever clock the device shows, wherever it is, just like its due time. So reminders are
//! `NaiveDateTime`s (a date and time with no time zone). Turning one into a real moment
//! (including what to do on the night the clocks change) is the app's job, done at scheduling
//! time with the device's current time zone.
//!
//! **What is reminded** (see [`plan`]):
//! - a task with a due date and time: at its due time, plus `lead_minutes` earlier if set;
//! - a *repeating* task with a due time: also at each later scheduled time while it stays
//!   open. A repeating task is stored as one open instance, and completing it creates the
//!   next. If Monday's 2 PM "Fill Out Your Timesheet" (Mon–Thu) is not done, it stays the
//!   open instance, and the plan still reminds on Tuesday at 2 PM, Wednesday at 2 PM, and so
//!   on: exactly the times the task is due again. Completing it creates the next instance as
//!   usual, and the next plan follows that one;
//! - a task due on a day *without* a time: once, at `day_time` (09:00 by default) on that day,
//!   "Due today". Only its open instance: once that day is over, an untimed task is simply
//!   overdue (see the digest below);
//! - nothing that is completed or deleted, and no events.
//!
//! Only reminders in the future are returned. A device that was off at a reminder's time
//! never gets it replayed later: the plan simply starts from "now".
//!
//! **The settings** live in the database's `settings` table under the `reminders.*` keys in
//! [`settings_keys`](crate::db::settings_keys). That table is per device and never synced, so
//! each device decides for itself whether and when to remind.

use chrono::{NaiveDateTime, NaiveTime, TimeDelta};

use crate::calendar::add_days;
use crate::clock::{TimeFormat, format_time};
use crate::db::{Db, DbError, settings_keys};
use crate::model::{Item, Kind};
use crate::repeat::next_occurrence;

/// The largest allowed lead time: one day, in minutes.
pub const MAX_LEAD_MINUTES: u32 = 24 * 60;

/// The most scheduled times looked at for one repeating task in one plan. A daily task in a
/// 48-hour window needs about three; this only stops a far-too-long window (or a rule bug)
/// from looping for ages.
const MAX_OCCURRENCES_PER_TASK: usize = 400;

/// The reminder settings, with the defaults filled in for anything not saved.
///
/// `#[derive(...)]` asks the compiler to write some standard code for us: `Debug` (printing for
/// tests), `Clone` / `Copy` (cheap copies; every field is small), `PartialEq` / `Eq` (comparing
/// with `==`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReminderSettings {
    /// The master switch. Off by default: nothing is reminded until the user turns it on.
    pub enabled: bool,
    /// Minutes before a timed task's due time for an extra, earlier reminder. 0 = none.
    /// Always between 0 and [`MAX_LEAD_MINUTES`].
    pub lead_minutes: u32,
    /// When tasks due on a day *without* a time are reminded on that day, and when the
    /// overdue digest comes. 09:00 by default.
    pub day_time: NaiveTime,
    /// A once-a-day reminder listing the overdue tasks. Off by default.
    pub overdue_digest: bool,
}

/// What can go wrong when saving reminder settings.
///
/// `thiserror` writes the boring parts of an error type for us: `#[error("...")]` is the
/// message shown to the user, and `#[from]` lets `?` convert a database error automatically.
#[derive(Debug, thiserror::Error)]
pub enum ReminderSettingsError {
    /// The lead time is more than a day.
    #[error("the reminder lead time must be between 0 and {MAX_LEAD_MINUTES} minutes")]
    LeadOutOfRange,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl Default for ReminderSettings {
    /// Off, no lead time, 09:00, no digest.
    fn default() -> ReminderSettings {
        ReminderSettings {
            enabled: false,
            lead_minutes: 0,
            day_time: default_day_time(),
            overdue_digest: false,
        }
    }
}

/// 09:00, the default `day_time`.
fn default_day_time() -> NaiveTime {
    // `from_hms_opt` returns `None` only for impossible times (like 25:00); 9:00:00 always
    // exists, so `unwrap_or_default` (midnight) is never actually used. It just avoids an
    // `unwrap()` in non-test code.
    NaiveTime::from_hms_opt(9, 0, 0).unwrap_or_default()
}

impl ReminderSettings {
    /// Reads the settings from the database. Anything missing *or not understood* (a lead time
    /// that isn't a number from 0 to 1440, a time that isn't "HH:MM") falls back to its
    /// default, so a damaged value can never stop the app or switch reminders on by accident.
    pub fn load(db: &Db) -> Result<ReminderSettings, DbError> {
        let defaults = ReminderSettings::default();
        // `as_deref` turns `Option<String>` into `Option<&str>`, so it can be compared with
        // string literals and passed to parsers without copying.
        let on = |value: Option<String>| value.as_deref() == Some("on");
        let lead_minutes = db
            .get_setting(settings_keys::REMINDERS_LEAD_MINUTES)?
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|&minutes| minutes <= MAX_LEAD_MINUTES)
            .unwrap_or(defaults.lead_minutes);
        let day_time = db
            .get_setting(settings_keys::REMINDERS_DAY_TIME)?
            .and_then(|text| NaiveTime::parse_from_str(text.trim(), "%H:%M").ok())
            .unwrap_or(defaults.day_time);
        Ok(ReminderSettings {
            enabled: on(db.get_setting(settings_keys::REMINDERS_ENABLED)?),
            lead_minutes,
            day_time,
            overdue_digest: on(db.get_setting(settings_keys::REMINDERS_OVERDUE_DIGEST)?),
        })
    }

    /// Saves the settings, after checking them: a lead time over [`MAX_LEAD_MINUTES`] is
    /// refused and nothing is written. The time is stored as "HH:MM", like due times.
    pub fn save(&self, db: &Db) -> Result<(), ReminderSettingsError> {
        if self.lead_minutes > MAX_LEAD_MINUTES {
            return Err(ReminderSettingsError::LeadOutOfRange);
        }
        let on_off = |flag: bool| if flag { "on" } else { "off" };
        db.set_setting(settings_keys::REMINDERS_ENABLED, on_off(self.enabled))?;
        db.set_setting(
            settings_keys::REMINDERS_LEAD_MINUTES,
            &self.lead_minutes.to_string(),
        )?;
        db.set_setting(
            settings_keys::REMINDERS_DAY_TIME,
            &self.day_time.format("%H:%M").to_string(),
        )?;
        db.set_setting(
            settings_keys::REMINDERS_OVERDUE_DIGEST,
            on_off(self.overdue_digest),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A time of day, for short tests.
    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn a_new_database_has_reminders_off_with_the_defaults() {
        let db = Db::open_in_memory().unwrap();
        let s = ReminderSettings::load(&db).unwrap();
        assert_eq!(s, ReminderSettings::default());
        assert!(!s.enabled);
        assert_eq!(s.lead_minutes, 0);
        assert_eq!(s.day_time, t(9, 0));
        assert!(!s.overdue_digest);
    }

    #[test]
    fn saved_settings_read_back() {
        let db = Db::open_in_memory().unwrap();
        let s = ReminderSettings {
            enabled: true,
            lead_minutes: 15,
            day_time: t(7, 30),
            overdue_digest: true,
        };
        s.save(&db).unwrap();
        assert_eq!(ReminderSettings::load(&db).unwrap(), s);
        // Stored in the plain forms other settings use.
        let get = |key| db.get_setting(key).unwrap();
        assert_eq!(get(settings_keys::REMINDERS_ENABLED).as_deref(), Some("on"));
        assert_eq!(
            get(settings_keys::REMINDERS_LEAD_MINUTES).as_deref(),
            Some("15")
        );
        assert_eq!(
            get(settings_keys::REMINDERS_DAY_TIME).as_deref(),
            Some("07:30")
        );
    }

    #[test]
    fn the_lead_time_is_checked_when_saving() {
        let db = Db::open_in_memory().unwrap();
        let mut s = ReminderSettings {
            lead_minutes: MAX_LEAD_MINUTES,
            ..ReminderSettings::default()
        };
        s.save(&db).unwrap();
        s.lead_minutes = MAX_LEAD_MINUTES + 1;
        assert!(matches!(
            s.save(&db),
            Err(ReminderSettingsError::LeadOutOfRange)
        ));
        // The refused value was not written.
        assert_eq!(
            ReminderSettings::load(&db).unwrap().lead_minutes,
            MAX_LEAD_MINUTES
        );
    }

    #[test]
    fn values_that_are_not_understood_fall_back_to_the_defaults() {
        let db = Db::open_in_memory().unwrap();
        for (key, value) in [
            (settings_keys::REMINDERS_ENABLED, "yes"),
            (settings_keys::REMINDERS_LEAD_MINUTES, "-5"),
            (settings_keys::REMINDERS_DAY_TIME, "9am"),
            (settings_keys::REMINDERS_OVERDUE_DIGEST, "1"),
        ] {
            db.set_setting(key, value).unwrap();
        }
        assert_eq!(
            ReminderSettings::load(&db).unwrap(),
            ReminderSettings::default()
        );
        // Too large is also not understood.
        db.set_setting(settings_keys::REMINDERS_LEAD_MINUTES, "1441")
            .unwrap();
        assert_eq!(ReminderSettings::load(&db).unwrap().lead_minutes, 0);
    }
}

/// Tests for the planner, with fixed local times so they never depend on the clock.
#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::model::{DaySet, Priority, RepeatRule};
    use chrono::{NaiveDate, Utc};

    /// A date.
    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    /// A time of day.
    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    /// A local date and time.
    fn at(y: i32, mo: u32, day: u32, h: u32, mi: u32) -> NaiveDateTime {
        d(y, mo, day).and_time(t(h, mi))
    }

    /// An open task with this title, due date and optional time.
    fn task(title: &str, date: Option<NaiveDate>, time: Option<NaiveTime>) -> Item {
        let mut item = Item::new(Kind::Task, title, Utc::now());
        item.priority = Priority::Medium;
        item.date = date;
        item.due_time = time;
        item
    }

    /// Reminders switched on, with the given lead time.
    fn on(lead_minutes: u32) -> ReminderSettings {
        ReminderSettings {
            enabled: true,
            lead_minutes,
            ..ReminderSettings::default()
        }
    }

    /// The plan for the 48 hours after `now`, in 12-hour times.
    fn plan48(items: &[Item], now: NaiveDateTime, settings: &ReminderSettings) -> Vec<Reminder> {
        plan(
            items,
            now,
            now + TimeDelta::hours(48),
            settings,
            TimeFormat::Hour12,
        )
    }

    /// Just the (kind, time) of each planned reminder, for compact assertions.
    fn times(plan: &[Reminder]) -> Vec<(ReminderKind, NaiveDateTime)> {
        plan.iter().map(|r| (r.kind, r.fire_at)).collect()
    }

    // 2026-10-22 is a Thursday.

    #[test]
    fn a_timed_task_is_reminded_at_its_due_time() {
        let items = [task("Call the bank", Some(d(2026, 10, 22)), Some(t(14, 0)))];
        let plan = plan48(&items, at(2026, 10, 22, 9, 0), &on(0));
        assert_eq!(times(&plan), [(ReminderKind::Due, at(2026, 10, 22, 14, 0))]);
        assert_eq!(plan[0].title, "Call the bank");
        assert_eq!(plan[0].body, "Due 2:00 PM");
        assert_eq!(plan[0].item_id, items[0].id);
    }

    #[test]
    fn a_lead_time_adds_an_earlier_reminder() {
        let items = [task("Call the bank", Some(d(2026, 10, 22)), Some(t(14, 0)))];
        let plan = plan48(&items, at(2026, 10, 22, 9, 0), &on(15));
        assert_eq!(
            times(&plan),
            [
                (ReminderKind::Lead, at(2026, 10, 22, 13, 45)),
                (ReminderKind::Due, at(2026, 10, 22, 14, 0)),
            ]
        );
        assert_eq!(plan[0].body, "Due in 15 min (2:00 PM)");
    }

    #[test]
    fn a_lead_time_can_cross_midnight() {
        let items = [task("Night shift", Some(d(2026, 10, 23)), Some(t(0, 10)))];
        let plan = plan48(&items, at(2026, 10, 22, 20, 0), &on(30));
        assert_eq!(
            times(&plan),
            [
                (ReminderKind::Lead, at(2026, 10, 22, 23, 40)),
                (ReminderKind::Due, at(2026, 10, 23, 0, 10)),
            ]
        );
    }

    #[test]
    fn a_passed_lead_time_leaves_only_the_due_reminder() {
        let items = [task("Call the bank", Some(d(2026, 10, 22)), Some(t(14, 0)))];
        let plan = plan48(&items, at(2026, 10, 22, 13, 50), &on(15));
        assert_eq!(times(&plan), [(ReminderKind::Due, at(2026, 10, 22, 14, 0))]);
    }

    #[test]
    fn a_time_already_passed_today_is_not_reminded() {
        let items = [task("Call the bank", Some(d(2026, 10, 22)), Some(t(8, 0)))];
        assert!(plan48(&items, at(2026, 10, 22, 9, 0), &on(0)).is_empty());
        // Exactly "now" counts as passed, so nothing fires twice.
        assert!(plan48(&items, at(2026, 10, 22, 8, 0), &on(0)).is_empty());
    }

    #[test]
    fn nothing_beyond_the_window() {
        let items = [task("Later", Some(d(2026, 10, 30)), Some(t(14, 0)))];
        assert!(plan48(&items, at(2026, 10, 22, 9, 0), &on(0)).is_empty());
    }

    #[test]
    fn completed_deleted_and_events_are_never_reminded() {
        let now = at(2026, 10, 22, 9, 0);
        let mut done = task("Done", Some(d(2026, 10, 22)), Some(t(14, 0)));
        done.completed_at = Some(Utc::now());
        let mut gone = task("Deleted", Some(d(2026, 10, 22)), Some(t(14, 0)));
        gone.deleted = true;
        let mut event = Item::new(Kind::Event, "Concert", Utc::now());
        event.date = Some(d(2026, 10, 22));
        event.repeat = RepeatRule::None;
        assert!(plan48(&[done, gone, event], now, &on(15)).is_empty());
    }

    #[test]
    fn with_reminders_switched_off_the_plan_is_empty() {
        let items = [task("Call the bank", Some(d(2026, 10, 22)), Some(t(14, 0)))];
        let off = ReminderSettings::default();
        assert!(plan48(&items, at(2026, 10, 22, 9, 0), &off).is_empty());
    }

    #[test]
    fn twenty_four_hour_texts_and_long_leads() {
        let items = [task("Call the bank", Some(d(2026, 10, 22)), Some(t(14, 0)))];
        let now = at(2026, 10, 22, 9, 0);
        let until = now + TimeDelta::hours(48);
        let plan = plan(&items, now, until, &on(90), TimeFormat::Hour24);
        assert_eq!(plan[0].body, "Due in 1 h 30 min (14:00)");
        assert_eq!(plan[1].body, "Due 14:00");
        let plan = plan48(&items, now, &on(60));
        assert_eq!(plan[0].body, "Due in 1 h (2:00 PM)");
    }

    /// The Mon–Thu 2 PM timesheet task, with its open instance due on `date`.
    fn timesheet(date: NaiveDate) -> Item {
        let mut item = task("Fill Out Your Timesheet", Some(date), Some(t(14, 0)));
        item.repeat = RepeatRule::Weekdays(DaySet::MON_THU);
        item
    }

    #[test]
    fn mon_thu_completed_on_thursday_next_reminds_monday_at_two() {
        // Completing Thursday's instance creates Monday's (core's complete path), so the
        // stored open instance is due Mon 2026-10-26 at 2 PM. Planned on Friday morning, a
        // 4-day window holds exactly Monday 2 PM, nothing over the weekend.
        let items = [timesheet(d(2026, 10, 26))];
        let now = at(2026, 10, 23, 8, 0);
        let plan = plan(
            &items,
            now,
            now + TimeDelta::days(4),
            &on(0),
            TimeFormat::Hour12,
        );
        assert_eq!(times(&plan), [(ReminderKind::Due, at(2026, 10, 26, 14, 0))]);
        assert_eq!(plan[0].body, "Due 2:00 PM");
    }

    #[test]
    fn a_missed_monday_still_reminds_tuesday_at_two() {
        // Monday's instance was never completed: it is still the open one on Tuesday morning.
        let items = [timesheet(d(2026, 10, 26))];
        let plan = plan48(&items, at(2026, 10, 27, 8, 0), &on(0));
        assert_eq!(
            times(&plan),
            [
                (ReminderKind::Due, at(2026, 10, 27, 14, 0)),
                (ReminderKind::Due, at(2026, 10, 28, 14, 0)),
            ]
        );
        // Every reminder is about the stored instance, so "Done" completes that one.
        assert!(plan.iter().all(|r| r.item_id == items[0].id));
    }

    #[test]
    fn a_repeating_task_reminds_on_each_scheduled_day_in_the_window_only() {
        // Thursday evening: Friday to Sunday are not Mon–Thu days.
        let items = [timesheet(d(2026, 10, 22))];
        let plan = plan48(&items, at(2026, 10, 22, 18, 0), &on(0));
        assert!(plan.is_empty());
    }

    #[test]
    fn a_long_overdue_daily_task_starts_from_now() {
        let mut item = task("Stretch", Some(d(2026, 1, 1)), Some(t(7, 0)));
        item.repeat = RepeatRule::Daily;
        let plan = plan48(&[item], at(2026, 10, 22, 9, 0), &on(0));
        assert_eq!(
            times(&plan),
            [
                (ReminderKind::Due, at(2026, 10, 23, 7, 0)),
                (ReminderKind::Due, at(2026, 10, 24, 7, 0)),
            ]
        );
    }

    #[test]
    fn a_phone_off_for_days_gets_only_future_reminders() {
        // Planned on Thursday morning; the phone was then off until the next Tuesday
        // evening. Planning again from that moment never replays Thursday, Monday or
        // Tuesday's 2 PM: only what is still ahead.
        let items = [
            timesheet(d(2026, 10, 22)),
            task("One-off", Some(d(2026, 10, 23)), Some(t(10, 0))),
        ];
        let before = plan48(&items, at(2026, 10, 22, 9, 0), &on(0));
        assert_eq!(before.len(), 2);
        let after = plan48(&items, at(2026, 10, 27, 19, 0), &on(0));
        assert_eq!(
            times(&after),
            [
                (ReminderKind::Due, at(2026, 10, 28, 14, 0)),
                (ReminderKind::Due, at(2026, 10, 29, 14, 0)),
            ]
        );
        assert!(after.iter().all(|r| r.fire_at > at(2026, 10, 27, 19, 0)));
    }

    #[test]
    fn dst_days_keep_the_wall_clock_time() {
        // The planner has no time zone: on the days the clocks change, a reminder is still
        // planned at the time shown on the clock. 02:30 on 2026-03-08 doesn't exist in most
        // of the US (spring forward), and 01:30 on 2026-11-01 happens twice (fall back); what
        // to do with those is decided by the app when it turns them into alarms.
        let spring = [task("Spring", Some(d(2026, 3, 8)), Some(t(2, 30)))];
        let fall = [task("Fall", Some(d(2026, 11, 1)), Some(t(1, 30)))];
        assert_eq!(
            times(&plan48(&spring, at(2026, 3, 7, 22, 0), &on(0))),
            [(ReminderKind::Due, at(2026, 3, 8, 2, 30))]
        );
        // Planned once, not twice.
        assert_eq!(
            times(&plan48(&fall, at(2026, 10, 31, 22, 0), &on(0))),
            [(ReminderKind::Due, at(2026, 11, 1, 1, 30))]
        );
    }

    #[test]
    fn an_untimed_task_is_reminded_at_day_time_on_its_day() {
        let items = [
            task("Today", Some(d(2026, 10, 22)), None),
            task("Tomorrow", Some(d(2026, 10, 23)), None),
            task("No date", None, None),
        ];
        let plan = plan48(&items, at(2026, 10, 22, 7, 0), &on(15));
        // The lead time is only for timed tasks.
        assert_eq!(
            times(&plan),
            [
                (ReminderKind::DayOf, at(2026, 10, 22, 9, 0)),
                (ReminderKind::DayOf, at(2026, 10, 23, 9, 0)),
            ]
        );
        assert_eq!(plan[0].title, "Today");
        assert_eq!(plan[0].body, "Due today");
    }

    #[test]
    fn untimed_reminders_follow_the_day_time_setting_and_skip_the_past() {
        let items = [task("Today", Some(d(2026, 10, 22)), None)];
        let settings = ReminderSettings {
            day_time: t(7, 30),
            ..on(0)
        };
        assert_eq!(
            times(&plan48(&items, at(2026, 10, 22, 6, 0), &settings)),
            [(ReminderKind::DayOf, at(2026, 10, 22, 7, 30))]
        );
        // Already past today's day time: nothing (and no catch-up).
        assert!(plan48(&items, at(2026, 10, 22, 10, 0), &settings).is_empty());
    }

    #[test]
    fn an_overdue_untimed_repeating_task_waits_for_its_open_instance() {
        // Only the open instance is reminded: yesterday's daily untimed task is overdue now,
        // which is the digest's job, not another "due today".
        let mut item = task("Water plants", Some(d(2026, 10, 21)), None);
        item.repeat = RepeatRule::Daily;
        assert!(plan48(&[item], at(2026, 10, 22, 7, 0), &on(0)).is_empty());
    }

    #[test]
    fn the_key_names_item_kind_and_time() {
        let items = [task("Call the bank", Some(d(2026, 10, 22)), Some(t(14, 0)))];
        let plan = plan48(&items, at(2026, 10, 22, 9, 0), &on(0));
        assert_eq!(
            plan[0].key(),
            format!("{}|Due|2026-10-22T14:00", items[0].id)
        );
    }
}

// ---- the planner -----------------------------------------------------------------------

/// Why a reminder fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReminderKind {
    /// At a timed task's due time.
    Due,
    /// `lead_minutes` before a timed task's due time.
    Lead,
    /// At `day_time` on the day an untimed task is due.
    DayOf,
}

/// One planned reminder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reminder {
    /// The task it is about.
    pub item_id: String,
    /// Why it fires.
    pub kind: ReminderKind,
    /// When, as local wall-clock time (see the module docs).
    pub fire_at: NaiveDateTime,
    /// The notification's title: the task's title.
    pub title: String,
    /// The notification's text, e.g. "Due 2:00 PM".
    pub body: String,
}

impl Reminder {
    /// A text that names this reminder and stays the same every time it is planned: the item,
    /// the kind and the time. Apps use it to schedule each reminder exactly once (planning
    /// again replaces the alarm with the same key instead of adding a second one), and to find
    /// the alarms to cancel when a reminder is no longer planned.
    pub fn key(&self) -> String {
        format!(
            "{}|{:?}|{}",
            self.item_id,
            self.kind,
            self.fire_at.format("%Y-%m-%dT%H:%M")
        )
    }
}

/// Plans every reminder that fires after `now` and no later than `until` (both local wall-clock
/// times), sorted by time.
///
/// - `items`: all items (completed, deleted and events are skipped here).
/// - `settings`: the reminder settings; with the master switch off, the plan is empty.
/// - `time_format`: 12- or 24-hour, for the texts ("Due 2:00 PM" / "Due 14:00").
///
/// Reminders at or before `now` are never returned, so nothing missed is replayed.
pub fn plan(
    items: &[Item],
    now: NaiveDateTime,
    until: NaiveDateTime,
    settings: &ReminderSettings,
    time_format: TimeFormat,
) -> Vec<Reminder> {
    let mut reminders = Vec::new();
    if !settings.enabled {
        return reminders;
    }
    // `TimeDelta` is a length of time; `try_minutes` returns `None` only for absurdly large
    // values, which `lead_minutes` (at most a day) never is.
    let lead = TimeDelta::try_minutes(i64::from(settings.lead_minutes)).unwrap_or_default();
    // Only something strictly after `now` and not after `until` goes in the plan.
    let in_window = |at: NaiveDateTime| at > now && at <= until;

    for item in items.iter().filter(|i| is_open_task(i)) {
        // A task without a date is never reminded.
        let Some(date) = item.date else {
            continue;
        };
        // `let ... else`: the code below is for tasks WITH a due time; an untimed one gets
        // its single "due today" reminder here and moves on.
        let Some(time) = item.due_time else {
            let fire_at = date.and_time(settings.day_time);
            if in_window(fire_at) {
                reminders.push(Reminder {
                    item_id: item.id.clone(),
                    kind: ReminderKind::DayOf,
                    fire_at,
                    title: item.title.clone(),
                    body: "Due today".to_string(),
                });
            }
            continue;
        };
        // The days to look at: the open instance's due date and, for a repeating task, each
        // later scheduled day. Start from the first scheduled day that could still have a
        // reminder after `now` (the lead reminder is the earliest one), so a task that is long
        // overdue doesn't walk through months of past days first.
        let earliest = (now - lead).date().max(date);
        let mut day = if item.repeat.is_repeating() {
            // `next_occurrence(rule, anchor, from)`: the first scheduled day on or after
            // `from`, counted from the task's own date (so "monthly on the 31st" stays right).
            next_occurrence(item.repeat, date, earliest)
        } else {
            date
        };
        for _ in 0..MAX_OCCURRENCES_PER_TASK {
            let due = day.and_time(time);
            // Past the window, even for its earlier reminder: no later day can be inside.
            if due - lead > until {
                break;
            }
            if in_window(due) {
                reminders.push(Reminder {
                    item_id: item.id.clone(),
                    kind: ReminderKind::Due,
                    fire_at: due,
                    title: item.title.clone(),
                    body: due_text(time, time_format),
                });
            }
            // The earlier reminder, if a lead time is set. It can fall on the previous day
            // (a task due at 00:10 with a 30-minute lead is reminded at 23:40 the day before).
            if settings.lead_minutes > 0 && in_window(due - lead) {
                reminders.push(Reminder {
                    item_id: item.id.clone(),
                    kind: ReminderKind::Lead,
                    fire_at: due - lead,
                    title: item.title.clone(),
                    body: lead_text(settings.lead_minutes, time, time_format),
                });
            }
            if !item.repeat.is_repeating() {
                break;
            }
            let next = next_occurrence(item.repeat, date, add_days(day, 1));
            // A rule that doesn't move forward would loop forever; stop instead.
            if next <= day {
                break;
            }
            day = next;
        }
    }

    // Earliest first; ties in a fixed order so the plan is always the same for the same input.
    reminders.sort_by(|a, b| (a.fire_at, a.kind, &a.item_id).cmp(&(b.fire_at, b.kind, &b.item_id)));
    reminders
}

/// True for a task that is still open and not deleted: the only things that are reminded.
fn is_open_task(item: &Item) -> bool {
    item.kind == Kind::Task && !item.deleted && !item.is_completed()
}

/// "Due 2:00 PM" / "Due 14:00".
fn due_text(time: NaiveTime, format: TimeFormat) -> String {
    format!("Due {}", format_time(time, format))
}

/// "Due in 15 min (2:00 PM)", "Due in 1 h (2:00 PM)", "Due in 1 h 30 min (2:00 PM)".
fn lead_text(lead_minutes: u32, time: NaiveTime, format: TimeFormat) -> String {
    let (hours, minutes) = (lead_minutes / 60, lead_minutes % 60);
    // `match` on a tuple of two numbers: whichever pattern fits first wins.
    let lead = match (hours, minutes) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    };
    format!("Due in {lead} ({})", format_time(time, format))
}
