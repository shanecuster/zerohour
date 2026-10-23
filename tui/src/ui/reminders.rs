//! The Reminders popup (`n`): this device's reminder settings, saved the moment they change,
//! and a button that sends a test notification.
//!
//! The settings are core's [`ReminderSettings`] (the four `reminders.*` keys). They live in
//! the database's `settings` table, which is never synced: switching reminders on here only
//! affects this computer; the phone has its own switch. On the desktop they are shown by
//! `zerohour remind`, which a systemd user timer runs every minute
//! (`zerohour remind --print-units`).
//!
//! Like the sync popup, this file owns the popup's state ([`RemindersForm`]), its keys
//! ([`RemindersForm::handle_key`]) and its drawing ([`draw`]). The app only sees a
//! [`RemindersOutcome`] and does the actual work (saving, sending the test).

use chrono::{NaiveTime, TimeDelta};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use zerohour_core::clock::{TimeFormat, format_time, parse_time_input};
use zerohour_core::icons::{Icon, IconMode};
use zerohour_core::reminders::ReminderSettings;
use zerohour_core::theme::Theme;

use unicode_width::UnicodeWidthStr;

use super::{
    centered, color, popup_block, popup_height, popup_text_width, wrap_text, wrapped_lines,
};

/// The lead times `←`/`→` step through, in minutes. 0 = no earlier reminder.
const LEAD_CHOICES: [u32; 8] = [0, 5, 10, 15, 30, 60, 120, 1440];

/// How far `←`/`→` move the day time.
const DAY_TIME_STEP_MINUTES: i64 = 15;

/// One row of the popup, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Enabled,
    Lead,
    DayTime,
    Digest,
    Test,
}

impl Row {
    /// Every row in order (focus moves through this list).
    const ALL: [Row; 5] = [
        Row::Enabled,
        Row::Lead,
        Row::DayTime,
        Row::Digest,
        Row::Test,
    ];

    /// The label on the left, or the button text.
    fn label(self) -> &'static str {
        match self {
            Row::Enabled => "Reminders",
            Row::Lead => "Early",
            Row::DayTime => "Day time",
            Row::Digest => "Overdue",
            Row::Test => "Send a test notification",
        }
    }
}

/// What the app should do after a key press in the popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemindersOutcome {
    /// Nothing; keep the popup open.
    Continue,
    /// Close the popup (everything is already saved).
    Close,
    /// A setting changed: save these right away.
    Save(ReminderSettings),
    /// Send a test notification.
    Test,
}

/// The state of the Reminders popup.
#[derive(Debug, Clone)]
pub struct RemindersForm {
    /// The settings as currently shown (and, after each change, saved).
    pub settings: ReminderSettings,
    /// Index into [`Row::ALL`].
    pub focus: usize,
    /// A day time being typed ("7:30am"), or `None` when not typing. Saved on `Enter`.
    pub typing: Option<String>,
    /// A message under the rows ("Test sent", a typing mistake...): (text, is_error).
    pub message: Option<(String, bool)>,
}

impl RemindersForm {
    /// A popup showing the saved settings.
    pub fn new(settings: ReminderSettings) -> RemindersForm {
        RemindersForm {
            settings,
            focus: 0,
            typing: None,
            message: None,
        }
    }

    /// The focused row.
    fn focused(&self) -> Row {
        Row::ALL[self.focus]
    }

