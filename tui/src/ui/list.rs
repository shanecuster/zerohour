//! The main screen: Tasks on top, Events underneath, a key-hint footer at the bottom.
//!
//! ```text
//! ╭─ zerohour ──────────────────────────────── Thu Oct 08 2026 ─╮
//! │╭  Tasks (2) ────────────────────────────────────────────────╮│
//! ││ ☐ ▲ Renew car registration          due in 3d     High    ││
//! │╰────────────────────────────────────────────────────────────╯│
//! │╭  Events (2) ───────────────────────────────────────────────╮│
//! ││ ◷ Christmas                         78 days    ↻ yearly    ││
//! │╰────────────────────────────────────────────────────────────╯│
//! │ a add  space done  e edit  d delete  Tab stats  ? help       │
//! ╰──────────────────────────────────────────────────────────────╯
//! ```
//!
//! Tasks and events each get their own color (theme `task` / `event` slots) AND their own
//! marker icon, so color is never the only way to tell them apart.
//!
//! **Layout** (`v` toggles, saved in settings): *split* gives Tasks and Events equal halves of
//! the height (the default); *fit* sizes the Events box to its rows and gives Tasks the rest.
//!
//! **Narrow mode** (under [`NARROW_WIDTH`] columns): the priority text, repeat label and date
//! columns are dropped, countdowns get shorter ("3d"), and long titles end in `…`.

use chrono::{Datelike, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Padding, Paragraph, Row, Table};
use zerohour_core::icons::{Icon, IconMode, priority_icon};
use zerohour_core::model::{Item, Priority};
use zerohour_core::repeat::{days_until, is_overdue};
use zerohour_core::theme::{Rgb, Theme};

use super::{NARROW_WIDTH, color, screen_block, truncate, width_u16};
use crate::app::{App, EventRow, LayoutMode};

/// From this width up, an extra column shows the actual date ("Fri Dec 25").
const DATE_COLUMN_WIDTH: u16 = 90;
/// Width of the date column: "Thu Oct 08" (this year) or "Oct 07 2027" (other years).
const DATE_WIDTH: u16 = 11;
/// Ratatui's default gap between table columns.
const COLUMN_SPACING: u16 = 1;

/// Draws the main screen into `area`.
pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let subtitle = if app.demo { " · demo" } else { "" };
    let outer = screen_block(app, subtitle);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let narrow = area.width < NARROW_WIDTH;
    let show_dates = area.width >= DATE_COLUMN_WIDTH;

    // One line for notes of the selected item (if any), one line for the key hints; the two
    // section boxes share what's left according to the layout mode.
    let notes = app
        .selected_item()
        .and_then(|item| item.notes.clone())
        .filter(|n| !n.is_empty());
    let notes_height = u16::from(notes.is_some());
    let sections_height = inner.height.saturating_sub(notes_height + 1);
    let events_height = match app.layout {
        LayoutMode::Split => split_heights(sections_height).1,
        // Fit: the Events box is sized to its rows (+2 for borders), but never more than half
        // the space; Tasks get everything else. (This is the original phase 1 layout.)
        LayoutMode::Fit => {
            let event_rows = app.events.len().max(1) as u16;
            (event_rows + 2).clamp(3, (inner.height / 2).max(3))
        }
    };

    // `Layout::vertical(...).areas(rect)` splits a rectangle into rows. `Fill(1)` takes
    // whatever space is left after the fixed `Length`s.
    let [tasks_area, events_area, notes_area, footer_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(events_height),
        Constraint::Length(notes_height),
        Constraint::Length(1),
    ])
    .areas(inner);

    draw_tasks(frame, tasks_area, app, narrow, show_dates);
    draw_events(frame, events_area, app, narrow, show_dates);
    if let Some(notes) = notes {
        let text = truncate(&format!(" notes: {notes}"), notes_area.width as usize);
        frame.render_widget(
            Paragraph::new(Span::styled(
                text,
                Style::new()
                    .fg(color(app.theme.muted))
                    .add_modifier(Modifier::ITALIC),
            )),
            notes_area,
        );
    }
    draw_footer(frame, footer_area, app.theme, narrow);
}

