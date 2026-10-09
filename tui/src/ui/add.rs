//! The add/edit dialog for tasks and events.
//!
//! Adding is two steps:
//! 1. **Choose Task or Event** (`Tab` flips, default Task), each with a one-line explanation.
//! 2. **Fill in the fields** for that kind:
//!    - Task:  title, priority, due date, repeat, notes
//!    - Event: title, date, repeat, keep-after-pass, notes
//!
//! Editing skips step 1 (an item's kind can't change) and starts with the fields pre-filled.
//!
//! The date field also has a month-grid picker (Ctrl+D or F2, see `calendar.rs`); typing a
//! date still works.
//!
//! This file holds the form's *state* ([`Form`]), its *key handling* ([`Form::handle_key`]), and
//! its *drawing* ([`draw`]). The app only sees the result: [`FormOutcome::Save`] with a
//! validated [`FormData`], or `Cancel`.

use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use zerohour_core::calendar::WeekStart;
use zerohour_core::icons::{Icon, IconMode};
use zerohour_core::model::{Item, Kind, Priority, RepeatRule, Unit};
use zerohour_core::repeat::{days_until, parse_date_input};
use zerohour_core::theme::Theme;

use super::calendar::{self, Calendar, CalendarOutcome};
use super::{centered, color, popup_block, tail_fit};

/// The repeat options shown in the form, in cycling order. The last one ("custom") reveals the
/// "every N" and unit fields.
const REPEAT_CHOICES: [&str; 6] = ["none", "daily", "weekly", "monthly", "yearly", "custom"];
/// Index of "custom" in [`REPEAT_CHOICES`].
const CUSTOM: usize = 5;

/// Which step of the dialog we're on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    ChooseKind,
    Fields,
}

/// One input in the form. Which ones appear depends on the kind (see [`Form::fields`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Priority,
    Date,
    Repeat,
    EveryN,
    EveryUnit,
    KeepAfterPass,
    Notes,
}

impl Field {
    /// The label shown to the left of the input.
    fn label(self, kind: Kind) -> &'static str {
        match self {
            Field::Title => "Title",
            Field::Priority => "Priority",
            Field::Date if kind == Kind::Task => "Due date",
            Field::Date => "Date",
            Field::Repeat => "Repeat",
            Field::EveryN => "Every",
            Field::EveryUnit => "Unit",
            Field::KeepAfterPass => "Keep after",
            Field::Notes => "Notes",
        }
    }

    /// True for free-text inputs (typing goes into them).
    fn is_text(self) -> bool {
        matches!(
            self,
            Field::Title | Field::Date | Field::EveryN | Field::Notes
        )
    }
}

/// The validated result of a form, ready to be written to the database.
#[derive(Debug, Clone)]
pub struct FormData {
    /// `Some(id)` when editing an existing item, `None` when adding a new one.
    pub editing: Option<String>,
    pub kind: Kind,
    pub title: String,
    pub notes: Option<String>,
    pub priority: Priority,
    pub date: Option<NaiveDate>,
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
        item.repeat = self.repeat;
        item.keep_after_pass = self.keep_after_pass;
    }
}

/// What happened after a key press in the form.
pub enum FormOutcome {
    /// Keep the form open.
    Continue,
    /// User pressed Esc: close without saving.
    Cancel,
    /// User pressed Enter and everything was valid: save this.
    Save(FormData),
    /// The user switched the calendar's first day of the week; the app saves the setting.
    /// The form stays open.
    WeekStartChanged(WeekStart),
}

/// The whole state of the add/edit dialog.
///
/// Inputs are kept as the raw text the user typed (e.g. `date_text`), and only parsed when
/// saving, so a half-typed date like "12/2" doesn't get rejected while you're still typing.
#[derive(Debug, Clone)]
pub struct Form {
    pub step: Step,
    pub kind: Kind,
    /// `Some(id)` when editing.
    pub editing: Option<String>,
    pub title: String,
    pub priority: Priority,
    pub date_text: String,
    /// Index into [`REPEAT_CHOICES`].
    pub repeat_choice: usize,
    pub every_n: String,
    pub unit: Unit,
    pub keep_after_pass: bool,
    pub notes: String,
    /// Index into `self.fields()` of the focused input.
    pub focus: usize,
    /// Validation message shown at the bottom (cleared on the next key press).
    pub error: Option<String>,
    /// The date picker, while it's open over the form.
    pub calendar: Option<Calendar>,
    /// First day of the week for the date picker (from settings).
    pub week_start: WeekStart,
}

