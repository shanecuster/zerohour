//! The add/edit dialog for tasks and events.
//!
//! Adding is two steps:
//! 1. **Choose Task or Event** (`Tab` flips, default Task), each with a one-line explanation.
//! 2. **Fill in the fields** for that kind:
//!    - Task:  title, priority, due date, due time, repeat, notes
//!
//! Repeat choices include weekday presets ("weekdays", "Mon–Thu") and "pick days"; those
//! show a row of day checkboxes. A start date that isn't one of the chosen days moves forward
//! to the next one on save (the date preview says so), and an empty date is filled in.
//!    - Event: title, date, repeat, keep-after-pass, notes
//!
//! Editing skips step 1 (an item's kind can't change) and starts with the fields pre-filled.
//!
//! The date field also has a month-grid picker (Ctrl+D or F2, see `calendar.rs`); typing a
//! date still works.
//!
//! The form's values, which fields appear, the repeat choices, validation and the date/time
//! previews live in core (`zerohour_core::form`), so the Android app's form behaves exactly
//! the same. This file holds the terminal side: the dialog *state* on top of that ([`Form`]),
//! its *key handling* ([`Form::handle_key`]), and its *drawing* ([`draw`]). The app only sees
//! the result: [`FormOutcome::Save`] with a validated [`FormData`], or `Cancel`.

use chrono::{NaiveDate, Weekday};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use zerohour_core::calendar::WeekStart;
use zerohour_core::clock::TimeFormat;
use zerohour_core::form::{Field, FormData, FormInput, RepeatChoice};
use zerohour_core::icons::{Icon, IconMode};
use zerohour_core::model::{Item, Kind, Priority, Unit};
use zerohour_core::repeat::parse_date_input;
use zerohour_core::theme::{Rgb, Theme};

use super::calendar::{self, Calendar, CalendarOutcome, GridArea};
use super::{
    centered, color, popup_block, popup_height, popup_text_width, tail_fit, truncate, wrapped_lines,
};
use zerohour_core::calendar::add_months;

/// Which step of the dialog we're on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    ChooseKind,
    Fields,
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
/// The values themselves (title, typed date, repeat choice...) live in a core
/// [`FormInput`], shared with the Android app, which also knows which fields appear and how to
/// validate them. This struct adds only what the terminal needs on top: the step, which field
/// has focus, the day-checkbox cursor, the error line, and the open date picker.
#[derive(Debug, Clone)]
pub struct Form {
    pub step: Step,
    /// Everything typed or chosen so far (see `zerohour_core::form`).
    pub input: FormInput,
    /// Which day checkbox ←/→ is on (0..7, in the order shown).
    pub day_cursor: usize,
    /// Index into `self.fields()` of the focused input.
    pub focus: usize,
    /// Validation message shown at the bottom (cleared on the next key press).
    pub error: Option<String>,
    /// The date picker, while it's open over the form.
    pub calendar: Option<Calendar>,
    /// Display settings the form needs (week start, 12/24-hour times).
    pub prefs: FormPrefs,
}

/// The user's display settings that the form needs, passed in when it opens.
///
/// `Default` gives Sunday and 12-hour times, the app's defaults (handy in tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FormPrefs {
    /// First day of the week in the date picker.
    pub week_start: WeekStart,
    /// How the due time is shown when editing and in the preview.
    pub time_format: TimeFormat,
}

impl Form {
    /// A blank form for adding something new (starts at the Task/Event choice).
    pub fn new_add(prefs: FormPrefs) -> Form {
        Form {
            step: Step::ChooseKind,
            input: FormInput::default(),
            day_cursor: 0,
            focus: 0,
            error: None,
            calendar: None,
            prefs,
        }
    }

    /// A form pre-filled from an existing item, for editing (skips the Task/Event step).
    pub fn edit(item: &Item, prefs: FormPrefs) -> Form {
        let mut form = Form::new_add(prefs);
        form.step = Step::Fields;
        form.input = FormInput::from_item(item, prefs.time_format);
        form
    }