/// Split layout: divides `height` lines into (tasks, events) halves. An odd line goes to Tasks.
///
/// Equal halves also give each box its minimum height whenever the screen can fit both: with
/// at least 10 lines, each box gets at least 5 (3 rows plus its borders). On a smaller
/// terminal the two still share equally, so neither disappears.
fn split_heights(height: u16) -> (u16, u16) {
    let events = height / 2;
    (height - events, events)
}

/// The bordered box around one section, titled with its marker, name and item count.
fn section_block<'a>(
    name: &'a str,
    count: usize,
    icon: &'a str,
    section: Color,
    theme: &Theme,
) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(section))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(icon, Style::new().fg(section)),
            Span::styled(
                format!(" {name} "),
                Style::new().fg(section).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("({count}) "), Style::new().fg(color(theme.muted))),
        ]))
        // One column of breathing room between the border and the rows.
        .padding(Padding::horizontal(1))
}

/// The Tasks section.
fn draw_tasks(frame: &mut Frame, area: Rect, app: &mut App, narrow: bool, show_dates: bool) {
    let theme = app.theme;
    let mode = app.icon_mode;
    let today = app.today;
    let block = section_block(
        "Tasks",
        app.tasks.len(),
        Icon::Task.get(mode),
        color(theme.task),
        theme,
    );

    if app.tasks.is_empty() {
        let hint = Paragraph::new(Span::styled(
            " Nothing to do. Press a to add a task.",
            Style::new().fg(color(theme.muted)),
        ))
        .block(block);
        frame.render_widget(hint, area);
        return;
    }

    // Fixed column widths. Icons can be 1 cell (Nerd/Unicode) or 3 (ASCII "[ ]").
    let marker_w = width_u16(Icon::Task.get(mode));
    let prio_w = Priority::ALL
        .iter()
        .map(|&p| width_u16(priority_icon(p).get(mode)))
        .max()
        .unwrap_or(1);
    let due_w: u16 = if narrow { 5 } else { 13 };
    let mut fixed = vec![marker_w, prio_w];
    if show_dates {
        fixed.push(DATE_WIDTH);
    }
    fixed.push(due_w);
    if !narrow {
        fixed.push(6); // "Medium"
    }
    let title_w = title_width(area, &fixed);

    // Build one table row per task. `.iter().map(...).collect()` turns tasks into rows.
    let rows: Vec<Row> = app
        .tasks
        .iter()
        .map(|task| {
            let overdue = is_overdue(task, today);
            let mut title = task.title.clone();
            if task.repeat.is_repeating() {
                title.push(' ');
                title.push_str(Icon::Repeat.get(mode));
            }
            let title_style = if overdue {
                Style::new().fg(color(theme.danger))
            } else {
                Style::new().fg(color(theme.fg))
            };
            let prio_style = Style::new().fg(priority_color(task.priority, theme));
            let (due_text, due_slot) = due_label(task, today, narrow, mode, theme);

            let mut cells = vec![
                Cell::from(Span::styled(
                    Icon::Task.get(mode),
                    Style::new().fg(color(theme.task)),
                )),
                Cell::from(Span::styled(
                    priority_icon(task.priority).get(mode),
                    prio_style,
                )),
                Cell::from(Span::styled(truncate(&title, title_w), title_style)),
            ];
            if show_dates {
                cells.push(date_cell(task.date, today, theme));
            }
            cells.push(Cell::from(Span::styled(
                due_text,
                Style::new().fg(color(due_slot)),
            )));
            if !narrow {
                cells.push(Cell::from(Span::styled(task.priority.label(), prio_style)));
            }
            Row::new(cells)
        })
        .collect();

    // Select a row in this table only if the selection is inside the Tasks section.
    let selected = (app.selected < app.tasks.len()).then_some(app.selected);
    app.task_table.select(selected);
    let table = Table::new(rows, constraints(&fixed, 2))
        .block(block)
        .column_spacing(COLUMN_SPACING)
        .row_highlight_style(selection_style(theme));
    frame.render_stateful_widget(table, area, &mut app.task_table);
}

