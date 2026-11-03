//! The add/edit form's logic, shared by the TUI and the Android app.
//!
//! Both apps show the same form: Task or Event, then the fields for that kind (title,
//! priority, date, time, repeat, notes...). How it *looks* and how you move around in it
//! (keys in the terminal, taps on the phone) belongs to each app. *What* it contains belongs
//! here:
//!
//! - [`FormInput`]: the raw values as typed ("fri", "2pm"), plus the repeat choice.
//! - [`Field`] and [`FormInput::fields`]: which inputs appear for the current kind and repeat.
//! - [`RepeatChoice`]: the repeat options in order, including the weekday presets and the
//!   "nth weekday" rule (the 4th Thursday of November).
//! - [`FormInput::step_choice`] / [`FormInput::field_value`]: step any choice field and read
//!   any field's value as text, so an app can handle every field the same way.
//! - [`FormInput::validate`]: turns the typed values into a checked [`FormData`], or explains
//!   what's wrong.
//! - [`FormInput::preview`]: the line under a field: "→ Fri Oct 16 2026 · in 7 days" under
//!   the date, the understood time, and the next dates of an Nth-weekday rule.
//!
//! Inputs are kept as the raw text the user typed and only parsed when saving, so a half-typed
//! date like "12/2" isn't rejected while you're still typing.

use chrono::{Datelike, Month, NaiveDate, NaiveTime, TimeDelta, Weekday};

use crate::clock::{TimeFormat, format_time, parse_time_input};
use crate::model::{DaySet, Item, Kind, Ordinal, Priority, RepeatRule, Unit};
use crate::model::{month_label, weekday_label};
use crate::repeat::{days_until, next_occurrence, parse_date_input, scheduled_on_or_after};
use crate::theme::Slot;

/// The repeat options, in the order the form cycles through them.
///
/// "Custom" reveals the "every N" and unit fields; "weekdays", "Mon–Thu" and "pick days" reveal
/// the day checkboxes; "nth weekday" reveals the ordinal, weekday and month. ("Every day" is
/// just "daily".)
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
    /// The Nth weekday of a month, or of every month: "4th Thu of Nov", "every 1st Fri".
    NthWeekday,
    /// Every N days, weeks or months.
    Custom,
    /// Any set of weekdays.
    PickDays,
}