    /// Handles one key press.
    pub fn handle_key(&mut self, key: KeyEvent) -> RemindersOutcome {
        // While a day time is being typed, keys edit that text.
        if self.typing.is_some() {
            return self.typing_key(key);
        }
        let row = self.focused();
        let count = Row::ALL.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return RemindersOutcome::Close,
            KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
                self.focus = (self.focus + 1) % count;
            }
            KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => {
                self.focus = (self.focus + count - 1) % count;
            }
            KeyCode::Char(' ') | KeyCode::Enter => match row {
                Row::Enabled => return self.change(|s| s.enabled = !s.enabled),
                Row::Digest => return self.change(|s| s.overdue_digest = !s.overdue_digest),
                Row::Lead => return self.step_lead(1),
                // Enter on the day time starts typing a new one.
                Row::DayTime => {
                    self.typing = Some(String::new());
                    self.message = None;
                }
                Row::Test => {
                    self.message = None;
                    return RemindersOutcome::Test;
                }
            },
            KeyCode::Right | KeyCode::Char('l') => return self.step(row, 1),
            KeyCode::Left | KeyCode::Char('h') => return self.step(row, -1),
            // Typing a digit on the day time row starts typing straight away.
            KeyCode::Char(c) if row == Row::DayTime && c.is_ascii_digit() => {
                self.typing = Some(c.to_string());
                self.message = None;
            }
            _ => {}
        }
        RemindersOutcome::Continue
    }

    /// Keys while a day time is being typed: text, `Backspace`, `Enter` to save, `Esc` to
    /// stop without changing it.
    fn typing_key(&mut self, key: KeyEvent) -> RemindersOutcome {
        // `as_mut` borrows the text inside the `Option` so it can be changed in place.
        let Some(text) = self.typing.as_mut() else {
            return RemindersOutcome::Continue;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.typing = None,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char('u') if ctrl => text.clear(),
            KeyCode::Char(c) if !ctrl => text.push(c),
            KeyCode::Enter => {
                // Core's time parser understands "9", "9am", "7:30 pm", "14:00"...
                match parse_time_input(text) {
                    Ok(Some(time)) => {
                        self.typing = None;
                        return self.change(|s| s.day_time = time);
                    }
                    // Empty: keep the old time.
                    Ok(None) => self.typing = None,
                    Err(problem) => self.message = Some((problem, true)),
                }
            }
            _ => {}
        }
        RemindersOutcome::Continue
    }

    /// `←`/`→` on a row: a switch flips, the lead time and day time step.
    fn step(&mut self, row: Row, direction: i64) -> RemindersOutcome {
        match row {
            Row::Enabled => self.change(|s| s.enabled = direction > 0),
            Row::Digest => self.change(|s| s.overdue_digest = direction > 0),
            Row::Lead => self.step_lead(direction),
            Row::DayTime => {
                let step = TimeDelta::minutes(DAY_TIME_STEP_MINUTES * direction);
                // `overflowing_add_signed` wraps around midnight and also says by how many
                // seconds it wrapped, which we don't need (`.0` keeps just the time).
                let time = self.settings.day_time.overflowing_add_signed(step).0;
                self.change(|s| s.day_time = round_to_step(time))
            }
            Row::Test => RemindersOutcome::Continue,
        }
    }

    /// Moves the lead time to the next (or previous) choice in [`LEAD_CHOICES`]. A saved value
    /// that isn't one of the choices moves to the nearest choice in that direction.
    fn step_lead(&mut self, direction: i64) -> RemindersOutcome {
        let current = self.settings.lead_minutes;
        let next = if direction > 0 {
            // The first choice bigger than now, or back to the start after the last one.
            LEAD_CHOICES
                .iter()
                .copied()
                .find(|&m| m > current)
                .unwrap_or(LEAD_CHOICES[0])
        } else {
            // The last choice smaller than now, or round to the end from 0.
            LEAD_CHOICES
                .iter()
                .copied()
                .rev()
                .find(|&m| m < current)
                .unwrap_or(LEAD_CHOICES[LEAD_CHOICES.len() - 1])
        };
        self.change(|s| s.lead_minutes = next)
    }

    /// Applies a change to the settings and asks the app to save them now.
    ///
    /// `impl FnOnce(&mut ReminderSettings)` means "a closure that changes the settings, called
    /// once". Every change goes through here, so none can forget to save.
    fn change(&mut self, edit: impl FnOnce(&mut ReminderSettings)) -> RemindersOutcome {
        edit(&mut self.settings);
        self.message = None;
        RemindersOutcome::Save(self.settings)
    }
}

/// Rounds a time down to a whole [`DAY_TIME_STEP_MINUTES`] step, so stepping from a typed
/// 7:40 lands on 7:45 / 7:30 and then stays on the quarter hours.
fn round_to_step(time: NaiveTime) -> NaiveTime {
    use chrono::Timelike;
    let minutes = i64::from(time.hour() * 60 + time.minute());
    let rounded = minutes - minutes % DAY_TIME_STEP_MINUTES;
    // Always inside the day (0..1440 minutes), so `from_hms_opt` can't fail; the fallback is
    // only there to avoid an `unwrap()`.
    NaiveTime::from_hms_opt((rounded / 60) as u32, (rounded % 60) as u32, 0).unwrap_or(time)
}

/// "off", "15 min before", "1 h before", "1 day before".
fn lead_text(minutes: u32) -> String {
    match minutes {
        0 => "off (only at the due time)".to_string(),
        1440 => "1 day before".to_string(),
        m if m % 60 == 0 => format!("{} h before", m / 60),
        m => format!("{m} min before"),
    }
}