impl Form {
    /// A blank form for adding something new (starts at the Task/Event choice).
    /// `week_start` is the saved first day of the week, for the date picker.
    pub fn new_add(week_start: WeekStart) -> Form {
        Form {
            step: Step::ChooseKind,
            kind: Kind::Task,
            editing: None,
            title: String::new(),
            priority: Priority::Medium,
            date_text: String::new(),
            repeat_choice: 0,
            every_n: "2".to_string(),
            unit: Unit::Weeks,
            keep_after_pass: false,
            notes: String::new(),
            focus: 0,
            error: None,
            calendar: None,
            week_start,
        }
    }

    /// A form pre-filled from an existing item, for editing (skips the Task/Event step).
    pub fn edit(item: &Item, week_start: WeekStart) -> Form {
        let mut form = Form::new_add(week_start);
        form.step = Step::Fields;
        form.kind = item.kind;
        form.editing = Some(item.id.clone());
        form.title = item.title.clone();
        form.priority = item.priority;
        // Dates are shown in ISO form; the user can type any accepted form over it.
        form.date_text = item
            .date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        form.keep_after_pass = item.keep_after_pass;
        form.notes = item.notes.clone().unwrap_or_default();
        // Map the stored rule back onto the choice list (+ the custom fields).
        form.repeat_choice = match item.repeat {
            RepeatRule::None => 0,
            RepeatRule::Daily => 1,
            RepeatRule::Weekly => 2,
            RepeatRule::Monthly => 3,
            RepeatRule::Yearly => 4,
            RepeatRule::EveryN { n, unit } => {
                form.every_n = n.to_string();
                form.unit = unit;
                CUSTOM
            }
        };
        form
    }

