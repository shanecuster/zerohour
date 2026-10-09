//! The month-grid date picker that opens over the add/edit dialog's date field
//! (Ctrl+D or F2).
//!
//! ```text
//! ╭ October 2026 ───────────────╮
//! │  Su Mo Tu We Th Fr Sa       │
//! │  27 28 29 30  1  2  3       │
//! │   4  5  6  7  8 [9] 10      │
//! │  ...                        │
//! │  ←→↑↓/hjkl day · PgUp/PgDn  │
//! ╰─────────────────────────────╯
//! ```
//!
//! This file has the picker's state ([`Calendar`]), its keys ([`Calendar::handle_key`]) and
//! its drawing ([`draw`]). The date math (which date is in which cell, moving by months) is in
//! `zerohour_core::calendar`, so the Android app can reuse it. Typing a date into the field
//! still works; the picker is just another way to fill it.

use chrono::{Datelike, NaiveDate};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use zerohour_core::calendar::{
    WeekStart, add_days, add_months, month_grid, month_title, same_month,
};
use zerohour_core::theme::Theme;

use super::{centered, color, popup_block};

/// Popup width: the longest hint line (29) + 1 column of padding on each side + 2 borders.
const WIDTH: u16 = 33;
/// Popup height: 2 borders + the weekday header + 6 weeks + 2 hint lines.
const HEIGHT: u16 = 11;

/// The picker's state while it's open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calendar {
    /// The highlighted day; moving it past the month's edge changes the month shown.
    pub cursor: NaiveDate,
    /// The date already in the field, if any (marked so you can see it).
    pub current: Option<NaiveDate>,
    /// Which day the grid's weeks start on.
    pub week_start: WeekStart,
}

/// What a key press in the picker means for the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarOutcome {
    /// Keep the picker open.
    Continue,
    /// Close without changing the date.
    Cancel,
    /// Put this date in the field and close.
    Pick(NaiveDate),
    /// The user switched the first day of the week (to be saved in settings).
    WeekStartChanged(WeekStart),
}

impl Calendar {
    /// Opens on the field's current date, or on today if the field is empty or unreadable.
    pub fn new(current: Option<NaiveDate>, today: NaiveDate, week_start: WeekStart) -> Calendar {
        Calendar {
            cursor: current.unwrap_or(today),
            current,
            week_start,
        }
    }

    /// Handles one key. `today` is needed for the `t` (jump to today) key.
    pub fn handle_key(&mut self, key: KeyEvent, today: NaiveDate) -> CalendarOutcome {
        match key.code {
            // Days and weeks. Moving off the shown month simply shows the new month.
            KeyCode::Left | KeyCode::Char('h') => self.cursor = add_days(self.cursor, -1),
            KeyCode::Right | KeyCode::Char('l') => self.cursor = add_days(self.cursor, 1),
            KeyCode::Up | KeyCode::Char('k') => self.cursor = add_days(self.cursor, -7),
            KeyCode::Down | KeyCode::Char('j') => self.cursor = add_days(self.cursor, 7),
            // Whole months (the day is clamped: Jan 31 → Feb 28).
            KeyCode::PageUp => self.cursor = add_months(self.cursor, -1),
            KeyCode::PageDown => self.cursor = add_months(self.cursor, 1),
            KeyCode::Char('t') => self.cursor = today,
            KeyCode::Char('w') => {
                self.week_start = self.week_start.toggled();
                return CalendarOutcome::WeekStartChanged(self.week_start);
            }
            KeyCode::Enter => return CalendarOutcome::Pick(self.cursor),
            KeyCode::Esc | KeyCode::Char('q') => return CalendarOutcome::Cancel,
            _ => {}
        }
        CalendarOutcome::Continue
    }
}

/// Draws the picker centered on the screen.
pub fn draw(frame: &mut Frame, calendar: &Calendar, theme: &Theme, today: NaiveDate) {
    let area = centered(frame.area(), WIDTH, HEIGHT);
    let block = popup_block(
        &format!(" {} ", month_title(calendar.cursor)),
        theme,
        color(theme.accent),
    );
    let muted = Style::new().fg(color(theme.muted));

    let mut lines: Vec<Line> = Vec::new();
    // Column headers: "Su Mo Tu ..." (each padded to the cell width).
    let header: Vec<Span> = calendar
        .week_start
        .day_labels()
        .iter()
        .map(|label| Span::styled(format!("{label} "), muted))
        .collect();
    lines.push(Line::from(header));

    for week in month_grid(calendar.cursor, calendar.week_start) {
        let cells: Vec<Span> = week
            .iter()
            .map(|&date| {
                Span::styled(
                    format!("{:>2} ", date.day()),
                    day_style(date, calendar, theme, today),
                )
            })
            .collect();
        lines.push(Line::from(cells));
    }
    lines.push(Line::from(Span::styled(
        "←→↑↓/hjkl day · PgUp/Dn month",
        muted,
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "t today · w week: {} · ⏎ pick",
            calendar.week_start.day_labels()[0]
        ),
        muted,
    )));

    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// How one day cell looks. Several things can apply at once; they're layered in order.