/// Draws the popup.
pub fn draw(
    frame: &mut Frame,
    form: &RemindersForm,
    theme: &Theme,
    icons: IconMode,
    time_format: TimeFormat,
) {
    let width = 66.min(frame.area().width);
    let text_width = popup_text_width(width);
    const LABEL_WIDTH: usize = 10;
    // Room for a value: the width minus the marker ("› "), the label and its space.
    let value_width = text_width.saturating_sub(LABEL_WIDTH + 3);
    let accent = Style::new()
        .fg(color(theme.accent))
        .add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(color(theme.muted));
    let fg = Style::new().fg(color(theme.fg));
    let check = |on: bool| if on { "[x] on" } else { "[ ] off" };
    let s = &form.settings;

    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in Row::ALL.iter().enumerate() {
        let focused = i == form.focus;
        let marker = Span::styled(if focused { "› " } else { "  " }, accent);
        if *row == Row::Test {
            lines.push(Line::default());
            let style = if focused { accent } else { fg };
            lines.push(Line::from(vec![
                marker,
                Span::styled(format!("[ {} ]", row.label()), style),
            ]));
            continue;
        }
        let value = match row {
            Row::Enabled => format!("{}   (this computer only)", check(s.enabled)),
            Row::Lead => format!("‹ {} ›", lead_text(s.lead_minutes)),
            Row::DayTime => match &form.typing {
                Some(text) => format!("{text}▏  (Enter save · Esc keep)"),
                None => format!(
                    "‹ {} ›   (tasks with no time)",
                    format_time(s.day_time, time_format)
                ),
            },
            Row::Digest => format!(
                "{}   (a daily list, at the day time)",
                check(s.overdue_digest)
            ),
            Row::Test => String::new(),
        };
        let label_style = if focused { accent } else { muted };
        // A value too wide for a narrow terminal wraps under the value column.
        let parts = if value.width() > value_width {
            wrap_text(&value, value_width, 0)
        } else {
            vec![value]
        };
        let mut parts = parts.into_iter();
        lines.push(Line::from(vec![
            marker,
            Span::styled(format!("{:<LABEL_WIDTH$} ", row.label()), label_style),
            Span::styled(parts.next().unwrap_or_default(), fg),
        ]));
        for part in parts {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(LABEL_WIDTH + 3)),
                Span::styled(part, fg),
            ]));
        }
    }

    lines.push(Line::default());
    lines.extend(wrapped_lines(
        "  Desktop popups come from `zerohour remind`, run every minute by a systemd timer: \
         `zerohour remind --print-units` prints it. The phone has its own settings.",
        text_width,
        2,
        muted,
    ));
    if let Some((text, is_error)) = &form.message {
        let slot = if *is_error { theme.danger } else { theme.ok };
        lines.extend(wrapped_lines(
            &format!("  {text}"),
            text_width,
            2,
            Style::new().fg(color(slot)),
        ));
    }
    lines.extend(wrapped_lines(
        "  ↑↓ move · Space on/off · ←→ change · Enter type a time · Esc close (saved as you go)",
        text_width,
        2,
        muted,
    ));

    let area = centered(frame.area(), width, popup_height(lines.len()));
    let title = format!(" {} Reminders ", Icon::Reminder.get(icons));
    let block = popup_block(&title, theme, color(theme.accent));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    /// Moves focus to `row`.
    fn focus(form: &mut RemindersForm, row: Row) {
        form.focus = Row::ALL.iter().position(|r| *r == row).unwrap();
    }

    /// The settings a key press asks to save, or panics.
    fn saved(outcome: RemindersOutcome) -> ReminderSettings {
        match outcome {
            RemindersOutcome::Save(settings) => settings,
            other => panic!("expected a save, got {other:?}"),
        }
    }

    #[test]
    fn space_switches_reminders_on_and_saves_at_once() {
        let mut form = RemindersForm::new(ReminderSettings::default());
        assert!(saved(form.handle_key(key(KeyCode::Char(' ')))).enabled);
        assert!(!saved(form.handle_key(key(KeyCode::Char(' ')))).enabled);
        focus(&mut form, Row::Digest);
        assert!(saved(form.handle_key(key(KeyCode::Enter))).overdue_digest);
    }

    #[test]
    fn the_lead_time_steps_through_the_choices_and_wraps() {
        let mut form = RemindersForm::new(ReminderSettings::default());
        focus(&mut form, Row::Lead);
        assert_eq!(saved(form.handle_key(key(KeyCode::Right))).lead_minutes, 5);
        assert_eq!(saved(form.handle_key(key(KeyCode::Left))).lead_minutes, 0);
        assert_eq!(
            saved(form.handle_key(key(KeyCode::Left))).lead_minutes,
            1440
        );
        assert_eq!(saved(form.handle_key(key(KeyCode::Right))).lead_minutes, 0);
        // A saved value that isn't a choice (20) steps to its neighbours.
        form.settings.lead_minutes = 20;
        assert_eq!(saved(form.handle_key(key(KeyCode::Right))).lead_minutes, 30);
        form.settings.lead_minutes = 20;
        assert_eq!(saved(form.handle_key(key(KeyCode::Left))).lead_minutes, 15);
    }

    #[test]
    fn the_day_time_steps_by_quarter_hours_around_midnight() {
        let mut form = RemindersForm::new(ReminderSettings::default());
        focus(&mut form, Row::DayTime);
        assert_eq!(
            saved(form.handle_key(key(KeyCode::Right))).day_time,
            t(9, 15)
        );
        form.settings.day_time = t(0, 0);
        assert_eq!(
            saved(form.handle_key(key(KeyCode::Left))).day_time,
            t(23, 45)
        );
        form.settings.day_time = t(7, 40);
        assert_eq!(
            saved(form.handle_key(key(KeyCode::Right))).day_time,
            t(7, 45)
        );
    }

    #[test]
    fn a_typed_day_time_is_saved_on_enter() {
        let mut form = RemindersForm::new(ReminderSettings::default());
        focus(&mut form, Row::DayTime);
        // A digit starts typing right away.
        for c in "7:30am".chars() {
            assert_eq!(
                form.handle_key(key(KeyCode::Char(c))),
                RemindersOutcome::Continue
            );
        }
        assert_eq!(form.typing.as_deref(), Some("7:30am"));
        assert_eq!(
            saved(form.handle_key(key(KeyCode::Enter))).day_time,
            t(7, 30)
        );
        assert_eq!(form.typing, None);
    }

    #[test]
    fn a_mistyped_day_time_is_explained_and_not_saved() {
        let mut form = RemindersForm::new(ReminderSettings::default());
        focus(&mut form, Row::DayTime);
        form.handle_key(key(KeyCode::Enter));
        for c in "25:99".chars() {
            form.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(
            form.handle_key(key(KeyCode::Enter)),
            RemindersOutcome::Continue
        );
        assert!(form.message.as_ref().is_some_and(|(_, error)| *error));
        // Esc stops typing and keeps the old time.
        form.handle_key(key(KeyCode::Esc));
        assert_eq!(form.typing, None);
        assert_eq!(form.settings.day_time, t(9, 0));
    }

    #[test]
    fn the_test_button_and_esc() {
        let mut form = RemindersForm::new(ReminderSettings::default());
        focus(&mut form, Row::Test);
        assert_eq!(form.handle_key(key(KeyCode::Enter)), RemindersOutcome::Test);
        assert_eq!(form.handle_key(key(KeyCode::Esc)), RemindersOutcome::Close);
    }

    #[test]
    fn nothing_in_the_popup_is_cut_off_at_any_width() {
        use crate::ui::test_support::{WIDTHS, assert_shows, screen_text};
        use zerohour_core::theme::THEMES;

        let mut form = RemindersForm::new(ReminderSettings {
            enabled: true,
            lead_minutes: 15,
            ..ReminderSettings::default()
        });
        form.message = Some((
            "Test sent. If nothing appeared, is a notification daemon running?".into(),
            false,
        ));
        let expected = [
            "Reminders [x] on (this computer only)",
            "Early ‹ 15 min before ›",
            "Day time ‹ 9:00 AM › (tasks with no time)",
            "Overdue [ ] off (a daily list, at the day time)",
            "[ Send a test notification ]",
            "Desktop popups come from `zerohour remind`, run every minute by a systemd timer: \
             `zerohour remind --print-units` prints it. The phone has its own settings.",
            "Test sent. If nothing appeared, is a notification daemon running?",
            "↑↓ move · Space on/off · ←→ change · Enter type a time · Esc close (saved as you go)",
        ];
        for width in WIDTHS {
            let screen = screen_text(width, 40, |frame| {
                draw(
                    frame,
                    &form,
                    &THEMES[0],
                    IconMode::Unicode,
                    TimeFormat::Hour12,
                );
            });
            assert_shows(&screen, width, &expected);
        }
    }
}