    /// The inputs shown for the current kind, top to bottom.
    pub fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::Title];
        if self.kind == Kind::Task {
            fields.push(Field::Priority);
        }
        fields.push(Field::Date);
        fields.push(Field::Repeat);
        if self.repeat_choice == CUSTOM {
            fields.push(Field::EveryN);
            fields.push(Field::EveryUnit);
        }
        // "Keep after pass" only matters for one-off events (repeating ones roll forward).
        if self.kind == Kind::Event && self.repeat_choice == 0 {
            fields.push(Field::KeepAfterPass);
        }
        fields.push(Field::Notes);
        fields
    }

    /// The currently focused input.
    fn focused(&self) -> Field {
        let fields = self.fields();
        // `.get()` returns None instead of panicking if `focus` is out of range
        // (can happen right after a field disappears); fall back to the title.
        fields.get(self.focus).copied().unwrap_or(Field::Title)
    }

    /// Handles one key press. `today` is needed to validate dates on save.
    pub fn handle_key(&mut self, key: KeyEvent, today: NaiveDate) -> FormOutcome {
        self.error = None;
        // While the date picker is open, it gets every key (Esc closes only the picker).
        if let Some(calendar) = &mut self.calendar {
            match calendar.handle_key(key, today) {
                CalendarOutcome::Continue => {}
                CalendarOutcome::Cancel => self.calendar = None,
                CalendarOutcome::Pick(date) => {
                    // ISO form: unambiguous, and exactly what editing shows too.
                    self.date_text = date.format("%Y-%m-%d").to_string();
                    self.calendar = None;
                }
                CalendarOutcome::WeekStartChanged(start) => {
                    self.week_start = start;
                    return FormOutcome::WeekStartChanged(start);
                }
            }
            return FormOutcome::Continue;
        }
        if key.code == KeyCode::Esc {
            return FormOutcome::Cancel;
        }
        match self.step {
            Step::ChooseKind => self.handle_choose_kind(key),
            Step::Fields => self.handle_fields(key, today),
        }
    }

    /// Keys on the "Task or Event?" step.
    fn handle_choose_kind(&mut self, key: KeyEvent) -> FormOutcome {
        match key.code {
            KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Char('j')
            | KeyCode::Char('k') => {
                self.kind = match self.kind {
                    Kind::Task => Kind::Event,
                    Kind::Event => Kind::Task,
                };
            }
            // Shortcuts: press t or e to pick and continue in one go.
            KeyCode::Char('t') => {
                self.kind = Kind::Task;
                self.step = Step::Fields;
            }
            KeyCode::Char('e') => {
                self.kind = Kind::Event;
                self.step = Step::Fields;
            }
            KeyCode::Enter => self.step = Step::Fields,
            _ => {}
        }
        FormOutcome::Continue
    }

    /// Keys while filling in fields.
    fn handle_fields(&mut self, key: KeyEvent, today: NaiveDate) -> FormOutcome {
        let field_count = self.fields().len();
        let field = self.focused();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Enter => match self.validate(today) {
                Ok(data) => return FormOutcome::Save(data),
                Err(message) => self.error = Some(message),
            },
            // Move between fields (wrapping around at the ends).
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % field_count,
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = (self.focus + field_count - 1) % field_count;
            }
            // Left/Right change "choice" fields; Space toggles the checkbox.
            KeyCode::Left => self.cycle(field, false),
            KeyCode::Right => self.cycle(field, true),
            KeyCode::Char(' ') if field == Field::KeepAfterPass => {
                self.keep_after_pass = !self.keep_after_pass;
            }
            // Ctrl+D or F2 on the date field opens the month-grid picker, starting on the date
            // typed so far (or today if it's empty or not a date yet).
            KeyCode::Char('d') | KeyCode::F(2)
                if field == Field::Date && (ctrl || key.code == KeyCode::F(2)) =>
            {
                let current = parse_date_input(&self.date_text, today).ok().flatten();
                self.calendar = Some(Calendar::new(current, today, self.week_start));
            }
            // Ctrl+U clears the focused text field (same as in a shell).
            KeyCode::Char('u') if ctrl => {
                if let Some(text) = self.text_mut(field) {
                    text.clear();
                }
            }
            KeyCode::Backspace => {
                if let Some(text) = self.text_mut(field) {
                    text.pop();
                }
            }
            KeyCode::Char(c) if !ctrl => {
                // The "every N" box only accepts digits.
                let allowed = field != Field::EveryN || c.is_ascii_digit();
                if allowed && let Some(text) = self.text_mut(field) {
                    text.push(c);
                }
            }
            _ => {}
        }
        // A field may have appeared/disappeared (e.g. repeat changed); keep focus in range.
        self.focus = self.focus.min(self.fields().len() - 1);
        FormOutcome::Continue
    }

    /// Returns a mutable borrow of the text behind a text field, or `None` for choice fields.
    ///
    /// `Option<&mut String>`: maybe a *mutable reference* — a temporary, exclusive permission
    /// to change the string that lives inside `self`.
    fn text_mut(&mut self, field: Field) -> Option<&mut String> {
        match field {
            Field::Title => Some(&mut self.title),
            Field::Date => Some(&mut self.date_text),
            Field::EveryN => Some(&mut self.every_n),
            Field::Notes => Some(&mut self.notes),
            _ => None,
        }
    }

    /// Steps a choice field forward (`right = true`) or backward.
    fn cycle(&mut self, field: Field, right: bool) {
        /// Moves an index one step around a list of length `len`, wrapping at both ends.
        fn step(i: usize, len: usize, right: bool) -> usize {
            if right {
                (i + 1) % len
            } else {
                (i + len - 1) % len
            }
        }
        match field {
            Field::Priority => {
                let i = Priority::ALL
                    .iter()
                    .position(|&p| p == self.priority)
                    .unwrap_or(1);
                self.priority = Priority::ALL[step(i, Priority::ALL.len(), right)];
            }
            Field::Repeat => {
                self.repeat_choice = step(self.repeat_choice, REPEAT_CHOICES.len(), right);
            }
            Field::EveryUnit => {
                let i = Unit::ALL.iter().position(|&u| u == self.unit).unwrap_or(0);
                self.unit = Unit::ALL[step(i, Unit::ALL.len(), right)];
            }
            Field::KeepAfterPass => self.keep_after_pass = !self.keep_after_pass,
            _ => {}
        }
    }

    /// The repeat rule currently chosen, or an error message for a bad "every N".
    fn repeat_rule(&self) -> Result<RepeatRule, String> {
        Ok(match self.repeat_choice {
            0 => RepeatRule::None,
            1 => RepeatRule::Daily,
            2 => RepeatRule::Weekly,
            3 => RepeatRule::Monthly,
            4 => RepeatRule::Yearly,
            _ => {
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
        let date = parse_date_input(&self.date_text, today)?;
        if self.kind == Kind::Event && date.is_none() {
            return Err("Events need a date to count down to.".to_string());
        }
        let repeat = self.repeat_rule()?;
        let notes = self.notes.trim();
        Ok(FormData {
            editing: self.editing.clone(),
            kind: self.kind,
            title: title.to_string(),
            notes: (!notes.is_empty()).then(|| notes.to_string()),
            priority: self.priority,
            date,
            repeat,
            // Only meaningful for one-off events; stored false otherwise to keep data tidy.
            keep_after_pass: self.kind == Kind::Event
                && !repeat.is_repeating()
                && self.keep_after_pass,
        })
    }
}

/// Draws the dialog as a centered popup over whatever is behind it.
pub fn draw(frame: &mut Frame, form: &Form, theme: &Theme, icons: IconMode, today: NaiveDate) {
    match form.step {
        Step::ChooseKind => draw_choose_kind(frame, form, theme, icons),
        Step::Fields => draw_fields(frame, form, theme, today),
    }
    // The date picker goes on top of the form.
    if let Some(calendar) = &form.calendar {
        calendar::draw(frame, calendar, theme, today);
    }
}

/// Step 1: "Task or Event?"
fn draw_choose_kind(frame: &mut Frame, form: &Form, theme: &Theme, icons: IconMode) {
    let area = centered(frame.area(), 62, 9);
    let block = popup_block(" Add ", theme, color(theme.accent));

    // One option = a name line plus a hint line; the chosen one gets an arrow and color.
    let option = |kind: Kind, name: &str, icon: Icon, hint: &str, section_color| {
        let chosen = form.kind == kind;
        let marker = if chosen { "› " } else { "  " };
        let name_style = if chosen {
            Style::new().fg(section_color).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(color(theme.muted))
        };
        vec![
            Line::from(vec![
                Span::styled(marker, Style::new().fg(color(theme.accent))),
                Span::styled(format!("{} {name}", icon.get(icons)), name_style),
            ]),
            Line::from(Span::styled(
                format!("    {hint}"),
                Style::new().fg(color(theme.muted)),
            )),
        ]
    };

    let mut lines = Vec::new();
    lines.extend(option(
        Kind::Task,
        "Task",
        Icon::Task,
        "Something to do, optionally with a due date.",
        color(theme.task),
    ));
    lines.push(Line::default());
    lines.extend(option(
        Kind::Event,
        "Event",
        Icon::Event,
        "A date to count down to, like a birthday or holiday.",
        color(theme.event),
    ));
    lines.push(Line::default());
    lines.push(hint_line(
        "Tab switch · Enter continue · t/e pick · Esc cancel",
        theme,
    ));

    frame.render_widget(Clear, area); // erase what's behind the popup
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Step 2: the fields.
fn draw_fields(frame: &mut Frame, form: &Form, theme: &Theme, today: NaiveDate) {
    let fields = form.fields();
    // Height: one line per field + date preview + blank + error/hint lines + borders.
    let height = fields.len() as u16 + 6;
    let area = centered(frame.area(), 64, height);
    let (verb, noun) = match (&form.editing, form.kind) {
        (Some(_), Kind::Task) => ("Edit", "task"),
        (Some(_), Kind::Event) => ("Edit", "event"),
        (None, Kind::Task) => ("New", "task"),
        (None, Kind::Event) => ("New", "event"),
    };
    let section_color = match form.kind {
        Kind::Task => color(theme.task),
        Kind::Event => color(theme.event),
    };
    let block = popup_block(&format!(" {verb} {noun} "), theme, section_color);
    let inner = block.inner(area);

    const LABEL_WIDTH: usize = 11;
    // Space left for the value after "› " + label + " ".
    let value_width = (inner.width as usize).saturating_sub(LABEL_WIDTH + 3);
    let focused = form.focused();

    let mut lines: Vec<Line> = Vec::new();
    for field in &fields {
        let is_focused = *field == focused;
        let label_style = if is_focused {
            Style::new()
                .fg(color(theme.accent))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(color(theme.muted))
        };
        let value = field_value(form, *field, is_focused, value_width);
        lines.push(Line::from(vec![
            Span::styled(if is_focused { "› " } else { "  " }, label_style),
            Span::styled(
                format!("{:<LABEL_WIDTH$} ", field.label(form.kind)),
                label_style,
            ),
            Span::styled(value, Style::new().fg(color(theme.fg))),
        ]));
        // Right under the date: show how the typed text was understood.
        if *field == Field::Date {
            lines.push(date_preview(form, theme, today));
        }
    }
    lines.push(Line::default());
    match &form.error {
        Some(message) => lines.push(Line::from(Span::styled(
            format!("  {message}"),
            Style::new().fg(color(theme.danger)),
        ))),
        None => lines.push(Line::default()),
    }
    // On the date field, mention the picker instead of the ←→ hint (which does nothing there).
    let hint = if focused == Field::Date {
        "Tab/↑↓ move · Ctrl+D/F2 calendar · Enter save · Esc cancel"
    } else {
        "Tab/↑↓ move · ←→ change · Enter save · Esc cancel"
    };
    lines.push(hint_line(hint, theme));

    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The text shown for one field's value. Text fields get a cursor when focused; choice fields
/// show `‹ value ›` arrows when focused to hint that ←/→ change them.
fn field_value(form: &Form, field: Field, focused: bool, width: usize) -> String {
    let choice = |s: &str| {
        if focused {
            format!("‹ {s} ›")
        } else {
            s.to_string()
        }
    };
    match field {
        Field::Priority => choice(form.priority.label()),
        Field::Repeat => choice(REPEAT_CHOICES[form.repeat_choice]),
        Field::EveryUnit => choice(form.unit.as_str()),
        Field::KeepAfterPass => {
            let box_ = if form.keep_after_pass { "[x]" } else { "[ ]" };
            format!("{box_} show after the date passes")
        }
        _ => {
            // Text field: show the end of the text if it's too long (that's where you type).
            let text = match field {
                Field::Title => &form.title,
                Field::Date => &form.date_text,
                Field::EveryN => &form.every_n,
                _ => &form.notes,
            };
            debug_assert!(field.is_text());
            if focused {
                format!("{}▏", tail_fit(text, width.saturating_sub(1)))
            } else if text.is_empty() && field == Field::Date {
                "(none)".to_string()
            } else {
                tail_fit(text, width)
            }
        }
    }
}

/// The muted line under the date field: either the understood date or how to type one.
fn date_preview<'a>(form: &Form, theme: &Theme, today: NaiveDate) -> Line<'a> {
    let indent = " ".repeat(14);
    let (text, slot) = match parse_date_input(&form.date_text, today) {
        Ok(Some(date)) => {
            let days = days_until(date, today);
            let relative = match days {
                0 => "today".to_string(),
                1 => "tomorrow".to_string(),
                -1 => "yesterday".to_string(),
                n if n > 0 => format!("in {n} days"),
                n => format!("{} days ago", -n),
            };
            (
                format!("{indent}→ {} · {relative}", date.format("%a %b %d %Y")),
                theme.ok,
            )
        }
        Ok(None) => (
            format!("{indent}e.g. 12/25, 2026-12-25, +3d, +2w, tomorrow, fri"),
            theme.muted,
        ),
        Err(_) => (format!("{indent}? not a date yet"), theme.warn),
    };
    Line::from(Span::styled(text, Style::new().fg(color(slot))))
}

/// A dim one-line list of keys at the bottom of a popup.
fn hint_line<'a>(text: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(Span::styled(
        format!("  {text}"),
        Style::new().fg(color(theme.muted)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 8).unwrap()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(form: &mut Form, text: &str) {
        for c in text.chars() {
            form.handle_key(key(KeyCode::Char(c)), today());
        }
    }

    #[test]
    fn tab_flips_kind_then_enter_shows_event_fields() {
        let mut form = Form::new_add(WeekStart::Sunday);
        assert_eq!(form.kind, Kind::Task);
        form.handle_key(key(KeyCode::Tab), today());
        assert_eq!(form.kind, Kind::Event);
        form.handle_key(key(KeyCode::Enter), today());
        assert_eq!(form.step, Step::Fields);
        let fields = form.fields();
        assert!(!fields.contains(&Field::Priority));
        assert!(fields.contains(&Field::KeepAfterPass));
    }

    #[test]
    fn typing_a_task_and_saving() {
        let mut form = Form::new_add(WeekStart::Sunday);
        form.handle_key(key(KeyCode::Enter), today()); // Task
        type_text(&mut form, "Renew registration");
        form.handle_key(key(KeyCode::Tab), today()); // -> priority
        form.handle_key(key(KeyCode::Right), today()); // Medium -> High
        form.handle_key(key(KeyCode::Tab), today()); // -> date
        type_text(&mut form, "+3d");
        match form.handle_key(key(KeyCode::Enter), today()) {
            FormOutcome::Save(data) => {
                assert_eq!(data.title, "Renew registration");
                assert_eq!(data.priority, Priority::High);
                assert_eq!(data.date, NaiveDate::from_ymd_opt(2026, 10, 11));
                assert_eq!(data.repeat, RepeatRule::None);
            }
            _ => panic!("expected save"),
        }
    }

    #[test]
    fn custom_repeat_reveals_every_n_fields() {
        let mut form = Form::new_add(WeekStart::Sunday);
        form.step = Step::Fields;
        form.title = "x".into();
        form.repeat_choice = CUSTOM;
        assert!(form.fields().contains(&Field::EveryN));
        form.every_n = "3".into();
        form.unit = Unit::Days;
        let data = form.validate(today()).unwrap();
        assert_eq!(
            data.repeat,
            RepeatRule::EveryN {
                n: 3,
                unit: Unit::Days
            }
        );
        form.every_n = "0".into();
        assert!(form.validate(today()).is_err());
    }

    #[test]
    fn validation_errors() {
        let mut form = Form::new_add(WeekStart::Sunday);
        form.step = Step::Fields;
        assert!(form.validate(today()).is_err(), "empty title");
        form.title = "Party".into();
        form.kind = Kind::Event;
        assert!(form.validate(today()).is_err(), "event without date");
        form.date_text = "nonsense".into();
        assert!(form.validate(today()).is_err(), "bad date");
        form.date_text = "12/25".into();
        assert!(form.validate(today()).is_ok());
    }

    #[test]
    fn ctrl_d_opens_the_calendar_and_enter_fills_the_date() {
        let mut form = Form::new_add(WeekStart::Sunday);
        form.handle_key(key(KeyCode::Enter), today()); // Task
        form.focus = form
            .fields()
            .iter()
            .position(|f| *f == Field::Date)
            .unwrap();
        type_text(&mut form, "12/25");
        let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        form.handle_key(ctrl_d, today());
        let calendar = form.calendar.as_ref().expect("picker open");
        assert_eq!(
            calendar.cursor,
            NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()
        );

        // Esc closes only the picker, then F2 reopens it; → and Enter pick Dec 26.
        form.handle_key(key(KeyCode::Esc), today());
        assert!(form.calendar.is_none());
        assert_eq!(form.step, Step::Fields, "the form is still open");
        form.handle_key(key(KeyCode::F(2)), today());
        form.handle_key(key(KeyCode::Right), today());
        form.handle_key(key(KeyCode::Enter), today());
        assert!(form.calendar.is_none());
        assert_eq!(form.date_text, "2026-12-26");
    }

    #[test]
    fn plain_d_is_still_typed_and_the_picker_only_opens_on_the_date_field() {
        let mut form = Form::new_add(WeekStart::Sunday);
        form.handle_key(key(KeyCode::Enter), today());
        type_text(&mut form, "dd"); // title field
        assert_eq!(form.title, "dd");
        form.handle_key(key(KeyCode::F(2)), today());
        assert!(form.calendar.is_none(), "not on the title field");
    }

    #[test]
    fn edit_prefills_and_round_trips_repeat() {
        let mut item = Item::new(Kind::Event, "Xmas", chrono::Utc::now());
        item.date = NaiveDate::from_ymd_opt(2026, 12, 25);
        item.repeat = RepeatRule::EveryN {
            n: 2,
            unit: Unit::Months,
        };
        let form = Form::edit(&item, WeekStart::Sunday);
        assert_eq!(form.step, Step::Fields);
        let data = form.validate(today()).unwrap();
        assert_eq!(data.repeat, item.repeat);
        assert_eq!(data.date, item.date);
        assert_eq!(data.editing.as_deref(), Some(item.id.as_str()));
    }
}