/// The Events section.
fn draw_events(frame: &mut Frame, area: Rect, app: &mut App, narrow: bool, show_dates: bool) {
    let theme = app.theme;
    let mode = app.icon_mode;
    let block = section_block(
        "Events",
        app.events.len(),
        Icon::Event.get(mode),
        color(theme.event),
        theme,
    );

    if app.events.is_empty() {
        let hint = Paragraph::new(Span::styled(
            " No countdowns yet. Press a, then Tab to pick Event.",
            Style::new().fg(color(theme.muted)),
        ))
        .block(block);
        frame.render_widget(hint, area);
        return;
    }

    let marker_w = width_u16(Icon::Event.get(mode));
    let mut fixed = vec![marker_w];
    if show_dates {
        fixed.push(DATE_WIDTH);
    }
    fixed.push(if narrow { 5 } else { 13 });
    if !narrow {
        fixed.push(17); // "↻ every 12 weeks"
    }
    let title_w = title_width(area, &fixed);

    let rows: Vec<Row> = app
        .events
        .iter()
        .map(|row: &EventRow| {
            let (count_text, count_style) = countdown_label(row.days, narrow, theme);
            let mut cells = vec![
                Cell::from(Span::styled(
                    Icon::Event.get(mode),
                    Style::new().fg(color(theme.event)),
                )),
                Cell::from(Span::styled(
                    truncate(&row.item.title, title_w),
                    Style::new().fg(color(theme.fg)),
                )),
            ];
            if show_dates {
                cells.push(date_cell(row.target, app.today, theme));
            }
            cells.push(Cell::from(Span::styled(count_text, count_style)));
            if !narrow {
                let repeat = if row.item.repeat.is_repeating() {
                    format!("{} {}", Icon::Repeat.get(mode), row.item.repeat.label())
                } else {
                    String::new()
                };
                cells.push(Cell::from(Span::styled(
                    repeat,
                    Style::new().fg(color(theme.muted)),
                )));
            }
            Row::new(cells)
        })
        .collect();

    let selected = app.selected.checked_sub(app.tasks.len());
    app.event_table.select(selected);
    let table = Table::new(rows, constraints(&fixed, 1))
        .block(block)
        .column_spacing(COLUMN_SPACING)
        .row_highlight_style(selection_style(theme));
    frame.render_stateful_widget(table, area, &mut app.event_table);
}

/// Column constraints: the fixed widths, with a flexible title column inserted at
/// `title_index` that takes the remaining space.
fn constraints(fixed: &[u16], title_index: usize) -> Vec<Constraint> {
    let mut out: Vec<Constraint> = fixed.iter().map(|&w| Constraint::Length(w)).collect();
    out.insert(title_index, Constraint::Fill(1));
    out
}

/// How many columns the title gets: the box's inner width minus the fixed columns and the
/// gaps between all columns. We need the number to know where to put the `…`.
fn title_width(area: Rect, fixed: &[u16]) -> usize {
    let inner = area.width.saturating_sub(4); // left + right border and padding
    let gaps = COLUMN_SPACING * fixed.len() as u16; // fixed columns + title = len + 1 columns
    inner.saturating_sub(fixed.iter().sum::<u16>() + gaps) as usize
}

/// The background/bold style of the selected row.
fn selection_style(theme: &Theme) -> Style {
    Style::new()
        .bg(color(theme.selection))
        .add_modifier(Modifier::BOLD)
}

/// Color for a priority: High = danger, Medium = warn, Low = muted.
fn priority_color(priority: Priority, theme: &Theme) -> Color {
    color(match priority {
        Priority::High => theme.danger,
        Priority::Medium => theme.warn,
        Priority::Low => theme.muted,
    })
}

/// A muted date cell (empty if no date).
/// Shows the weekday for dates this year, and the year instead for other years, so a rolled-
/// over event ("Oct 07 2027") can't be mistaken for this year's.
fn date_cell<'a>(date: Option<NaiveDate>, today: NaiveDate, theme: &Theme) -> Cell<'a> {
    let text = date
        .map(|d| {
            let format = if d.year() == today.year() {
                "%a %b %d"
            } else {
                "%b %d %Y"
            };
            d.format(format).to_string()
        })
        .unwrap_or_default();
    Cell::from(Span::styled(text, Style::new().fg(color(theme.muted))))
}

