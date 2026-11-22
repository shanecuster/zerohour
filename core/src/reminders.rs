//! Reminders: which notifications are due, and when.
//!
//! This module only *plans*. It looks at the items and the time and says "remind about X at
//! 14:00". It never reads the clock, never touches the screen and never sends anything: the
//! Android app turns the plan into alarms and notifications, and the desktop `zerohour remind`
//! command (in the `tui` crate) into desktop notifications. Keeping the planning here means every app reminds about the
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
//! - with the overdue digest switched on (off by default): once a day at `day_time`, one
//!   reminder listing the tasks that are overdue at that moment ("2 overdue tasks"), if any.
//!   Overdue means the due *day* is over: a task due at 8:00 today whose time has passed is
//!   not in today's digest (its own 8:00 reminder already came), only in tomorrow's;
//! - an *event with an end date* (Advent of Cyber, Dec 1 to Dec 24): once a day at
//!   `day_time` on every day of its range ("Day 5 of 24 · 19 days left" … "Last day"),
//!   unless its reminders were stopped for that range (`Item::reminders_stopped_for`). A
//!   repeating event's next range is reminded again by itself;
//! - nothing that is completed or deleted, and no event without an end date.
//!
//! Only reminders in the future are returned. A device that was off at a reminder's time
//! never gets it replayed later: the plan simply starts from "now".
//!
//! **The settings** live in the database's `settings` table under the `reminders.*` keys in
//! [`settings_keys`](crate::db::settings_keys). That table is per device and never synced, so
//! each device decides for itself whether and when to remind.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};

use crate::calendar::add_days;
use crate::clock::{TimeFormat, format_time};
use crate::db::{Db, DbError, settings_keys};
use crate::model::{Item, Kind};
use crate::repeat::{
    EventRange, days_until, event_range, is_overdue, next_occurrence, within_series,
};
use crate::view::open_tasks;

/// The largest allowed lead time: one day, in minutes.
pub const MAX_LEAD_MINUTES: u32 = 24 * 60;