impl RepeatChoice {
    /// Every choice, in cycling order. "Pick days" stays last: stepping back from "none"
    /// lands on it (the Android app's tests rely on that).
    pub const ALL: [RepeatChoice; 10] = [
        RepeatChoice::None,
        RepeatChoice::Daily,
        RepeatChoice::Weekdays,
        RepeatChoice::MonThu,
        RepeatChoice::Weekly,
        RepeatChoice::Monthly,
        RepeatChoice::Yearly,
        RepeatChoice::NthWeekday,
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
            RepeatChoice::NthWeekday => "nth weekday",
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
    /// Nth-weekday repeats: which one (1st to 4th, or last).
    Ordinal,
    /// Nth-weekday repeats: which day of the week.
    Weekday,
    /// Nth-weekday repeats: which month, or every month.
    Month,
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
            // Reads top to bottom as "On the 4th / Weekday Thu / Month Nov".
            Field::Ordinal => "On the",
            Field::Weekday => "Weekday",
            Field::Month => "Month",
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

    /// True for the fields that pick one value from a list, stepped with
    /// [`FormInput::step_choice`] (shown as `‹ value ›`). Not the day checkboxes or the
    /// "keep after" checkbox.
    pub fn is_choice(self) -> bool {
        matches!(
            self,
            Field::Priority
                | Field::Repeat
                | Field::EveryUnit
                | Field::Ordinal
                | Field::Weekday
                | Field::Month
        )
    }
}

/// The month choices in cycling order: every month first, then January to December.
fn month_choices() -> [Option<Month>; 13] {
    // `std::array::from_fn` builds an array by calling the closure with each index 0..13.
    // Index 0 is "every month"; 1..=12 go through `Month::try_from`, which can't fail for
    // those numbers (`.ok()` would give "every month" if it somehow did).
    std::array::from_fn(|i| {
        if i == 0 {
            None
        } else {
            Month::try_from(i as u8).ok()
        }
    })
}

/// The weekday choices in cycling order, Monday first.
const WEEKDAY_CHOICES: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

/// The item after `current` in `list` (`forward`), or before it, wrapping at both ends. If
/// `current` isn't in the list, starts from the first item.
///
/// `<T: PartialEq + Copy>` makes this *generic*: it works for any type that can be compared
/// and copied (priorities, units, weekdays...), written once instead of once per type.
fn step_through<T: PartialEq + Copy>(list: &[T], current: T, forward: bool) -> T {
    let len = list.len();
    let i = list.iter().position(|&x| x == current).unwrap_or(0);
    let next = if forward {
        (i + 1) % len
    } else {
        (i + len - 1) % len
    };
    list[next]
}

/// A date that stays in one piece when a UI wraps text: "Thu Nov 26 2026" with non-breaking
/// spaces (U+00A0) instead of normal ones.
///
/// Why: both apps wrap preview lines at spaces, and a narrow screen would otherwise split a
/// date as "Thu Nov" / "26 2026". A non-breaking space looks the same but is never a wrapping
/// point, so a list of dates wraps between dates only.
fn unbroken_date(date: NaiveDate) -> String {
    crate::view::long_date(date).replace(' ', "\u{a0}")
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
    /// Nth-weekday repeats: which one (1st to 4th, or last).
    pub nth: Ordinal,
    /// Nth-weekday repeats: which day of the week.
    pub weekday: Weekday,
    /// Nth-weekday repeats: `Some(month)` for once a year, `None` for every month.
    pub month: Option<Month>,
    /// One-off events only: keep showing after the date passes.
    pub keep_after_pass: bool,
    pub notes: String,
}

impl Default for FormInput {
    /// A blank form for something new: a Medium task, "every 2 weeks" ready if custom is
    /// picked, Mon–Fri ready if a weekday repeat is picked, and "every 1st Mon" if the Nth
    /// weekday is picked without a full date to fill it in from.
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
            nth: Ordinal::First,
            weekday: Weekday::Mon,
            month: None,
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
            RepeatRule::NthWeekday {
                nth,
                weekday,
                month,
            } => {
                input.nth = nth;
                input.weekday = weekday;
                input.month = month;
                RepeatChoice::NthWeekday
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
        if self.repeat == RepeatChoice::NthWeekday {
            fields.push(Field::Ordinal);
            fields.push(Field::Weekday);
            fields.push(Field::Month);
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
    /// were there. "Nth weekday" fills in its three fields from the date, if the date field
    /// holds a full date (see [`FormInput::fill_nth_from_date`]).
    pub fn set_repeat(&mut self, choice: RepeatChoice) {
        self.repeat = choice;
        if let Some(days) = choice.preset_days() {
            self.days = days;
        }
        if choice == RepeatChoice::NthWeekday {
            self.fill_nth_from_date();
        }
    }

    /// Sets the Nth-weekday fields to describe the date in the date field, yearly: 2026-11-26
    /// gives "4th Thu of Nov". Days 29 to 31 give "last" (there's no 5th).
    ///
    /// Only a *full* date is used (2026-11-26 or 11/26/2026, which is also what editing and
    /// the calendar put there). A short form like "11/26" or "fri" depends on today's date,
    /// which the form doesn't have while you're choosing; then the fields keep their values.
    fn fill_nth_from_date(&mut self) {
        let text = self.date_text.trim();
        // `find_map` tries each format and stops at the first that parses.
        let Some(date) = ["%Y-%m-%d", "%m/%d/%Y"]
            .iter()
            .find_map(|format| NaiveDate::parse_from_str(text, format).ok())
        else {
            return;
        };
        self.nth = match date.day() {
            1..=7 => Ordinal::First,
            8..=14 => Ordinal::Second,
            15..=21 => Ordinal::Third,
            22..=28 => Ordinal::Fourth,
            _ => Ordinal::Last,
        };
        self.weekday = date.weekday();
        self.month = Month::try_from(date.month() as u8).ok();
    }

    /// Steps a choice field to its next value (`forward`) or the previous one, wrapping at
    /// both ends: Priority, Repeat, Unit, and the Nth weekday's ordinal, weekday and month.
    /// The "keep after" checkbox is flipped. Text fields and the day checkboxes are left alone
    /// (each app moves its own cursor across the days).
    ///
    /// One function for every choice field means an app needs no list of its own: it can
    /// call this for any field where [`Field::is_choice`] is true.
    pub fn step_choice(&mut self, field: Field, forward: bool) {
        match field {
            Field::Priority => {
                self.priority = step_through(&Priority::ALL, self.priority, forward);
            }
            Field::Repeat => {
                // `set_repeat` also fills in a preset's days, or the Nth weekday's fields.
                self.set_repeat(step_through(&RepeatChoice::ALL, self.repeat, forward));
            }
            Field::EveryUnit => self.unit = step_through(&Unit::ALL, self.unit, forward),
            Field::Ordinal => self.nth = step_through(&Ordinal::ALL, self.nth, forward),
            Field::Weekday => {
                self.weekday = step_through(&WEEKDAY_CHOICES, self.weekday, forward);
            }
            Field::Month => self.month = step_through(&month_choices(), self.month, forward),
            Field::KeepAfterPass => self.keep_after_pass = !self.keep_after_pass,
            Field::Title
            | Field::Date
            | Field::Time
            | Field::Days
            | Field::EveryN
            | Field::Notes => {}
        }
    }

    /// A field's current value as plain text: what was typed for text fields, the choice's
    /// name for choice fields ("Medium", "nth weekday", "4th", "Thu", "every month"), the
    /// day set's label for the day checkboxes ("Mon–Thu"), and "yes"/"no" for the "keep
    /// after" checkbox. Apps draw the checkboxes themselves from `days` and
    /// `keep_after_pass`; this is the plain-text summary.
    pub fn field_value(&self, field: Field) -> String {
        match field {
            Field::Title => self.title.clone(),
            Field::Date => self.date_text.clone(),
            Field::Time => self.time_text.clone(),
            Field::EveryN => self.every_n.clone(),
            Field::Notes => self.notes.clone(),
            Field::Priority => self.priority.label().to_string(),
            Field::Repeat => self.repeat.label().to_string(),
            Field::EveryUnit => self.unit.as_str().to_string(),
            Field::Ordinal => self.nth.label().to_string(),
            Field::Weekday => weekday_label(self.weekday).to_string(),
            Field::Month => self.month.map_or("every month", month_label).to_string(),
            Field::Days => self.days.label(),
            Field::KeepAfterPass => if self.keep_after_pass { "yes" } else { "no" }.to_string(),
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
            // Every combination of the three is a valid rule, so there's nothing to refuse.
            RepeatChoice::NthWeekday => RepeatRule::NthWeekday {
                nth: self.nth,
                weekday: self.weekday,
                month: self.month,
            },
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
        // Weekday and Nth-weekday repeats: an empty date becomes the first scheduled day from
        // today, and a date that isn't on the rule moves forward to the next one that is.
        let date = if lands_on_days(repeat) {
            Some(scheduled_on_or_after(repeat, typed_date.unwrap_or(today)))
        } else {
            typed_date
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

    /// The line under a field, if it has one, plus its color slot: under the date, the
    /// understood date; under the time, the understood time; under the Nth weekday's month,
    /// its next two dates. Each UI indents or wraps it as it likes.
    ///
    /// One function for every field, so an app can ask for each row's preview the same way.
    pub fn preview(
        &self,
        field: Field,
        today: NaiveDate,
        time_format: TimeFormat,
    ) -> Option<(String, Slot)> {
        match field {
            Field::Date => Some(self.date_preview(today)),
            Field::Time => Some(self.time_preview(time_format)),
            Field::Month => self.nth_preview(today),
            _ => None,
        }
    }

    /// The line under the date field: the understood date, or how to type one, plus its color
    /// slot. Each UI indents or wraps it as it likes.
    pub fn date_preview(&self, today: NaiveDate) -> (String, Slot) {
        // Weekday and Nth-weekday repeats: say where a date that isn't on the rule, or an
        // empty one, will land when saved. (`if let ... && ...` is a let chain: the block
        // runs only if the pattern matches *and* the condition is true.)
        if let Ok(rule) = self.repeat_rule()
            && lands_on_days(rule)
        {
            // Weekday sets name the weekday and move at most 6 days, so "Sat" and "Mon Oct 12"
            // say enough. An Nth weekday can move into next year, so it names whole dates.
            let nth = matches!(rule, RepeatRule::NthWeekday { .. });
            let short = |date: NaiveDate| date.format("%a %b %d").to_string();
            match parse_date_input(&self.date_text, today) {
                Ok(Some(date)) if scheduled_on_or_after(rule, date) != date => {
                    let moved = scheduled_on_or_after(rule, date);
                    let (typed, saved) = if nth {
                        (date.format("%b %d").to_string(), unbroken_date(moved))
                    } else {
                        (date.format("%a").to_string(), short(moved))
                    };
                    return (
                        format!("→ {typed} isn't a repeat day: saves as {saved}"),
                        Slot::Warn,
                    );
                }
                Ok(None) => {
                    let first = scheduled_on_or_after(rule, today);
                    let first = if nth {
                        unbroken_date(first)
                    } else {
                        short(first)
                    };
                    return (
                        format!("→ empty: saves as {first} (first repeat day)"),
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

    /// The line under the Nth weekday's Month field: the next two dates of the rule, counted
    /// from the typed date (or today, if it's empty or not a date yet), e.g.
    /// "→ Thu Nov 26 2026, Thu Nov 25 2027". `None` for every other repeat.
    ///
    /// Each date stays in one piece when wrapped (see [`unbroken_date`]), so a narrow screen
    /// shows one date per line instead of splitting one.
    pub fn nth_preview(&self, today: NaiveDate) -> Option<(String, Slot)> {
        // `rule @ pattern` matches the pattern and also names the whole value `rule`.
        let rule @ RepeatRule::NthWeekday { .. } = self.repeat_rule().ok()? else {
            return None;
        };
        // `.ok().flatten()`: a parse error and an empty date both become `None`.
        let start = parse_date_input(&self.date_text, today)
            .ok()
            .flatten()
            .unwrap_or(today);
        let first = scheduled_on_or_after(rule, start);
        // The next occurrence strictly after the first: on or after the day after it.
        let second = next_occurrence(rule, first, first + TimeDelta::days(1));
        Some((
            format!("→ {}, {}", unbroken_date(first), unbroken_date(second)),
            Slot::Ok,
        ))
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

/// True for rules that only land on certain days (weekday sets and Nth weekdays). For these
/// the form fills in an empty date and moves a date that isn't on the rule forward.
fn lands_on_days(rule: RepeatRule) -> bool {
    matches!(
        rule,
        RepeatRule::Weekdays(_) | RepeatRule::NthWeekday { .. }
    )
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

    /// A date with non-breaking spaces, as the Nth-weekday previews write it.
    fn nb(text: &str) -> String {
        text.replace(' ', "\u{a0}")
    }

    /// A task form with the Nth-weekday choice picked and the date typed first.
    fn nth_task(date_text: &str) -> FormInput {
        let mut input = task("Thanksgiving");
        input.date_text = date_text.into();
        input.set_repeat(RepeatChoice::NthWeekday);
        input
    }

    #[test]
    fn nth_weekday_choice_sits_after_yearly_and_shows_three_rows() {
        let mut input = task("x");
        input.set_repeat(RepeatChoice::Yearly);
        input.step_choice(Field::Repeat, true);
        assert_eq!(input.repeat, RepeatChoice::NthWeekday);
        assert_eq!(input.field_value(Field::Repeat), "nth weekday");
        input.step_choice(Field::Repeat, true);
        assert_eq!(input.repeat, RepeatChoice::Custom);
        // "Pick days" is still last: one step back from "none".
        input.set_repeat(RepeatChoice::None);
        input.step_choice(Field::Repeat, false);
        assert_eq!(input.repeat, RepeatChoice::PickDays);

        let input = nth_task("");
        assert_eq!(
            input.fields(),
            [
                Field::Title,
                Field::Priority,
                Field::Date,
                Field::Time,
                Field::Repeat,
                Field::Ordinal,
                Field::Weekday,
                Field::Month,
                Field::Notes
            ]
        );
        assert_eq!(Field::Ordinal.label(Kind::Task), "On the");
        assert_eq!(Field::Weekday.label(Kind::Task), "Weekday");
        assert_eq!(Field::Month.label(Kind::Task), "Month");
        // Repeating events don't get "keep after".
        let event = FormInput {
            kind: Kind::Event,
            ..nth_task("")
        };
        assert!(!event.fields().contains(&Field::KeepAfterPass));
    }

    #[test]
    fn choice_fields_and_step_choice_agree() {
        let choices = [
            Field::Priority,
            Field::Repeat,
            Field::EveryUnit,
            Field::Ordinal,
            Field::Weekday,
            Field::Month,
        ];
        for field in choices {
            assert!(field.is_choice(), "{field:?}");
            assert!(!field.is_text(), "{field:?}");
            // Stepping forward then back gives the original value.
            let mut input = nth_task("2026-11-26");
            let before = input.field_value(field);
            input.step_choice(field, true);
            assert_ne!(input.field_value(field), before, "{field:?} moves");
            input.step_choice(field, false);
            assert_eq!(input.field_value(field), before, "{field:?} comes back");
        }
        for field in [
            Field::Title,
            Field::Days,
            Field::KeepAfterPass,
            Field::Notes,
        ] {
            assert!(!field.is_choice(), "{field:?}");
        }
        // The checkbox flips; text fields don't change.
        let mut input = task("x");
        input.step_choice(Field::KeepAfterPass, true);
        assert!(input.keep_after_pass);
        input.step_choice(Field::Title, true);
        assert_eq!(input.title, "x");
    }

    #[test]
    fn nth_weekday_rows_step_and_wrap() {
        let mut input = nth_task("");
        // No full date: the defaults, every 1st Mon.
        assert_eq!(input.field_value(Field::Ordinal), "1st");
        assert_eq!(input.field_value(Field::Weekday), "Mon");
        assert_eq!(input.field_value(Field::Month), "every month");
        let ordinals: Vec<String> = (0..5)
            .map(|_| {
                input.step_choice(Field::Ordinal, true);
                input.field_value(Field::Ordinal)
            })
            .collect();
        assert_eq!(ordinals, ["2nd", "3rd", "4th", "last", "1st"]);
        input.step_choice(Field::Weekday, false);
        assert_eq!(input.field_value(Field::Weekday), "Sun", "wraps back");
        input.step_choice(Field::Month, true);
        assert_eq!(input.field_value(Field::Month), "Jan");
        input.step_choice(Field::Month, false);
        input.step_choice(Field::Month, false);
        assert_eq!(input.field_value(Field::Month), "Dec", "wraps back");
    }

    #[test]
    fn nth_weekday_fills_in_from_a_full_date_only() {
        let input = nth_task("2026-11-26");
        assert_eq!(
            input.repeat_rule(),
            Ok(RepeatRule::NthWeekday {
                nth: Ordinal::Fourth,
                weekday: Weekday::Thu,
                month: Some(Month::November),
            })
        );
        // Day 31 has no 5th: "last". US form works too.
        let input = nth_task("05/31/2027");
        assert_eq!(input.field_value(Field::Ordinal), "last");
        assert_eq!(input.field_value(Field::Weekday), "Mon");
        assert_eq!(input.field_value(Field::Month), "May");
        // Short forms depend on today, so they leave the rows alone.
        for short in ["11/26", "fri", "+3d"] {
            let input = nth_task(short);
            assert_eq!(input.field_value(Field::Ordinal), "1st", "{short}");
            assert_eq!(input.field_value(Field::Month), "every month", "{short}");
        }
    }

    #[test]
    fn nth_weekday_dates_fill_in_or_move_forward_when_saved() {
        // Today is Thu Oct 8 2026. Empty: this year's Thanksgiving.
        let mut input = nth_task("2026-11-26");
        input.date_text.clear();
        let data = input.validate(today()).unwrap();
        assert_eq!(data.date, NaiveDate::from_ymd_opt(2026, 11, 26));
        assert_eq!(data.repeat.to_db_string(), "nth:4:thu:nov");
        // Nov 19 is the 3rd Thursday: moves to the 4th.
        input.date_text = "2026-11-19".into();
        assert_eq!(
            input.validate(today()).unwrap().date,
            NaiveDate::from_ymd_opt(2026, 11, 26)
        );
        // After this year's: next year's.
        input.date_text = "2026-11-27".into();
        assert_eq!(
            input.validate(today()).unwrap().date,
            NaiveDate::from_ymd_opt(2027, 11, 25)
        );
        // Events too.
        let event = FormInput {
            kind: Kind::Event,
            date_text: String::new(),
            ..input
        };
        assert_eq!(
            event.validate(today()).unwrap().date,
            NaiveDate::from_ymd_opt(2026, 11, 26)
        );
    }

    #[test]
    fn nth_weekday_previews() {
        let mut input = nth_task("2026-11-26");
        let expected = format!("→ {}, {}", nb("Thu Nov 26 2026"), nb("Thu Nov 25 2027"));
        assert_eq!(
            input.nth_preview(today()),
            Some((expected.clone(), Slot::Ok))
        );
        assert_eq!(
            input.preview(Field::Month, today(), TimeFormat::Hour12),
            Some((expected, Slot::Ok))
        );
        assert_eq!(
            input.preview(Field::Title, today(), TimeFormat::Hour12),
            None
        );
        // A date that isn't on the rule, and an empty one.
        input.date_text = "2026-11-19".into();
        assert_eq!(
            input.date_preview(today()),
            (
                format!(
                    "→ Nov 19 isn't a repeat day: saves as {}",
                    nb("Thu Nov 26 2026")
                ),
                Slot::Warn
            )
        );
        input.date_text.clear();
        assert_eq!(
            input.date_preview(today()),
            (
                format!(
                    "→ empty: saves as {} (first repeat day)",
                    nb("Thu Nov 26 2026")
                ),
                Slot::Ok
            )
        );
        // First Friday of every month, from Oct 8: Oct 2 has passed, so Nov 6 and Dec 4.
        input.step_choice(Field::Weekday, true); // Thu -> Fri
        input.nth = Ordinal::First;
        input.month = None;
        assert_eq!(
            input.nth_preview(today()).unwrap().0,
            format!("→ {}, {}", nb("Fri Nov 06 2026"), nb("Fri Dec 04 2026"))
        );
        // Not shown for other repeats.
        input.set_repeat(RepeatChoice::Yearly);
        assert_eq!(input.nth_preview(today()), None);
        assert_eq!(
            input.preview(Field::Month, today(), TimeFormat::Hour12),
            None
        );
    }

    #[test]
    fn field_values_as_text() {
        let mut input = task("Report");
        input.date_text = "fri".into();
        input.time_text = "2pm".into();
        assert_eq!(input.field_value(Field::Title), "Report");
        assert_eq!(input.field_value(Field::Date), "fri");
        assert_eq!(input.field_value(Field::Time), "2pm");
        assert_eq!(input.field_value(Field::Priority), "Medium");
        assert_eq!(input.field_value(Field::Repeat), "none");
        assert_eq!(input.field_value(Field::EveryN), "2");
        assert_eq!(input.field_value(Field::EveryUnit), "weeks");
        assert_eq!(input.field_value(Field::Days), "weekdays");
        assert_eq!(input.field_value(Field::KeepAfterPass), "no");
        assert_eq!(input.field_value(Field::Notes), "");
    }

    #[test]
    fn editing_an_nth_weekday_rule_round_trips() {
        for rule in [
            RepeatRule::NthWeekday {
                nth: Ordinal::Last,
                weekday: Weekday::Mon,
                month: Some(Month::May),
            },
            RepeatRule::NthWeekday {
                nth: Ordinal::First,
                weekday: Weekday::Fri,
                month: None,
            },
        ] {
            let mut item = Item::new(Kind::Event, "Holiday", Utc::now());
            // A date on the rule, so saving doesn't move it.
            item.date = Some(scheduled_on_or_after(rule, today()));
            item.repeat = rule;
            let input = FormInput::from_item(&item, TimeFormat::Hour12);
            assert_eq!(input.repeat, RepeatChoice::NthWeekday);
            let data = input.validate(today()).unwrap();
            let mut copy = item.clone();
            data.apply_to(&mut copy);
            assert_eq!(copy, item);
        }
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
