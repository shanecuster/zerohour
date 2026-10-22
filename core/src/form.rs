//! The add/edit form's logic, shared by the TUI and the Android app.
//!
//! Both apps show the same form: Task or Event, then the fields for that kind (title,
//! priority, date, time, repeat, notes...). How it *looks* and how you move around in it
//! (keys in the terminal, taps on the phone) belongs to each app. *What* it contains belongs
//! here:
//!
//! - [`FormInput`]: the raw values as typed ("fri", "2pm"), plus the repeat choice.
//! - [`Field`] and [`FormInput::fields`]: which inputs appear for the current kind and repeat.
//! - [`RepeatChoice`]: the repeat options in order, including the weekday presets.
//! - [`FormInput::validate`]: turns the typed values into a checked [`FormData`], or explains
//!   what's wrong.
//! - [`FormInput::date_preview`] / [`FormInput::time_preview`]: the "→ Fri Oct 16 2026 · in
//!   7 days" line under the date and time.
//!
//! Inputs are kept as the raw text the user typed and only parsed when saving, so a half-typed
//! date like "12/2" isn't rejected while you're still typing.

use chrono::{NaiveDate, NaiveTime, Weekday};

use crate::clock::{TimeFormat, format_time, parse_time_input};
use crate::model::{DaySet, Item, Kind, Priority, RepeatRule, Unit};
use crate::repeat::{days_until, parse_date_input, scheduled_on_or_after};
use crate::theme::Slot;

/// The repeat options, in the order the form cycles through them.
///
/// "Custom" reveals the "every N" and unit fields; "weekdays", "Mon–Thu" and "pick days" reveal
/// the day checkboxes. ("Every day" is just "daily".)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatChoice {
    None,
    Daily,
    /// Monday to Friday (a preset of the weekday-set rule).
    Weekdays,
    /// Monday to Thursday (a preset of the weekday-set rule).
    MonThu,
    Weekly,
    Monthly,
    Yearly,
    /// Every N days, weeks or months.
    Custom,
    /// Any set of weekdays.
    PickDays,
}

impl RepeatChoice {
    /// Every choice, in cycling order.
    pub const ALL: [RepeatChoice; 9] = [
        RepeatChoice::None,
        RepeatChoice::Daily,
        RepeatChoice::Weekdays,
        RepeatChoice::MonThu,
        RepeatChoice::Weekly,
        RepeatChoice::Monthly,
        RepeatChoice::Yearly,
        RepeatChoice::Custom,
        RepeatChoice::PickDays,
    ];

    /// The name shown in the form.
    pub fn label(self) -> &'static str {
        match self {
            RepeatChoice::None => "none",
            RepeatChoice::Daily => "daily",
            RepeatChoice::Weekdays => "weekdays",
            RepeatChoice::MonThu => "Mon–Thu",
            RepeatChoice::Weekly => "weekly",
            RepeatChoice::Monthly => "monthly",
            RepeatChoice::Yearly => "yearly",
            RepeatChoice::Custom => "custom",
            RepeatChoice::PickDays => "pick days",
        }
    }

    /// True for the choices that repeat on a set of weekdays (and show the day checkboxes).
    pub fn shows_days(self) -> bool {
        matches!(
            self,
            RepeatChoice::Weekdays | RepeatChoice::MonThu | RepeatChoice::PickDays
        )
    }

    /// The days a preset stands for (`None` for every other choice).
    pub fn preset_days(self) -> Option<DaySet> {
        match self {
            RepeatChoice::Weekdays => Some(DaySet::WEEKDAYS),
            RepeatChoice::MonThu => Some(DaySet::MON_THU),
            _ => None,
        }
    }

    /// The choice that names a day set: a preset if it matches one, else "pick days".
    pub fn for_days(days: DaySet) -> RepeatChoice {
        match days {
            DaySet::WEEKDAYS => RepeatChoice::Weekdays,
            DaySet::MON_THU => RepeatChoice::MonThu,
            _ => RepeatChoice::PickDays,
        }
    }
}

/// One input in the form. Which ones appear depends on the kind and the repeat choice (see
/// [`FormInput::fields`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Priority,
    Date,
    /// Tasks only: optional due time (needs a date).
    Time,
    Repeat,
    /// The day checkboxes, for weekday-set repeats.
    Days,
    EveryN,
    EveryUnit,
    KeepAfterPass,
    Notes,
}

