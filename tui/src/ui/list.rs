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
//! **Banner**: on tall screens (30+ rows, not narrow), a big "zerohour" title and a
//! next-event tagline sit above the Tasks box (see `banner.rs`); the boxes share what's left.
//! `b` switches it off and on (saved in settings).
//!
//! **Layout** (`v` toggles, saved in settings): *split* gives Tasks and Events equal halves of
//! the height (the default); *fit* sizes the Events box to its rows and gives Tasks the rest.
//!
//! **Narrow mode** (under [`NARROW_WIDTH`] columns): the priority text, repeat label and date
//! columns are dropped, countdowns get shorter ("3d"), and long titles end in `…`.

use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Padding, Paragraph, Row, Table};
use zerohour_core::icons::{Icon, priority_icon};
use zerohour_core::model::Priority;
use zerohour_core::theme::Theme;
use zerohour_core::view::{self, EventRow, Label};

use super::{NARROW_WIDTH, banner, color, screen_block, truncate, width_u16};
use crate::app::{App, LayoutMode};

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
    // The banner takes its rows first, when it's switched on and the screen has room.
    let banner_height = if app.show_banner && banner::fits(area.width, area.height, app.icon_mode) {
        banner::height(app.icon_mode)
    } else {
        0
    };
    let sections_height = inner
        .height
        .saturating_sub(banner_height + notes_height + 1);
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
    let [
        banner_area,
        tasks_area,
        events_area,
        notes_area,
        footer_area,
    ] = Layout::vertical([
        Constraint::Length(banner_height),
        Constraint::Fill(1),
        Constraint::Length(events_height),
        Constraint::Length(notes_height),
        Constraint::Length(1),
    ])
    .areas(inner);

    if banner_height > 0 {
        banner::draw(frame, banner_area, app);
    }
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
    let now = app.now;
    let time_format = app.time_format;
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
    // The due column is sized from the longest due text actually shown (see
    // `due_column_width`). The texts and their color slots come from core.
    let due_labels: Vec<Label> = app
        .tasks
        .iter()
        .map(|task| view::due_label(task, now, narrow, mode, time_format))
        .collect();
    let longest = due_labels
        .iter()
        .map(|label| width_u16(&label.text))
        .max()
        .unwrap_or(0);
    let due_w = due_column_width(longest, narrow);
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
        // `.zip` walks two lists side by side, pairing each task with its due label.
        .zip(due_labels)
        .map(|(task, due)| {
            let title = view::task_title(task, mode);
            // Overdue titles turn red (core decides: `Danger`, else normal text).
            let title_style = Style::new().fg(color(theme.slot(view::task_title_slot(task, now))));
            let prio_style = Style::new().fg(priority_color(task.priority, theme));

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
            let due_style = label_style(&due, theme);
            cells.push(Cell::from(Span::styled(due.text, due_style)));
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
            let countdown = view::countdown_label(row.days, narrow);
            let count_style = label_style(&countdown, theme);
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
            cells.push(Cell::from(Span::styled(countdown.text, count_style)));
            if !narrow {
                let repeat = view::repeat_text(&row.item, mode);
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

/// Width of the Tasks section's due column, given the widest due text being shown.
///
/// - **Wide mode:** the widest text plus one blank column, and never less than the original
///   13. The extra blank keeps a long timed label ("today 12:00 PM · in 11h") from running
///   straight into the priority after it ("Medium"), which made that row look shifted.
///   A list without due times stays exactly as before: its texts are at most 12 wide.
/// - **Narrow mode:** unchanged: the widest text, at least 5.
fn due_column_width(longest: u16, narrow: bool) -> u16 {
    if narrow {
        longest.max(5)
    } else {
        (longest + 1).max(13)
    }
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

/// Color for a priority (core decides the slot: High = danger, Medium = warn, Low = muted).
fn priority_color(priority: Priority, theme: &Theme) -> Color {
    color(theme.slot(view::priority_slot(priority)))
}

/// A muted date cell (empty if no date). The text comes from core's [`view::date_text`]:
/// the weekday for dates this year, the year instead for other years.
fn date_cell<'a>(date: Option<NaiveDate>, today: NaiveDate, theme: &Theme) -> Cell<'a> {
    Cell::from(Span::styled(
        view::date_text(date, today),
        Style::new().fg(color(theme.muted)),
    ))
}

/// Turns a core [`Label`]'s slot and weight into a ratatui style in the current theme.
fn label_style(label: &Label, theme: &Theme) -> Style {
    let style = Style::new().fg(color(theme.slot(label.slot)));
    if label.bold {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
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

#[cfg(test)]
mod column_tests {
    use super::*;
    use chrono::NaiveDateTime;
    use chrono::{NaiveTime, TimeDelta};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use zerohour_core::clock::TimeFormat;
    use zerohour_core::db::{Db, settings_keys};
    use zerohour_core::icons::IconMode;
    use zerohour_core::model::{Item, Kind};

    /// "Now" for these tests: Fri Oct 9 2026, 00:30 local, so the labels are predictable.
    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 9)
            .unwrap()
            .and_hms_opt(0, 30, 0)
            .unwrap()
    }

    /// One test task: (title, due in N days, due time as (hour, minute), priority).
    type SampleTask = (&'static str, Option<i64>, Option<(u32, u32)>, Priority);

    /// An app with timed and untimed tasks covering the longest due texts:
    /// "today 12:00 PM · in 11h", "tomorrow 12:00 PM", "! overdue 23h", "in 3d · 12:00 PM",
    /// plus untimed ones ("due in 3d", "! 2d overdue") and one without a date.
    fn app_with_tasks(format: TimeFormat) -> App {
        let db = Db::open_in_memory().unwrap();
        db.set_setting(settings_keys::ICON_MODE, "unicode").unwrap();
        db.set_setting(settings_keys::TIME_FORMAT, format.as_str())
            .unwrap();
        let today = now().date();
        let tasks: [SampleTask; 7] = [
            ("noon today", Some(0), Some((12, 0)), Priority::Medium),
            ("noon tomorrow", Some(1), Some((12, 0)), Priority::Low),
            ("late last night", Some(-1), Some((1, 30)), Priority::High),
            ("later this week", Some(3), Some((12, 0)), Priority::Medium),
            ("untimed", Some(3), None, Priority::High),
            ("untimed overdue", Some(-2), None, Priority::Low),
            ("no date", None, None, Priority::Low),
        ];
        for (title, days, time, priority) in tasks {
            let mut item = Item::new(Kind::Task, title, chrono::Utc::now());
            item.date = days.map(|d| today + TimeDelta::days(d));
            item.due_time = time.map(|(h, m)| NaiveTime::from_hms_opt(h, m, 0).unwrap());
            item.priority = priority;
            db.insert_item(&item).unwrap();
        }
        let mut app = App::new(db, false, None).unwrap();
        app.show_banner = false;
        app.now = now();
        app.today = now().date();
        app.refresh().unwrap();
        app
    }

    /// Renders the main screen and returns each task row as a list of cell symbols, so an
    /// index into it is a screen column.
    fn task_rows(app: &mut App, width: u16) -> Vec<Vec<String>> {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                draw(frame, area, app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        // Row 0 is the screen border, row 1 the Tasks box title; tasks start on row 2.
        (2..2 + app.tasks.len() as u16)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    /// The screen column where `text` starts in `row`, if it's there in full.
    fn column_of(row: &[String], text: &str) -> Option<usize> {
        let line: String = row.concat();
        let byte = line.find(text)?;
        // All characters in these rows are 1 column wide, so a character count is a column.
        Some(line[..byte].chars().count())
    }

    #[test]
    fn due_and_priority_columns_line_up_at_every_width_and_format() {
        for format in [TimeFormat::Hour12, TimeFormat::Hour24] {
            let mut app = app_with_tasks(format);
            for width in [60, 80, 120] {
                let rows = task_rows(&mut app, width);
                let mut due_starts = Vec::new();
                let mut priority_starts = Vec::new();
                for (task, row) in app.tasks.iter().zip(&rows) {
                    let label = view::due_label(task, now(), false, IconMode::Unicode, format).text;
                    let priority = column_of(row, task.priority.label()).unwrap();
                    priority_starts.push(priority);
                    if label.is_empty() {
                        continue; // no date, no due text
                    }
                    let due = column_of(row, &label).unwrap_or_else(|| {
                        panic!(
                            "{format:?} at {width}: {label:?} is cut off: {}",
                            row.concat()
                        )
                    });
                    // At least one blank column between the due text and the priority, on
                    // top of the table's own one-column gap.
                    assert!(
                        due + label.chars().count() + 2 <= priority,
                        "{format:?} at {width}: {label:?} runs into the priority: {}",
                        row.concat()
                    );
                    due_starts.push(due);
                }
                let same = |starts: &[usize]| starts.windows(2).all(|w| w[0] == w[1]);
                assert!(
                    same(&due_starts),
                    "{format:?} at {width}: due {due_starts:?}"
                );
                assert!(
                    same(&priority_starts),
                    "{format:?} at {width}: priority {priority_starts:?}"
                );
            }
        }
    }

    #[test]
    fn the_longest_forms_are_the_ones_tested() {
        // Guards the test above: these exact texts must be among the labels it checks.
        let app = app_with_tasks(TimeFormat::Hour12);
        let labels: Vec<String> = app
            .tasks
            .iter()
            .map(|t| view::due_label(t, now(), false, IconMode::Unicode, TimeFormat::Hour12).text)
            .collect();
        for expected in [
            "today 12:00 PM · in 11h",
            "tomorrow 12:00 PM",
            "! overdue 23h",
            "in 3d · 12:00 PM",
            "due in 3d",
            "! 2d overdue",
        ] {
            assert!(
                labels.iter().any(|l| l == expected),
                "{expected:?} in {labels:?}"
            );
        }
    }

    #[test]
    fn untimed_lists_and_narrow_mode_keep_their_old_widths() {
        // Wide mode: untimed texts are at most 12 wide, so the column stays 13 as before.
        for longest in 0..=12 {
            assert_eq!(due_column_width(longest, false), 13);
        }
        assert_eq!(due_column_width(23, false), 24, "longest + one blank");
        // Narrow mode: exactly the old rule.
        for longest in 0..30 {
            assert_eq!(due_column_width(longest, true), longest.max(5));
        }
    }

    #[test]
    fn narrow_mode_rows_still_line_up() {
        let mut app = app_with_tasks(TimeFormat::Hour12);
        let rows = task_rows(&mut app, 50);
        // Narrow mode has no priority text; the due column must still start in one place.
        let starts: Vec<usize> = app
            .tasks
            .iter()
            .zip(&rows)
            .filter_map(|(task, row)| {
                let label =
                    view::due_label(task, now(), true, IconMode::Unicode, TimeFormat::Hour12).text;
                (!label.is_empty()).then(|| column_of(row, &label).unwrap())
            })
            .collect();
        assert!(starts.windows(2).all(|w| w[0] == w[1]), "{starts:?}");
    }
}