/// The "due in 3d" text for a task, and which theme color to draw it in.
///
/// Returns a tuple `(String, Rgb)`; the caller unpacks it with `let (text, slot) = ...`.
fn due_label(
    task: &Item,
    today: NaiveDate,
    narrow: bool,
    mode: IconMode,
    theme: &Theme,
) -> (String, Rgb) {
    let Some(due) = task.date else {
        return (String::new(), theme.muted);
    };
    let days = days_until(due, today);
    let overdue = Icon::Overdue.get(mode);
    // Match on (days, narrow) together; guards (`if d > 0`) refine an arm further.
    let text = match (days, narrow) {
        (0, false) => "due today".to_string(),
        (0, true) => "today".to_string(),
        (1, false) => "due tomorrow".to_string(),
        (1, true) => "tmrw".to_string(),
        (d, false) if d > 0 => format!("due in {d}d"),
        (d, true) if d > 0 => format!("{d}d"),
        (d, false) => format!("{overdue} {}d overdue", -d),
        (d, true) => format!("{overdue}{}d", -d),
    };
    let slot = match days {
        d if d < 0 => theme.danger,
        0 | 1 => theme.warn,
        _ => theme.muted,
    };
    (text, slot)
}

/// The "78 days" text for an event, and its style.
fn countdown_label(days: Option<i64>, narrow: bool, theme: &Theme) -> (String, Style) {
    let Some(days) = days else {
        return ("no date".to_string(), Style::new().fg(color(theme.danger)));
    };
    let text = match (days, narrow) {
        (0, _) => "today!".to_string(),
        (1, false) => "tomorrow".to_string(),
        (d, false) if d > 0 => format!("{d} days"),
        (d, true) if d > 0 => format!("{d}d"),
        (d, false) => format!("{}d ago", -d),
        (d, true) => format!("-{}d", -d),
    };
    let style = match days {
        0 => Style::new()
            .fg(color(theme.ok))
            .add_modifier(Modifier::BOLD),
        1..=7 => Style::new().fg(color(theme.warn)),
        d if d > 7 => Style::new().fg(color(theme.event)),
        _ => Style::new().fg(color(theme.muted)),
    };
    (text, style)
}

/// The key-hint line at the bottom: keys in the accent color, words muted.
fn draw_footer(frame: &mut Frame, area: Rect, theme: &Theme, narrow: bool) {
    let keys: &[(&str, &str)] = if narrow {
        &[
            ("a", "add"),
            ("␣", "done"),
            ("e", "edit"),
            ("d", "del"),
            ("?", "help"),
        ]
    } else {
        &[
            ("a", "add"),
            ("space", "done"),
            ("e", "edit"),
            ("d", "delete"),
            ("Tab", "stats"),
            ("t", "theme"),
            ("?", "help"),
            ("q", "quit"),
        ]
    };
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in keys {
        spans.push(Span::styled(
            *key,
            Style::new()
                .fg(color(theme.accent))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {label}  "),
            Style::new().fg(color(theme.muted)),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest useful section box: 3 rows plus the top and bottom borders.
    const MIN_SECTION_HEIGHT: u16 = 5;

    #[test]
    fn split_gives_equal_halves_with_the_odd_line_to_tasks() {
        assert_eq!(split_heights(20), (10, 10));
        assert_eq!(split_heights(21), (11, 10));
        assert_eq!(split_heights(0), (0, 0));
    }

    #[test]
    fn split_meets_the_minimum_whenever_both_fit() {
        for height in 2 * MIN_SECTION_HEIGHT..200 {
            let (tasks, events) = split_heights(height);
            assert!(tasks >= MIN_SECTION_HEIGHT && events >= MIN_SECTION_HEIGHT);
            assert_eq!(tasks + events, height);
        }
    }

    #[test]
    fn layout_setting_defaults_to_split() {
        assert_eq!(LayoutMode::parse(None), LayoutMode::Split);
        assert_eq!(LayoutMode::parse(Some("garbage")), LayoutMode::Split);
        assert_eq!(LayoutMode::parse(Some("fit")), LayoutMode::Fit);
        for mode in [LayoutMode::Split, LayoutMode::Fit] {
            assert_eq!(LayoutMode::parse(Some(mode.as_str())), mode);
            assert_eq!(mode.toggled().toggled(), mode);
        }
    }
}