impl Field {
    /// The label shown next to the input.
    pub fn label(self, kind: Kind) -> &'static str {
        match self {
            Field::Title => "Title",
            Field::Priority => "Priority",
            Field::Date if kind == Kind::Task => "Due date",
            Field::Date => "Date",
            Field::Time => "Due time",
            Field::Repeat => "Repeat",
            Field::Days => "Days",
            Field::EveryN => "Every",
            Field::EveryUnit => "Unit",
            Field::KeepAfterPass => "Keep after",
            Field::Notes => "Notes",
        }
    }

    /// True for free-text inputs (typing goes into them).
    pub fn is_text(self) -> bool {
        matches!(
            self,
            Field::Title | Field::Date | Field::Time | Field::EveryN | Field::Notes
        )
    }
}

/// Everything typed or chosen in the form so far, before it's checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormInput {
    /// Task or Event. Chosen on the first step when adding; fixed when editing.
    pub kind: Kind,
    /// `Some(id)` when editing an existing item, `None` when adding.
    pub editing: Option<String>,
    pub title: String,
    /// Tasks only.
    pub priority: Priority,
    /// The date as typed ("fri", "12/25", "+3d"); empty = no date.
    pub date_text: String,
    /// The due time as typed ("2pm", "14:30"); empty = no time. Tasks only.
    pub time_text: String,
    /// The chosen repeat option.
    pub repeat: RepeatChoice,
    /// The "every N" number as typed, for [`RepeatChoice::Custom`].
    pub every_n: String,
    /// The unit for [`RepeatChoice::Custom`].
    pub unit: Unit,
    /// The chosen weekdays, for weekday-set repeats.
    pub days: DaySet,
    /// One-off events only: keep showing after the date passes.
    pub keep_after_pass: bool,
    pub notes: String,
}

impl Default for FormInput {
    /// A blank form for something new: a Medium task, "every 2 weeks" ready if custom is
    /// picked, and Mon–Fri ready if a weekday repeat is picked.
    fn default() -> FormInput {
        FormInput {
            kind: Kind::Task,
            editing: None,
            title: String::new(),
            priority: Priority::Medium,
            date_text: String::new(),
            time_text: String::new(),
            repeat: RepeatChoice::None,
            every_n: "2".to_string(),
            unit: Unit::Weeks,
            days: DaySet::WEEKDAYS,
            keep_after_pass: false,
            notes: String::new(),
        }
    }
}

impl FormInput {
    /// The form pre-filled from an existing item, for editing.
    ///
    /// The date is shown in ISO form ("2026-12-25"); the time in the user's 12/24-hour style.
    /// Both can be typed back in any accepted form.
    pub fn from_item(item: &Item, time_format: TimeFormat) -> FormInput {
        // `..FormInput::default()` fills every field not named here from the default.
        let mut input = FormInput {
            kind: item.kind,
            editing: Some(item.id.clone()),
            title: item.title.clone(),
            priority: item.priority,
            date_text: item.date.map(iso_date).unwrap_or_default(),
            time_text: item
                .due_time
                .map(|t| format_time(t, time_format))
                .unwrap_or_default(),
            keep_after_pass: item.keep_after_pass,
            notes: item.notes.clone().unwrap_or_default(),
            ..FormInput::default()
        };
        // Map the stored rule back onto the choice list (plus the custom or day fields).
        input.repeat = match item.repeat {
            RepeatRule::None => RepeatChoice::None,
            RepeatRule::Daily => RepeatChoice::Daily,
            RepeatRule::Weekly => RepeatChoice::Weekly,
            RepeatRule::Monthly => RepeatChoice::Monthly,
            RepeatRule::Yearly => RepeatChoice::Yearly,
            RepeatRule::EveryN { n, unit } => {
                input.every_n = n.to_string();
                input.unit = unit;
                RepeatChoice::Custom
            }
            RepeatRule::Weekdays(days) => {
                input.days = days;
                RepeatChoice::for_days(days)
            }
        };
        input
    }