/// The digest lists at most this many titles, then "and N more", so it stays readable.
const DIGEST_MAX_TITLES: usize = 5;

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
        assert_eq!(plan[0].item_id.as_deref(), Some(items[0].id.as_str()));
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
    fn completed_deleted_and_events_without_an_end_date_are_never_reminded() {
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

    /// The Advent of Cyber event: yearly, Dec 1 to Dec 24 (stored as 2026).
    fn advent() -> Item {
        let mut item = Item::new(Kind::Event, "Advent of Cyber", Utc::now());
        item.date = Some(d(2026, 12, 1));
        item.end_date = Some(d(2026, 12, 24));
        item.repeat = RepeatRule::Yearly;
        item
    }

    /// The plan between two moments, in 12-hour times (for spans longer than 48 hours).
    fn plan_between(items: &[Item], from: NaiveDateTime, until: NaiveDateTime) -> Vec<Reminder> {
        plan(items, from, until, &on(0), TimeFormat::Hour12)
    }

    /// Just the dates of the planned reminders.
    fn dates(plan: &[Reminder]) -> Vec<NaiveDate> {
        plan.iter().map(|r| r.fire_at.date()).collect()
    }

    #[test]
    fn a_dec_1_to_24_event_reminds_every_day_of_its_range() {
        let plan = plan_between(&[advent()], at(2026, 11, 22, 12, 0), at(2027, 1, 31, 0, 0));
        assert_eq!(plan.len(), 24);
        assert!(plan.iter().all(|r| r.kind == ReminderKind::Daily));
        // Every day at the Day time (09:00 by default), Dec 1 to Dec 24 with no gaps.
        let expected: Vec<NaiveDateTime> = (1..=24).map(|day| at(2026, 12, day, 9, 0)).collect();
        let fire_times: Vec<NaiveDateTime> = plan.iter().map(|r| r.fire_at).collect();
        assert_eq!(fire_times, expected);
        assert_eq!(plan[0].body, "Day 1 of 24 · 23 days left");
        assert_eq!(plan[4].body, "Day 5 of 24 · 19 days left");
        assert_eq!(plan[22].body, "Day 23 of 24 · 1 day left");
        assert_eq!(plan[23].body, "Last day");
        // Only the last day is the last day, and only it offers Keep/Remove.
        let last: Vec<bool> = plan.iter().map(|r| r.last_day).collect();
        assert_eq!(last.iter().filter(|&&l| l).count(), 1);
        assert!(plan[23].last_day);
        assert_eq!(plan[23].actions, ReminderActions::StopKeepRemove);
        assert!(
            plan[..23]
                .iter()
                .all(|r| r.actions == ReminderActions::StopForYear)
        );
    }

    #[test]
    fn the_range_follows_the_day_time_setting_and_skips_todays_passed_time() {
        let settings = ReminderSettings {
            day_time: t(7, 30),
            ..on(0)
        };
        let plan = plan(
            &[advent()],
            at(2026, 12, 23, 8, 0),
            at(2026, 12, 26, 0, 0),
            &settings,
            TimeFormat::Hour12,
        );
        // Dec 23 at 7:30 has passed at 8:00: only Dec 24 is left.
        assert_eq!(
            times(&plan),
            [(ReminderKind::Daily, at(2026, 12, 24, 7, 30))]
        );
    }

    #[test]
    fn stopping_silences_this_year_and_next_years_range_is_reminded_again() {
        let mut event = advent();
        event.reminders_stopped_for = Some(d(2026, 12, 1));
        let this_year = plan_between(
            &[event.clone()],
            at(2026, 12, 5, 12, 0),
            at(2026, 12, 31, 0, 0),
        );
        assert!(this_year.is_empty());
        let next_year = plan_between(&[event], at(2027, 11, 1, 0, 0), at(2027, 12, 31, 0, 0));
        assert_eq!(next_year.len(), 24);
        assert_eq!(next_year[0].fire_at, at(2027, 12, 1, 9, 0));
    }

    #[test]
    fn a_one_off_range_reminds_once_and_offers_stop_only() {
        let mut once = advent();
        once.repeat = RepeatRule::None;
        once.date = Some(d(2026, 12, 1));
        once.end_date = Some(d(2026, 12, 3));
        let plan = plan_between(&[once], at(2026, 11, 22, 12, 0), at(2028, 1, 1, 0, 0));
        assert_eq!(
            dates(&plan),
            [d(2026, 12, 1), d(2026, 12, 2), d(2026, 12, 3)]
        );
        assert!(plan.iter().all(|r| r.actions == ReminderActions::Stop));
        assert!(plan[2].last_day);
        // A one-day range: its only day is its last.
        let mut one_day = advent();
        one_day.end_date = Some(d(2026, 12, 1));
        let plan = plan_between(&[one_day], at(2026, 11, 22, 12, 0), at(2026, 12, 31, 0, 0));
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].body, "Last day");
    }

    #[test]
    fn deleted_events_and_switched_off_reminders_have_no_daily_reminders() {
        let mut gone = advent();
        gone.deleted = true;
        assert!(plan48(&[gone], at(2026, 12, 10, 12, 0), &on(0)).is_empty());
        let off = ReminderSettings::default();
        assert!(plan48(&[advent()], at(2026, 12, 10, 12, 0), &off).is_empty());
    }

    #[test]
    fn daily_reminders_keep_their_floating_time_on_clock_change_nights() {
        // 02:30 doesn't exist on Mar 8 2026 (US spring forward) and happens twice on Nov 1.
        // The planner returns the wall-clock time; the apps decide what it means.
        let settings = ReminderSettings {
            day_time: t(2, 30),
            ..on(0)
        };
        for day in [d(2026, 3, 8), d(2026, 11, 1)] {
            let mut event = advent();
            event.repeat = RepeatRule::None;
            event.date = Some(day);
            event.end_date = Some(day);
            let plan = plan(
                &[event],
                day.pred_opt().unwrap().and_time(t(12, 0)),
                day.succ_opt().unwrap().and_time(t(0, 0)),
                &settings,
                TimeFormat::Hour12,
            );
            assert_eq!(
                times(&plan),
                [(ReminderKind::Daily, day.and_time(t(2, 30)))]
            );
        }
    }

    #[test]
    fn daily_and_task_reminders_share_one_sorted_plan_with_their_buttons() {
        let items = [
            advent(),
            task("Call the bank", Some(d(2026, 12, 24)), Some(t(14, 0))),
        ];
        let plan = plan_between(&items, at(2026, 12, 23, 12, 0), at(2026, 12, 25, 0, 0));
        let summary: Vec<(ReminderKind, NaiveDateTime, ReminderActions)> = plan
            .iter()
            .map(|r| (r.kind, r.fire_at, r.actions))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    ReminderKind::Daily,
                    at(2026, 12, 24, 9, 0),
                    ReminderActions::StopKeepRemove
                ),
                (
                    ReminderKind::Due,
                    at(2026, 12, 24, 14, 0),
                    ReminderActions::Done
                ),
            ]
        );
        assert_eq!(
            plan[0].key(),
            format!("{}|Daily|2026-12-24T09:00", items[0].id)
        );
    }

    #[test]
    fn the_digest_offers_no_buttons() {
        let items = [task("Late", Some(d(2026, 12, 1)), None)];
        let settings = ReminderSettings {
            overdue_digest: true,
            ..on(0)
        };
        let plan = plan(
            &items,
            at(2026, 12, 5, 0, 0),
            at(2026, 12, 5, 12, 0),
            &settings,
            TimeFormat::Hour12,
        );
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].actions, ReminderActions::None);
    }

    #[test]
    fn still_applies_follows_tasks_ranges_and_stops() {
        let today = d(2026, 12, 10);
        let mut event = advent();
        assert!(still_applies(&event, today));
        assert!(!still_applies(&event, d(2026, 11, 30)), "before the range");
        assert!(!still_applies(&event, d(2026, 12, 25)), "after it");
        event.reminders_stopped_for = Some(d(2026, 12, 1));
        assert!(!still_applies(&event, today), "stopped");
        assert!(still_applies(&event, d(2027, 12, 1)), "next year's range");
        event.end_date = None;
        assert!(!still_applies(&event, today), "no end date");

        let mut open = task("Call", Some(today), None);
        assert!(still_applies(&open, today));
        open.completed_at = Some(Utc::now());
        assert!(!still_applies(&open, today));
    }

    #[test]
    fn reminders_stopped_only_names_the_current_range() {
        let mut event = advent();
        assert!(!reminders_stopped(&event, d(2026, 12, 10)));
        event.reminders_stopped_for = Some(d(2026, 12, 1));
        assert!(
            reminders_stopped(&event, d(2026, 11, 22)),
            "the coming range"
        );
        assert!(reminders_stopped(&event, d(2026, 12, 24)));
        // From Dec 25 the current range is 2027's: last year's stop is stale.
        assert!(!reminders_stopped(&event, d(2026, 12, 25)));
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
    fn a_repeating_task_is_reminded_only_up_to_its_until_date() {
        // Daily at 7 PM until Thu Dec 24, open instance Dec 23: Dec 23 and Dec 24 only.
        let mut room = task(
            "Advent of Cyber room",
            Some(d(2026, 12, 23)),
            Some(t(19, 0)),
        );
        room.repeat = RepeatRule::Daily;
        room.end_date = Some(d(2026, 12, 24));
        let plan = plan_between(
            std::slice::from_ref(&room),
            at(2026, 12, 23, 12, 0),
            at(2026, 12, 28, 0, 0),
        );
        assert_eq!(
            times(&plan),
            [
                (ReminderKind::Due, at(2026, 12, 23, 19, 0)),
                (ReminderKind::Due, at(2026, 12, 24, 19, 0)),
            ]
        );
        // Left open past the end: no more reminders (the overdue digest can still list it).
        room.date = Some(d(2026, 12, 22));
        assert!(plan_between(&[room], at(2026, 12, 25, 0, 0), at(2026, 12, 28, 0, 0)).is_empty());
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
        assert!(
            plan.iter()
                .all(|r| r.item_id.as_deref() == Some(items[0].id.as_str()))
        );
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

    /// Reminders on, with the overdue digest.
    fn with_digest() -> ReminderSettings {
        ReminderSettings {
            overdue_digest: true,
            ..on(0)
        }
    }

    #[test]
    fn the_digest_is_off_by_default() {
        let items = [task("Late", Some(d(2026, 10, 20)), None)];
        assert!(plan48(&items, at(2026, 10, 22, 7, 0), &on(0)).is_empty());
    }

    #[test]
    fn the_digest_lists_whats_overdue_once_a_day() {
        let items = [
            task("Pay bill", Some(d(2026, 10, 20)), None),
            task("Send report", Some(d(2026, 10, 22)), Some(t(8, 0))),
            task("Later today", Some(d(2026, 10, 22)), Some(t(18, 0))),
        ];
        let plan = plan48(&items, at(2026, 10, 22, 7, 0), &with_digest());
        let digests: Vec<&Reminder> = plan
            .iter()
            .filter(|r| r.kind == ReminderKind::Digest)
            .collect();
        // The 48 hours from Thursday 07:00 end on Saturday 07:00, before that day's 09:00:
        // so today at 9:00 and tomorrow at 9:00. Today's lists only the bill: the 8:00 report
        // has passed its time but is due today, which isn't overdue. Tomorrow's lists all
        // three, since both of Thursday's tasks are a day late by then.
        assert_eq!(digests.len(), 2);
        assert_eq!(digests[0].fire_at, at(2026, 10, 22, 9, 0));
        assert_eq!(digests[0].title, "1 overdue task");
        assert!(digests[0].body.contains("Pay bill") && !digests[0].body.contains("Send report"));
        assert_eq!(digests[1].title, "3 overdue tasks");
        assert_eq!(digests[0].item_id, None);
        assert!(digests[0].key().starts_with("digest|Digest|"));
    }

    #[test]
    fn no_digest_when_nothing_is_overdue_and_never_for_completed_or_deleted_tasks() {
        let mut done = task("Done", Some(d(2026, 10, 20)), None);
        done.completed_at = Some(Utc::now());
        let mut gone = task("Deleted", Some(d(2026, 10, 20)), None);
        gone.deleted = true;
        let upcoming = task("Upcoming", Some(d(2026, 10, 30)), None);
        assert!(
            plan48(
                &[done, gone, upcoming],
                at(2026, 10, 22, 7, 0),
                &with_digest()
            )
            .is_empty()
        );
    }

    #[test]
    fn a_long_digest_is_shortened() {
        let items: Vec<Item> = (1..=7)
            .map(|n| task(&format!("Task {n}"), Some(d(2026, 10, 20)), None))
            .collect();
        let now = at(2026, 10, 22, 7, 0);
        let plan = plan(
            &items,
            now,
            at(2026, 10, 22, 12, 0),
            &with_digest(),
            TimeFormat::Hour12,
        );
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].title, "7 overdue tasks");
        assert!(plan[0].body.ends_with(" and 2 more"), "{}", plan[0].body);
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
    /// The once-a-day list of overdue tasks (not about one task).
    Digest,
    /// At `day_time` on each day of an event's range (an event with an end date).
    Daily,
}

