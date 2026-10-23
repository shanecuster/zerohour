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
//! **The settings** live in the database's `settings` table under the `reminders.*` keys in
//! [`settings_keys`](crate::db::settings_keys). That table is per device and never synced, so
//! each device decides for itself whether and when to remind.

use chrono::NaiveTime;

use crate::db::{Db, DbError, settings_keys};

/// The largest allowed lead time: one day, in minutes.
pub const MAX_LEAD_MINUTES: u32 = 24 * 60;

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