    /// The inputs shown for the current kind and repeat choice, top to bottom.
    pub fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::Title];
        if self.kind == Kind::Task {
            fields.push(Field::Priority);
        }
        fields.push(Field::Date);
        if self.kind == Kind::Task {
            fields.push(Field::Time);
        }
        fields.push(Field::Repeat);
        if self.repeat.shows_days() {
            fields.push(Field::Days);
        }
        if self.repeat == RepeatChoice::Custom {
            fields.push(Field::EveryN);
            fields.push(Field::EveryUnit);
        }
        // "Keep after pass" only matters for one-off events (repeating ones roll forward).
        if self.kind == Kind::Event && self.repeat == RepeatChoice::None {
            fields.push(Field::KeepAfterPass);
        }
        fields.push(Field::Notes);
        fields
    }

    /// Picks a repeat choice. The presets fill in their days; "pick days" keeps whatever days
    /// were there.
    pub fn set_repeat(&mut self, choice: RepeatChoice) {
        self.repeat = choice;
        if let Some(days) = choice.preset_days() {
            self.days = days;
        }
    }

    /// Flips one day checkbox, then shows the matching preset name if the new set is one
    /// ("weekdays", "Mon–Thu"), or "pick days".
    pub fn toggle_day(&mut self, day: Weekday) {
        self.days = self.days.toggled(day);
        self.repeat = RepeatChoice::for_days(self.days);
    }

    /// Puts a picked date (from a calendar) into the date field, in ISO form: unambiguous, and
    /// exactly what editing shows too.
    pub fn set_date(&mut self, date: NaiveDate) {
        self.date_text = iso_date(date);
    }

    /// The repeat rule currently chosen, or an error message for a bad "every N" or an empty
    /// set of days.
    pub fn repeat_rule(&self) -> Result<RepeatRule, String> {
        Ok(match self.repeat {
            RepeatChoice::None => RepeatRule::None,
            RepeatChoice::Daily => RepeatRule::Daily,
            RepeatChoice::Weekly => RepeatRule::Weekly,
            RepeatChoice::Monthly => RepeatRule::Monthly,
            RepeatChoice::Yearly => RepeatRule::Yearly,
            RepeatChoice::Weekdays | RepeatChoice::MonThu | RepeatChoice::PickDays => {
                if self.days.is_empty() {
                    return Err("Pick at least one day.".to_string());
                }
                // All seven days is just "daily", stored as such (and readable by every build).
                if self.days == DaySet::ALL {
                    RepeatRule::Daily
                } else {
                    RepeatRule::Weekdays(self.days)
                }
            }
            RepeatChoice::Custom => {
                // `.ok()` turns a parse error into None; `.filter` keeps only 1..=999;
                // `.ok_or(...)?` turns None into our error message and returns it.
                let n: u32 = self
                    .every_n
                    .parse()
                    .ok()
                    .filter(|n| (1..=999).contains(n))
                    .ok_or("\"Every\" must be a number from 1 to 999.")?;
                RepeatRule::EveryN { n, unit: self.unit }
            }
        })
    }

    /// Checks every field and builds the result, or explains what's wrong.
    pub fn validate(&self, today: NaiveDate) -> Result<FormData, String> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err("Please enter a title.".to_string());
        }
        let typed_date = parse_date_input(&self.date_text, today)?;
        let repeat = self.repeat_rule()?;
        // Weekday repeats: an empty date becomes the first scheduled day from today, and a
        // date on an unscheduled day moves forward to the next scheduled one.
        let date = match (typed_date, repeat) {
            (Some(date), RepeatRule::Weekdays(_)) => Some(scheduled_on_or_after(repeat, date)),
            (None, RepeatRule::Weekdays(_)) => Some(scheduled_on_or_after(repeat, today)),
            (date, _) => date,
        };
        if self.kind == Kind::Event && date.is_none() {
            return Err("Events need a date to count down to.".to_string());
        }
        // Only tasks have a due time, and it needs a date to belong to.
        let due_time = if self.kind == Kind::Task {
            parse_time_input(&self.time_text)?
        } else {
            None
        };
        if due_time.is_some() && date.is_none() {
            return Err("A due time needs a due date.".to_string());
        }
        let notes = self.notes.trim();
        Ok(FormData {
            editing: self.editing.clone(),
            kind: self.kind,
            title: title.to_string(),
            notes: (!notes.is_empty()).then(|| notes.to_string()),
            priority: self.priority,
            date,
            due_time,
            repeat,
            // Only meaningful for one-off events; stored false otherwise to keep data tidy.
            keep_after_pass: self.kind == Kind::Event
                && !repeat.is_repeating()
                && self.keep_after_pass,
        })
    }

    /// The line under the date field: the understood date, or how to type one, plus its color
    /// slot. Each UI indents or wraps it as it likes.
    pub fn date_preview(&self, today: NaiveDate) -> (String, Slot) {
        // Weekday repeats: say where an unscheduled or empty date will land when saved.
        // (`rule @ pattern` matches the pattern and also names the whole value `rule`.)
        if let Ok(rule @ RepeatRule::Weekdays(_)) = self.repeat_rule() {
            match parse_date_input(&self.date_text, today) {
                Ok(Some(date)) if scheduled_on_or_after(rule, date) != date => {
                    let moved = scheduled_on_or_after(rule, date);
                    return (
                        format!(
                            "→ {} isn't a repeat day: saves as {}",
                            date.format("%a"),
                            moved.format("%a %b %d")
                        ),
                        Slot::Warn,
                    );
                }
                Ok(None) => {
                    let first = scheduled_on_or_after(rule, today);
                    return (
                        format!(
                            "→ empty: saves as {} (first repeat day)",
                            first.format("%a %b %d")
                        ),
                        Slot::Ok,
                    );
                }
                _ => {}
            }
        }
        match parse_date_input(&self.date_text, today) {
            Ok(Some(date)) => {
                let relative = match days_until(date, today) {
                    0 => "today".to_string(),
                    1 => "tomorrow".to_string(),
                    -1 => "yesterday".to_string(),
                    n if n > 0 => format!("in {n} days"),
                    n => format!("{} days ago", -n),
                };
                (
                    format!("→ {} · {relative}", crate::view::long_date(date)),
                    Slot::Ok,
                )
            }
            Ok(None) => (
                "e.g. 12/25, 2026-12-25, +3d, +2w, tomorrow, fri".to_string(),
                Slot::Muted,
            ),
            Err(_) => ("? not a date yet".to_string(), Slot::Warn),
        }
    }

    /// The line under the time field: the understood time, or how to type one, plus its color
    /// slot.
    pub fn time_preview(&self, time_format: TimeFormat) -> (String, Slot) {
        match parse_time_input(&self.time_text) {
            Ok(Some(time)) => (format!("→ {}", format_time(time, time_format)), Slot::Ok),
            Ok(None) => (
                "e.g. 2pm, 2:30 pm, 14:30 (empty = any time that day)".to_string(),
                Slot::Muted,
            ),
            Err(_) => ("? not a time yet".to_string(), Slot::Warn),
        }
    }
}