/// Which buttons a reminder's notification offers.
///
/// Worked out here (see [`Reminder::actions`]) so the Android app never has to look at the
/// item to decide, and can't disagree with the plan. The desktop shows no buttons
/// (`notify-send` has none), so it ignores this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderActions {
    /// No buttons: the overdue digest, which is about several tasks.
    None,
    /// "Done": every task reminder.
    Done,
    /// "Stop reminders": a one-off event's daily reminder.
    Stop,
    /// "Stop for this year": a repeating event's daily reminder before its last day.
    StopForYear,
    /// "Stop for this year", "Keep" and "Remove": a repeating event's daily reminder on the
    /// last day of its range, when the apps ask whether to keep the event for next time.
    StopKeepRemove,
}

/// One planned reminder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reminder {
    /// The task or event it is about, or `None` for the digest (which is about several).
    pub item_id: Option<String>,
    /// Why it fires.
    pub kind: ReminderKind,
    /// When, as local wall-clock time (see the module docs).
    pub fire_at: NaiveDateTime,
    /// The notification's title: the task's (or event's) title.
    pub title: String,
    /// The notification's text, e.g. "Due 2:00 PM".
    pub body: String,
    /// True only for an event's daily reminder on the last day of its range. Worked out here,
    /// not stored, so the apps can never disagree about which day is last.
    pub last_day: bool,
    /// The buttons its notification offers (see [`ReminderActions`]).
    pub actions: ReminderActions,
}