    /// A left click while the date picker is open: if it hit a day (per `grid`, where the
    /// picker was last drawn), put that date in the field and close the picker.
    pub fn click_calendar(&mut self, grid: GridArea, column: u16, row: u16) {
        let Some(calendar) = &self.calendar else {
            return;
        };
        if let Some(date) = grid.date_at(calendar, column, row) {
            self.input.set_date(date);
            self.calendar = None;
        }
    }

    /// The scroll wheel over the open date picker: one month back (`-1`) or forward (`1`).
    pub fn scroll_calendar(&mut self, months: i32) {
        if let Some(calendar) = &mut self.calendar {
            calendar.cursor = add_months(calendar.cursor, months);
        }
    }

    /// The inputs shown for the current kind and repeat choice, top to bottom (core decides).
    pub fn fields(&self) -> Vec<Field> {
        self.input.fields()
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
                    // Core writes it in ISO form: unambiguous, and what editing shows too.
                    self.input.set_date(date);
                    self.calendar = None;
                }
                CalendarOutcome::WeekStartChanged(start) => {
                    self.prefs.week_start = start;
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
                self.input.kind = match self.input.kind {
                    Kind::Task => Kind::Event,
                    Kind::Event => Kind::Task,
                };
            }
            // Shortcuts: press t or e to pick and continue in one go.
            KeyCode::Char('t') => {
                self.input.kind = Kind::Task;
                self.step = Step::Fields;
            }
            KeyCode::Char('e') => {
                self.input.kind = Kind::Event;
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
            KeyCode::Enter => match self.input.validate(today) {
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
                self.input.keep_after_pass = !self.input.keep_after_pass;
            }
            KeyCode::Char(' ') if field == Field::Days => self.toggle_day(),
            // Ctrl+D or F2 on the date field opens the month-grid picker, starting on the date
            // typed so far (or today if it's empty or not a date yet).
            KeyCode::Char('d') | KeyCode::F(2)
                if field == Field::Date && (ctrl || key.code == KeyCode::F(2)) =>
            {
                let current = parse_date_input(&self.input.date_text, today)
                    .ok()
                    .flatten();
                self.calendar = Some(Calendar::new(current, today, self.prefs.week_start));
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
            Field::Title => Some(&mut self.input.title),
            Field::Date => Some(&mut self.input.date_text),
            Field::Time => Some(&mut self.input.time_text),
            Field::EveryN => Some(&mut self.input.every_n),
            Field::Notes => Some(&mut self.input.notes),
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
                    .position(|&p| p == self.input.priority)
                    .unwrap_or(1);
                self.input.priority = Priority::ALL[step(i, Priority::ALL.len(), right)];
            }
            Field::Repeat => {
                let i = RepeatChoice::ALL
                    .iter()
                    .position(|&c| c == self.input.repeat)
                    .unwrap_or(0);
                // `set_repeat` also fills in a preset's days ("pick days" keeps what's there).
                self.input
                    .set_repeat(RepeatChoice::ALL[step(i, RepeatChoice::ALL.len(), right)]);
            }
            // On the day checkboxes, ←/→ move between days (Space toggles one).
            Field::Days => self.day_cursor = step(self.day_cursor, 7, right),
            Field::EveryUnit => {
                let i = Unit::ALL
                    .iter()
                    .position(|&u| u == self.input.unit)
                    .unwrap_or(0);
                self.input.unit = Unit::ALL[step(i, Unit::ALL.len(), right)];
            }
            Field::KeepAfterPass => self.input.keep_after_pass = !self.input.keep_after_pass,
            _ => {}
        }
    }

    /// The days in the order the checkboxes are shown (following the calendar's week start).
    fn day_order(&self) -> [Weekday; 7] {
        self.prefs.week_start.weekdays()
    }

    /// Space on the day checkboxes: flips the day under the cursor (core then names the
    /// matching preset, "weekdays" / "Mon–Thu", or "pick days").
    fn toggle_day(&mut self) {
        let day = self.day_order()[self.day_cursor];
        self.input.toggle_day(day);
    }
}