/// A date in ISO form, "2026-12-25": how the form shows a stored or picked date.
fn iso_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// The checked result of a form, ready to be written to the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormData {
    /// `Some(id)` when editing an existing item, `None` when adding a new one.
    pub editing: Option<String>,
    pub kind: Kind,
    pub title: String,
    pub notes: Option<String>,
    pub priority: Priority,
    pub date: Option<NaiveDate>,
    /// Tasks only; always `None` for events.
    pub due_time: Option<NaiveTime>,
    pub repeat: RepeatRule,
    pub keep_after_pass: bool,
}

impl FormData {
    /// Copies the form's values onto an item (new or existing). Leaves ids/timestamps alone.
    pub fn apply_to(&self, item: &mut Item) {
        item.title = self.title.clone();
        item.notes = self.notes.clone();
        item.priority = self.priority;
        item.date = self.date;
        item.due_time = self.due_time;
        item.repeat = self.repeat;
        item.keep_after_pass = self.keep_after_pass;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// Thu Oct 8 2026.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 8).unwrap()
    }

    fn task(title: &str) -> FormInput {
        FormInput {
            title: title.to_string(),
            ..FormInput::default()
        }
    }

    #[test]
    fn fields_follow_kind_and_repeat() {
        let mut input = task("x");
        assert_eq!(
            input.fields(),
            [
                Field::Title,
                Field::Priority,
                Field::Date,
                Field::Time,
                Field::Repeat,
                Field::Notes
            ]
        );
        input.set_repeat(RepeatChoice::Custom);
        assert!(input.fields().contains(&Field::EveryN));
        input.set_repeat(RepeatChoice::MonThu);
        assert!(input.fields().contains(&Field::Days));
        assert_eq!(input.days, DaySet::MON_THU, "presets fill their days");

        let event = FormInput {
            kind: Kind::Event,
            ..FormInput::default()
        };
        assert!(event.fields().contains(&Field::KeepAfterPass));
        assert!(!event.fields().contains(&Field::Priority));
        assert_eq!(Field::Date.label(Kind::Task), "Due date");
        assert_eq!(Field::Date.label(Kind::Event), "Date");
    }