fn day_style(date: NaiveDate, calendar: &Calendar, theme: &Theme, today: NaiveDate) -> Style {
    // Base: days of other months and past days are dimmed.
    let mut style = if !same_month(date, calendar.cursor) || date < today {
        Style::new().fg(color(theme.muted))
    } else {
        Style::new().fg(color(theme.fg))
    };
    // The date already in the field.
    if Some(date) == calendar.current {
        style = style.fg(color(theme.ok)).add_modifier(Modifier::BOLD);
    }
    // Today is underlined and in the accent color.
    if date == today {
        style = style
            .fg(color(theme.accent))
            .add_modifier(Modifier::UNDERLINED);
    }
    // The cursor: drawn like the selected row in the lists.
    if date == calendar.cursor {
        style = style
            .bg(color(theme.selection))
            .add_modifier(Modifier::BOLD);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn press(calendar: &mut Calendar, code: KeyCode) -> CalendarOutcome {
        calendar.handle_key(KeyEvent::new(code, KeyModifiers::NONE), d(2026, 10, 9))
    }

    #[test]
    fn opens_on_the_field_date_or_today() {
        let today = d(2026, 10, 9);
        let empty = Calendar::new(None, today, WeekStart::Sunday);
        assert_eq!(empty.cursor, today);
        let filled = Calendar::new(Some(d(2026, 12, 25)), today, WeekStart::Sunday);
        assert_eq!(filled.cursor, d(2026, 12, 25));
    }

    #[test]
    fn arrows_and_vim_keys_move_by_day_and_week() {
        let mut cal = Calendar::new(Some(d(2026, 10, 31)), d(2026, 10, 9), WeekStart::Sunday);
        press(&mut cal, KeyCode::Right);
        assert_eq!(cal.cursor, d(2026, 11, 1), "crosses into the next month");
        press(&mut cal, KeyCode::Char('h'));
        press(&mut cal, KeyCode::Char('k'));
        assert_eq!(cal.cursor, d(2026, 10, 24));
        press(&mut cal, KeyCode::Down);
        press(&mut cal, KeyCode::Char('j'));
        assert_eq!(cal.cursor, d(2026, 11, 7));
        press(&mut cal, KeyCode::Up);
        press(&mut cal, KeyCode::Left);
        press(&mut cal, KeyCode::Char('l'));
        assert_eq!(cal.cursor, d(2026, 10, 31));
    }

    #[test]
    fn page_keys_move_months_and_t_jumps_to_today() {
        let mut cal = Calendar::new(Some(d(2026, 1, 31)), d(2026, 10, 9), WeekStart::Sunday);
        press(&mut cal, KeyCode::PageDown);
        assert_eq!(cal.cursor, d(2026, 2, 28));
        press(&mut cal, KeyCode::PageUp);
        assert_eq!(cal.cursor, d(2026, 1, 28));
        press(&mut cal, KeyCode::Char('t'));
        assert_eq!(cal.cursor, d(2026, 10, 9));
    }

    #[test]
    fn enter_picks_esc_cancels_w_switches_week_start() {
        let mut cal = Calendar::new(None, d(2026, 10, 9), WeekStart::Sunday);
        press(&mut cal, KeyCode::Right);
        assert_eq!(
            press(&mut cal, KeyCode::Enter),
            CalendarOutcome::Pick(d(2026, 10, 10))
        );
        assert_eq!(press(&mut cal, KeyCode::Esc), CalendarOutcome::Cancel);
        assert_eq!(
            press(&mut cal, KeyCode::Char('w')),
            CalendarOutcome::WeekStartChanged(WeekStart::Monday)
        );
        assert_eq!(cal.week_start, WeekStart::Monday);
    }

    #[test]
    fn draws_at_any_size() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use zerohour_core::theme::THEMES;

        let cal = Calendar::new(Some(d(2026, 10, 9)), d(2026, 10, 9), WeekStart::Sunday);
        for (width, height) in [(80, 24), (40, 14), (6, 3)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    draw(frame, &cal, &THEMES[0], d(2026, 10, 9));
                })
                .unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            if width == 80 {
                assert!(screen.contains("October 2026"));
                assert!(screen.contains("Su Mo Tu We Th Fr Sa"));
            }
        }
    }
}