/// Draws the dialog as a centered popup over whatever is behind it. Returns where the date
/// picker's day grid is, if the picker is open (for mouse clicks).
pub fn draw(
    frame: &mut Frame,
    form: &Form,
    theme: &Theme,
    icons: IconMode,
    today: NaiveDate,
) -> Option<GridArea> {
    match form.step {
        Step::ChooseKind => draw_choose_kind(frame, form, theme, icons),
        Step::Fields => draw_fields(frame, form, theme, today),
    }
    // The date picker goes on top of the form.
    form.calendar
        .as_ref()
        .map(|calendar| calendar::draw(frame, calendar, theme, today))
}

/// Step 1: "Task or Event?"
fn draw_choose_kind(frame: &mut Frame, form: &Form, theme: &Theme, icons: IconMode) {
    // The popup is 62 columns, or the whole width on a smaller terminal. Text is wrapped to
    // fit, and the height follows from the number of lines.
    let width = 62.min(frame.area().width);
    let text_width = popup_text_width(width);
    let muted = Style::new().fg(color(theme.muted));

    // One option = a name line plus a hint line; the chosen one gets an arrow and color.
    let option = |kind: Kind, name: &str, icon: Icon, hint: &str, section_color| {
        let chosen = form.input.kind == kind;
        let marker = if chosen { "› " } else { "  " };
        let name_style = if chosen {
            Style::new().fg(section_color).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(color(theme.muted))
        };
        let mut lines = vec![Line::from(vec![
            Span::styled(marker, Style::new().fg(color(theme.accent))),
            Span::styled(format!("{} {name}", icon.get(icons)), name_style),
        ])];
        lines.extend(wrapped_lines(&format!("    {hint}"), text_width, 4, muted));
        lines
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
    lines.extend(hint_lines(
        "Tab switch · Enter continue · t/e pick · Esc cancel",
        text_width,
        theme,
    ));

    let area = centered(frame.area(), width, popup_height(lines.len()));
    let block = popup_block(" Add ", theme, color(theme.accent));
    frame.render_widget(Clear, area); // erase what's behind the popup
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Step 2: the fields.
fn draw_fields(frame: &mut Frame, form: &Form, theme: &Theme, today: NaiveDate) {
    let fields = form.fields();
    // 64 columns, or the whole width on a smaller terminal. Previews, errors and hints wrap
    // to fit, so the height is worked out from the lines once they're built.
    let width = 64.min(frame.area().width);
    let text_width = popup_text_width(width);
    let (verb, noun) = match (&form.input.editing, form.input.kind) {
        (Some(_), Kind::Task) => ("Edit", "task"),
        (Some(_), Kind::Event) => ("Edit", "event"),
        (None, Kind::Task) => ("New", "task"),
        (None, Kind::Event) => ("New", "event"),
    };
    let section_color = match form.input.kind {
        Kind::Task => color(theme.task),
        Kind::Event => color(theme.event),
    };

    const LABEL_WIDTH: usize = 11;
    // Space left for the value after "› " + label + " ".
    let value_width = text_width.saturating_sub(LABEL_WIDTH + 3);
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
        // Values can't wrap (they're one input each), so the rare too-long one (the day
        // checkboxes on a narrow screen) is shortened with "…" instead.
        let value = truncate(
            &field_value(form, *field, is_focused, value_width),
            value_width,
        );
        lines.push(Line::from(vec![
            Span::styled(if is_focused { "› " } else { "  " }, label_style),
            Span::styled(
                format!("{:<LABEL_WIDTH$} ", field.label(form.input.kind)),
                label_style,
            ),
            Span::styled(value, Style::new().fg(color(theme.fg))),
        ]));
        // Right under the date and time: how the typed text was understood, wrapped under
        // the value column (14 = "› " + label + " ").
        let preview = match field {
            Field::Date => Some(date_preview(form, theme, today)),
            Field::Time => Some(time_preview(form, theme)),
            _ => None,
        };
        if let Some((text, slot)) = preview {
            let style = Style::new().fg(color(slot));
            lines.extend(wrapped_lines(&text, text_width, 14, style));
        }
    }
    lines.push(Line::default());
    match &form.error {
        Some(message) => lines.extend(wrapped_lines(
            &format!("  {message}"),
            text_width,
            2,
            Style::new().fg(color(theme.danger)),
        )),
        None => lines.push(Line::default()),
    }
    // The hint names the keys that do something on the focused field.
    let hint = match focused {
        Field::Date => "Tab/↑↓ move · Ctrl+D/F2 calendar · Enter save · Esc cancel",
        Field::Days => "↑↓ move · ←→ day · Space on/off · Enter save · Esc cancel",
        _ => "Tab/↑↓ move · ←→ change · Enter save · Esc cancel",
    };
    lines.extend(hint_lines(hint, text_width, theme));

    let area = centered(frame.area(), width, popup_height(lines.len()));
    let block = popup_block(&format!(" {verb} {noun} "), theme, section_color);
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
        Field::Priority => choice(form.input.priority.label()),
        Field::Repeat => choice(form.input.repeat.label()),
        Field::Days => {
            // "[x]Mo [x]Tu [ ]We ..."; the day under the cursor shows as "‹x›Mo" when focused.
            form.day_order()
                .iter()
                .enumerate()
                .map(|(i, &day)| {
                    let mark = if form.input.days.contains(day) {
                        "x"
                    } else {
                        " "
                    };
                    // Two-letter name: the first two characters of chrono's "Mon", "Tue"...
                    let name: String = day.to_string().chars().take(2).collect();
                    if focused && i == form.day_cursor {
                        format!("‹{mark}›{name}")
                    } else {
                        format!("[{mark}]{name}")
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        Field::EveryUnit => choice(form.input.unit.as_str()),
        Field::KeepAfterPass => {
            let box_ = if form.input.keep_after_pass {
                "[x]"
            } else {
                "[ ]"
            };
            format!("{box_} show after the date passes")
        }
        _ => {
            // Text field: show the end of the text if it's too long (that's where you type).
            let text = match field {
                Field::Title => &form.input.title,
                Field::Date => &form.input.date_text,
                Field::Time => &form.input.time_text,
                Field::EveryN => &form.input.every_n,
                _ => &form.input.notes,
            };
            debug_assert!(field.is_text());
            if focused {
                format!("{}▏", tail_fit(text, width.saturating_sub(1)))
            } else if text.is_empty() && matches!(field, Field::Date | Field::Time) {
                "(none)".to_string()
            } else {
                tail_fit(text, width)
            }
        }
    }
}

/// The line under the date field, indented to the value column: the understood date, or how
/// to type one. The words and color slot come from core; returned as (text, color) for the
/// caller to wrap.
fn date_preview(form: &Form, theme: &Theme, today: NaiveDate) -> (String, Rgb) {
    let (text, slot) = form.input.date_preview(today);
    (format!("{PREVIEW_INDENT}{text}"), theme.slot(slot))
}

/// The line under the time field: the understood time, or how to type one (words from core).
fn time_preview(form: &Form, theme: &Theme) -> (String, Rgb) {
    let (text, slot) = form.input.time_preview(form.prefs.time_format);
    (format!("{PREVIEW_INDENT}{text}"), theme.slot(slot))
}

/// Previews start under the value column: "› " + an 11-wide label + " " = 14 columns.
const PREVIEW_INDENT: &str = "              ";

/// The dim list of keys at the bottom of a popup, wrapped onto more lines if it's too wide.
fn hint_lines(text: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    wrapped_lines(
        &format!("  {text}"),
        width,
        2,
        Style::new().fg(color(theme.muted)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;
    use zerohour_core::model::{DaySet, RepeatRule};

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
        let mut form = Form::new_add(FormPrefs::default());
        assert_eq!(form.input.kind, Kind::Task);
        form.handle_key(key(KeyCode::Tab), today());
        assert_eq!(form.input.kind, Kind::Event);
        form.handle_key(key(KeyCode::Enter), today());
        assert_eq!(form.step, Step::Fields);
        let fields = form.fields();
        assert!(!fields.contains(&Field::Priority));
        assert!(fields.contains(&Field::KeepAfterPass));
    }

    #[test]
    fn typing_a_task_and_saving() {
        let mut form = Form::new_add(FormPrefs::default());
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
        let mut form = Form::new_add(FormPrefs::default());
        form.step = Step::Fields;
        form.input.title = "x".into();
        form.input.repeat = RepeatChoice::Custom;
        assert!(form.fields().contains(&Field::EveryN));
        form.input.every_n = "3".into();
        form.input.unit = Unit::Days;
        let data = form.input.validate(today()).unwrap();
        assert_eq!(
            data.repeat,
            RepeatRule::EveryN {
                n: 3,
                unit: Unit::Days
            }
        );
        form.input.every_n = "0".into();
        assert!(form.input.validate(today()).is_err());
    }

    #[test]
    fn validation_errors() {
        let mut form = Form::new_add(FormPrefs::default());
        form.step = Step::Fields;
        assert!(form.input.validate(today()).is_err(), "empty title");
        form.input.title = "Party".into();
        form.input.kind = Kind::Event;
        assert!(form.input.validate(today()).is_err(), "event without date");
        form.input.date_text = "nonsense".into();
        assert!(form.input.validate(today()).is_err(), "bad date");
        form.input.date_text = "12/25".into();
        assert!(form.input.validate(today()).is_ok());
    }

    #[test]
    fn ctrl_d_opens_the_calendar_and_enter_fills_the_date() {
        let mut form = Form::new_add(FormPrefs::default());
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
        assert_eq!(form.input.date_text, "2026-12-26");
    }

    #[test]
    fn plain_d_is_still_typed_and_the_picker_only_opens_on_the_date_field() {
        let mut form = Form::new_add(FormPrefs::default());
        form.handle_key(key(KeyCode::Enter), today());
        type_text(&mut form, "dd"); // title field
        assert_eq!(form.input.title, "dd");
        form.handle_key(key(KeyCode::F(2)), today());
        assert!(form.calendar.is_none(), "not on the title field");
    }

    #[test]
    fn tasks_take_an_optional_due_time_that_needs_a_date() {
        let mut form = Form::new_add(FormPrefs::default());
        form.step = Step::Fields;
        form.input.title = "standup".into();
        assert!(form.fields().contains(&Field::Time));
        form.input.time_text = "2pm".into();
        assert!(form.input.validate(today()).is_err(), "time without a date");
        form.input.date_text = "today".into();
        let data = form.input.validate(today()).unwrap();
        assert_eq!(data.due_time, NaiveTime::from_hms_opt(14, 0, 0));
        form.input.time_text = "25:00".into();
        assert!(form.input.validate(today()).is_err(), "bad time");

        // Events have no time field and never store one.
        form.input.kind = Kind::Event;
        assert!(!form.fields().contains(&Field::Time));
        form.input.time_text = "2pm".into();
        assert_eq!(form.input.validate(today()).unwrap().due_time, None);
    }

    #[test]
    fn editing_shows_the_time_in_the_chosen_style() {
        let mut item = Item::new(Kind::Task, "standup", chrono::Utc::now());
        item.date = Some(today());
        item.due_time = NaiveTime::from_hms_opt(14, 0, 0);
        let twelve = Form::edit(&item, FormPrefs::default());
        assert_eq!(twelve.input.time_text, "2:00 PM");
        let prefs = FormPrefs {
            time_format: TimeFormat::Hour24,
            ..FormPrefs::default()
        };
        assert_eq!(Form::edit(&item, prefs).input.time_text, "14:00");
        assert_eq!(
            twelve.input.validate(today()).unwrap().due_time,
            item.due_time
        );
    }

    /// A task form on the fields step with a title, ready for repeat tests.
    fn task_form() -> Form {
        let mut form = Form::new_add(FormPrefs::default());
        form.step = Step::Fields;
        form.input.title = "standup".into();
        form
    }

    /// Moves focus to `field`.
    fn focus_on(form: &mut Form, field: Field) {
        form.focus = form.fields().iter().position(|f| *f == field).unwrap();
    }

    #[test]
    fn weekday_presets_fill_the_days_and_show_the_checkboxes() {
        let mut form = task_form();
        assert!(!form.fields().contains(&Field::Days));
        focus_on(&mut form, Field::Repeat);
        form.handle_key(key(KeyCode::Right), today()); // daily
        form.handle_key(key(KeyCode::Right), today()); // weekdays
        assert_eq!(form.input.repeat.label(), "weekdays");
        assert_eq!(form.input.days, DaySet::WEEKDAYS);
        assert!(form.fields().contains(&Field::Days));
        form.handle_key(key(KeyCode::Right), today()); // Mon–Thu
        assert_eq!(form.input.days, DaySet::MON_THU);
        form.input.date_text = "2026-10-10".into(); // a Saturday, not one of Mon–Thu...
        let data = form.input.validate(today()).unwrap();
        assert_eq!(data.repeat, RepeatRule::Weekdays(DaySet::MON_THU));
        // ...so the saved date moves forward to Monday.
        assert_eq!(data.date, NaiveDate::from_ymd_opt(2026, 10, 12));
    }

    #[test]
    fn toggling_days_switches_between_presets_and_pick_days() {
        let mut form = task_form();
        form.input.repeat = RepeatChoice::Weekdays;
        form.input.days = DaySet::WEEKDAYS;
        focus_on(&mut form, Field::Days);
        // Sunday-first order: index 5 is Friday. Turning it off leaves Mon–Thu.
        form.day_cursor = 0;
        for _ in 0..5 {
            form.handle_key(key(KeyCode::Right), today());
        }
        form.handle_key(key(KeyCode::Char(' ')), today());
        assert_eq!(form.input.days, DaySet::MON_THU);
        assert_eq!(form.input.repeat, RepeatChoice::MonThu);
        // Turning Saturday on is no preset: "pick days".
        form.handle_key(key(KeyCode::Right), today());
        form.handle_key(key(KeyCode::Char(' ')), today());
        assert_eq!(form.input.repeat, RepeatChoice::PickDays);
    }

    #[test]
    fn day_sets_validate_and_fill_a_missing_date() {
        let mut form = task_form();
        form.input.repeat = RepeatChoice::PickDays;
        form.input.days = DaySet::EMPTY;
        assert!(form.input.validate(today()).is_err(), "no days picked");
        form.input.days = DaySet::ALL;
        assert_eq!(
            form.input.validate(today()).unwrap().repeat,
            RepeatRule::Daily
        );
        // Empty date: filled with the first scheduled day from today. Today (Thu Oct 8) is
        // one of Mon–Thu, so it's today; for Mondays only, it's the next Monday.
        form.input.days = DaySet::MON_THU;
        let data = form.input.validate(today()).unwrap();
        assert_eq!(data.date, Some(today()));
        form.input.days = DaySet::EMPTY.toggled(Weekday::Mon);
        let data = form.input.validate(today()).unwrap();
        assert_eq!(data.date, NaiveDate::from_ymd_opt(2026, 10, 12));
        // Events may repeat on weekdays too, and get a date filled in the same way.
        form.input.kind = Kind::Event;
        assert_eq!(
            form.input.validate(today()).unwrap().date,
            NaiveDate::from_ymd_opt(2026, 10, 12)
        );
    }

    #[test]
    fn editing_a_weekday_rule_restores_the_preset_or_pick_days() {
        let mut item = Item::new(Kind::Task, "x", chrono::Utc::now());
        item.date = NaiveDate::from_ymd_opt(2026, 10, 12);
        item.repeat = RepeatRule::Weekdays(DaySet::MON_THU);
        let form = Form::edit(&item, FormPrefs::default());
        assert_eq!(form.input.repeat, RepeatChoice::MonThu);
        let odd = DaySet::EMPTY.toggled(Weekday::Mon).toggled(Weekday::Sat);
        item.repeat = RepeatRule::Weekdays(odd);
        let form = Form::edit(&item, FormPrefs::default());
        assert_eq!(form.input.repeat, RepeatChoice::PickDays);
        assert_eq!(form.input.validate(today()).unwrap().repeat, item.repeat);
    }

    /// Renders the dialog at each test width and checks that every expected text is complete.
    fn assert_dialog_shows(form: &Form, expected: &[&str]) {
        use crate::ui::test_support::{WIDTHS, assert_shows, screen_text};
        use zerohour_core::theme::THEMES;
        for width in WIDTHS {
            let screen = screen_text(width, 50, |frame| {
                draw(frame, form, &THEMES[0], IconMode::Unicode, today());
            });
            assert_shows(&screen, width, expected);
        }
    }

    #[test]
    fn choose_kind_step_is_never_cut_off() {
        assert_dialog_shows(
            &Form::new_add(FormPrefs::default()),
            &[
                "Something to do, optionally with a due date.",
                "A date to count down to, like a birthday or holiday.",
                "Tab switch · Enter continue · t/e pick · Esc cancel",
            ],
        );
    }

    #[test]
    fn task_form_labels_previews_and_hints_are_never_cut_off() {
        let mut form = task_form();
        assert_dialog_shows(
            &form,
            &[
                "Title",
                "Priority",
                "Due date",
                "Due time",
                "Repeat",
                "Notes",
                "e.g. 12/25, 2026-12-25, +3d, +2w, tomorrow, fri",
                "e.g. 2pm, 2:30 pm, 14:30 (empty = any time that day)",
                "Tab/↑↓ move · ←→ change · Enter save · Esc cancel",
            ],
        );
        focus_on(&mut form, Field::Date);
        assert_dialog_shows(
            &form,
            &["Tab/↑↓ move · Ctrl+D/F2 calendar · Enter save · Esc cancel"],
        );
        // A long error message.
        form.error = parse_date_input("not a date at all", today()).err();
        let message = form.error.clone().unwrap();
        assert_dialog_shows(&form, &[message.as_str()]);
    }

    #[test]
    fn repeat_and_weekday_rows_are_never_cut_off() {
        let mut form = task_form();
        form.input.repeat = RepeatChoice::PickDays;
        form.input.days = DaySet::MON_THU;
        form.input.date_text = "sat".into();
        focus_on(&mut form, Field::Days);
        assert_dialog_shows(
            &form,
            &[
                "Days",
                "‹ ›Su [x]Mo [x]Tu [x]We [x]Th [ ]Fr [ ]Sa",
                "→ Sat isn't a repeat day: saves as Mon Oct 12",
                "↑↓ move · ←→ day · Space on/off · Enter save · Esc cancel",
            ],
        );
        form.input.repeat = RepeatChoice::Custom;
        assert_dialog_shows(&form, &["Every", "Unit"]);
    }

    #[test]
    fn event_form_is_never_cut_off() {
        let mut form = task_form();
        form.input.kind = Kind::Event;
        focus_on(&mut form, Field::KeepAfterPass);
        assert_dialog_shows(
            &form,
            &["Keep after", "[ ] show after the date passes", "Date"],
        );
    }

    #[test]
    fn calendar_over_the_form_is_never_cut_off() {
        let mut form = task_form();
        focus_on(&mut form, Field::Date);
        form.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE), today());
        assert_dialog_shows(
            &form,
            &[
                "October 2026",
                "Su Mo Tu We Th Fr Sa",
                "←→↑↓/hjkl day · PgUp/Dn month",
                "t today · w week: Su · ⏎ pick",
            ],
        );
    }

    #[test]
    fn edit_prefills_and_round_trips_repeat() {
        let mut item = Item::new(Kind::Event, "Xmas", chrono::Utc::now());
        item.date = NaiveDate::from_ymd_opt(2026, 12, 25);
        item.repeat = RepeatRule::EveryN {
            n: 2,
            unit: Unit::Months,
        };
        let form = Form::edit(&item, FormPrefs::default());
        assert_eq!(form.step, Step::Fields);
        let data = form.input.validate(today()).unwrap();
        assert_eq!(data.repeat, item.repeat);
        assert_eq!(data.date, item.date);
        assert_eq!(data.editing.as_deref(), Some(item.id.as_str()));
    }
}