    #[test]
    fn toggling_days_names_the_preset_or_pick_days() {
        let mut input = task("x");
        input.set_repeat(RepeatChoice::Weekdays);
        input.toggle_day(Weekday::Fri);
        assert_eq!(input.repeat, RepeatChoice::MonThu);
        input.toggle_day(Weekday::Sat);
        assert_eq!(input.repeat, RepeatChoice::PickDays);
    }

    #[test]
    fn validation_errors() {
        assert_eq!(
            task("  ").validate(today()),
            Err("Please enter a title.".into())
        );
        let mut input = task("x");
        input.time_text = "2pm".into();
        assert_eq!(
            input.validate(today()),
            Err("A due time needs a due date.".into())
        );
        input.set_repeat(RepeatChoice::Custom);
        input.every_n = "0".into();
        assert!(input.validate(today()).unwrap_err().contains("1 to 999"));
        input.set_repeat(RepeatChoice::PickDays);
        input.days = DaySet::EMPTY;
        assert_eq!(
            input.validate(today()),
            Err("Pick at least one day.".into())
        );
        let event = FormInput {
            kind: Kind::Event,
            title: "e".into(),
            ..FormInput::default()
        };
        assert_eq!(
            event.validate(today()),
            Err("Events need a date to count down to.".into())
        );
    }

    #[test]
    fn weekday_repeats_fill_or_move_the_date_and_seven_days_is_daily() {
        let mut input = task("Report");
        input.set_repeat(RepeatChoice::MonThu);
        // Empty → the first repeat day from today (Thu Oct 8 is one).
        assert_eq!(input.validate(today()).unwrap().date, Some(today()));
        // Saturday isn't a repeat day → moves to Monday.
        input.date_text = "2026-10-10".into();
        assert_eq!(
            input.validate(today()).unwrap().date,
            NaiveDate::from_ymd_opt(2026, 10, 12)
        );
        assert_eq!(input.date_preview(today()).1, Slot::Warn);
        input.days = DaySet::ALL;
        assert_eq!(input.validate(today()).unwrap().repeat, RepeatRule::Daily);
    }

    #[test]
    fn previews() {
        let mut input = task("x");
        assert_eq!(input.date_preview(today()).1, Slot::Muted);
        input.date_text = "fri".into();
        assert_eq!(
            input.date_preview(today()),
            ("→ Fri Oct 09 2026 · tomorrow".to_string(), Slot::Ok)
        );
        input.date_text = "zzz".into();
        assert_eq!(input.date_preview(today()).1, Slot::Warn);
        input.time_text = "14:30".into();
        assert_eq!(
            input.time_preview(TimeFormat::Hour12),
            ("→ 2:30 PM".to_string(), Slot::Ok)
        );
    }

    #[test]
    fn editing_round_trips_through_the_form() {
        let mut item = Item::new(Kind::Task, "Report", Utc::now());
        item.date = Some(today());
        item.due_time = NaiveTime::from_hms_opt(14, 0, 0);
        item.repeat = RepeatRule::EveryN {
            n: 3,
            unit: Unit::Months,
        };
        item.notes = Some("n".into());
        let input = FormInput::from_item(&item, TimeFormat::Hour24);
        assert_eq!(input.date_text, "2026-10-08");
        assert_eq!(input.time_text, "14:00");
        assert_eq!(input.repeat, RepeatChoice::Custom);
        let data = input.validate(today()).unwrap();
        let mut copy = item.clone();
        data.apply_to(&mut copy);
        assert_eq!(copy, item);
        assert_eq!(data.editing.as_deref(), Some(item.id.as_str()));
    }
}