impl Reminder {
    /// A text that names this reminder and stays the same every time it is planned: the item,
    /// the kind and the time. Apps use it to schedule each reminder exactly once (planning
    /// again replaces the alarm with the same key instead of adding a second one), and to find
    /// the alarms to cancel when a reminder is no longer planned.
    pub fn key(&self) -> String {
        format!(
            "{}|{:?}|{}",
            // `as_deref` borrows the text inside the `Option`; the digest has none.
            self.item_id.as_deref().unwrap_or("digest"),
            self.kind,
            self.fire_at.format("%Y-%m-%dT%H:%M")
        )
    }
}

/// Plans every reminder that fires after `now` and no later than `until` (both local wall-clock
/// times), sorted by time.
///
/// - `items`: all items (completed and deleted ones are skipped here, and so are events
///   without an end date).
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
                    item_id: Some(item.id.clone()),
                    kind: ReminderKind::DayOf,
                    fire_at,
                    title: item.title.clone(),
                    body: "Due today".to_string(),
                    last_day: false,
                    actions: ReminderActions::Done,
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
            // A later scheduled day past the series' end ("daily until Dec 24") is never
            // due. The open instance's own day always counts: it exists, even if the end
            // date was moved before it.
            if day != date && !within_series(item, day) {
                break;
            }
            let due = day.and_time(time);
            // Past the window, even for its earlier reminder: no later day can be inside.
            if due - lead > until {
                break;
            }
            if in_window(due) {
                reminders.push(Reminder {
                    item_id: Some(item.id.clone()),
                    kind: ReminderKind::Due,
                    fire_at: due,
                    title: item.title.clone(),
                    body: due_text(time, time_format),
                    last_day: false,
                    actions: ReminderActions::Done,
                });
            }
            // The earlier reminder, if a lead time is set. It can fall on the previous day
            // (a task due at 00:10 with a 30-minute lead is reminded at 23:40 the day before).
            if settings.lead_minutes > 0 && in_window(due - lead) {
                reminders.push(Reminder {
                    item_id: Some(item.id.clone()),
                    kind: ReminderKind::Lead,
                    fire_at: due - lead,
                    title: item.title.clone(),
                    body: lead_text(settings.lead_minutes, time, time_format),
                    last_day: false,
                    actions: ReminderActions::Done,
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

    // Daily event reminders: their own loop, one day at a time (see `daily_reminders`).
    for item in items {
        // `in_window` only borrows `now` and `until` (shared borrows can be copied), so the
        // closure itself is `Copy`: each call gets its own copy and it stays usable below.
        reminders.extend(daily_reminders(item, now, until, settings, in_window));
    }

    if settings.overdue_digest {
        reminders.extend(digests(items, now, until, settings, in_window));
    }

    // Earliest first; ties in a fixed order so the plan is always the same for the same input.
    reminders.sort_by(|a, b| (a.fire_at, a.kind, &a.item_id).cmp(&(b.fire_at, b.kind, &b.item_id)));
    reminders
}

/// The overdue digests in the window: one per day at `day_time`, listing the tasks overdue at
/// that moment (in the main screen's order), and none on a day when nothing is overdue.
///
/// `in_window` is the planner's "after now, not after until" test, passed in so both use the
/// same rule. `impl Fn(NaiveDateTime) -> bool` means "any function (or closure) that takes a
/// date-time and answers yes or no".
fn digests(
    items: &[Item],
    now: NaiveDateTime,
    until: NaiveDateTime,
    settings: &ReminderSettings,
    in_window: impl Fn(NaiveDateTime) -> bool,
) -> Vec<Reminder> {
    // Sorted the way the Tasks box lists them (core's view), so the digest reads the same.
    let open = open_tasks(items);
    let mut digests = Vec::new();
    let mut day = now.date();
    while day <= until.date() {
        let fire_at = day.and_time(settings.day_time);
        if in_window(fire_at) {
            // `open_tasks` leaves out completed tasks but not deleted ones, so those are
            // skipped here too.
            let overdue: Vec<&str> = open
                .iter()
                .filter(|item| !item.deleted && is_overdue(item, fire_at))
                .map(|item| item.title.as_str())
                .collect();
            if !overdue.is_empty() {
                digests.push(Reminder {
                    item_id: None,
                    kind: ReminderKind::Digest,
                    fire_at,
                    title: digest_title(overdue.len()),
                    body: digest_body(&overdue),
                    last_day: false,
                    actions: ReminderActions::None,
                });
            }
        }
        day = add_days(day, 1);
    }
    digests
}

/// "1 overdue task" / "3 overdue tasks".
fn digest_title(count: usize) -> String {
    if count == 1 {
        "1 overdue task".to_string()
    } else {
        format!("{count} overdue tasks")
    }
}

/// The titles, comma-separated: at most [`DIGEST_MAX_TITLES`], then "and N more".
fn digest_body(titles: &[&str]) -> String {
    let shown = titles.len().min(DIGEST_MAX_TITLES);
    let mut body = titles[..shown].join(", ");
    if titles.len() > shown {
        body.push_str(&format!(" and {} more", titles.len() - shown));
    }
    body
}

/// The range of `event` that `day` is a reminded day of: inside the range, and reminders
/// not stopped for it. `None` for anything else (a task, a deleted event, an event without an
/// end date, a day outside the range, or a stopped range).
///
/// A stop marker counts only for the range whose first day it names, so last year's stop
/// never silences this year's range.
pub fn reminded_range(event: &Item, day: NaiveDate) -> Option<EventRange> {
    if event.deleted {
        return None;
    }
    // `event_range` is `None` for a task or an event without an end date. `.filter(...)`
    // keeps the range only if the test passes.
    let range = event_range(event, day).filter(|range| range.contains(day))?;
    if event.reminders_stopped_for == Some(range.start) {
        return None;
    }
    Some(range)
}

/// One event's daily reminders in the plan window: at `day_time` on each day that
/// [`reminded_range`] says is reminded. A 48-hour plan looks at three days.
fn daily_reminders(
    event: &Item,
    now: NaiveDateTime,
    until: NaiveDateTime,
    settings: &ReminderSettings,
    in_window: impl Fn(NaiveDateTime) -> bool,
) -> Vec<Reminder> {
    let mut out = Vec::new();
    // Skip the day loop entirely for everything that can never have a daily reminder.
    if event.kind != Kind::Event || event.end_date.is_none() {
        return out;
    }
    let mut day = now.date();
    while day <= until.date() {
        let fire_at = day.and_time(settings.day_time);
        // A *let chain*: `cond && let Some(x) = value` runs the block only if the condition
        // holds AND the value is `Some`, binding `x` for the block. Here: is this moment in the
        // plan window, and is this day a reminded day (and of which range)?
        if in_window(fire_at)
            && let Some(range) = reminded_range(event, day)
        {
            let last_day = day == range.end;
            // `match` on a pair: the first pattern that fits wins.
            let actions = match (event.repeat.is_repeating(), last_day) {
                (false, _) => ReminderActions::Stop,
                (true, false) => ReminderActions::StopForYear,
                (true, true) => ReminderActions::StopKeepRemove,
            };
            out.push(Reminder {
                item_id: Some(event.id.clone()),
                kind: ReminderKind::Daily,
                fire_at,
                title: event.title.clone(),
                body: daily_text(range, day),
                last_day,
                actions,
            });
        }
        day = add_days(day, 1);
    }
    out
}

/// "Day 5 of 24 · 19 days left", "Day 23 of 24 · 1 day left", "Last day".
fn daily_text(range: EventRange, day: NaiveDate) -> String {
    let left = days_until(range.end, day);
    match left {
        0 => "Last day".to_string(),
        1 => format!(
            "Day {} of {} · 1 day left",
            range.day_number(day),
            range.len_days()
        ),
        n => format!(
            "Day {} of {} · {n} days left",
            range.day_number(day),
            range.len_days()
        ),
    }
}

/// True if `event`'s daily reminders are stopped for its current range (the running one, or
/// the next one if none is running): what the apps show as "stopped", and use to offer
/// "Resume" instead of "Stop". A marker naming an earlier range (last year's) doesn't count.
pub fn reminders_stopped(event: &Item, today: NaiveDate) -> bool {
    // `is_some_and`: true only for `Some(range)` where the closure says yes.
    event_range(event, today).is_some_and(|range| event.reminders_stopped_for == Some(range.start))
}

/// Should an alarm set earlier for this item still be shown on `today` (local date)?
///
/// Apps that schedule alarms ahead (Android) ask this when one goes off, as a second guard
/// after cancelling alarms that are no longer planned: a sync may have completed the task,
/// deleted the event, changed its range or stopped its reminders after the alarm was set.
///
/// - A task: still open and not deleted.
/// - An event: today is a reminded day of its range (see [`reminded_range`]).
pub fn still_applies(item: &Item, today: NaiveDate) -> bool {
    match item.kind {
        Kind::Task => is_open_task(item),
        Kind::Event => reminded_range(item, today).is_some(),
    }
}

/// True for a task that is still open and not deleted: the only tasks that are reminded.
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
