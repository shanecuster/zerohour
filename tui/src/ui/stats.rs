//! The stats screen: summary cards, a bar chart of completions per week, and a priority line.
//!
//! All numbers come from `app.stats`, which `core::stats::compute` fills in on every refresh.
//! This file only lays them out.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, BorderType, Paragraph};
use zerohour_core::icons::{Icon, IconMode, priority_icon};
use zerohour_core::model::Priority;
use zerohour_core::theme::{Rgb, Theme};

use super::{NARROW_WIDTH, color, screen_block};
use crate::app::App;

/// Draws the stats screen into `area`.
pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = app.theme;
    let mode = app.icon_mode;
    let subtitle = format!(" · {} stats", Icon::Stats.get(mode));
    let outer = screen_block(app, &subtitle);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let narrow = area.width < NARROW_WIDTH;
    // Cards are 4 lines tall; in narrow mode they stack as a 2x2 grid (8 lines).
    let cards_height = if narrow { 8 } else { 4 };
    let [cards_area, chart_area, priority_area, footer_area] = Layout::vertical([
        Constraint::Length(cards_height),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    draw_cards(frame, cards_area, app, narrow);
    draw_chart(frame, chart_area, app);
    draw_priorities(frame, priority_area, app, mode);
    draw_footer(frame, footer_area, theme);
}

/// The four summary cards: Completed, Streak, Overdue, On time.
fn draw_cards(frame: &mut Frame, area: Rect, app: &App, narrow: bool) {
    let s = &app.stats;
    let theme = app.theme;

    let on_time = match s.on_time_rate() {
        Some(rate) => format!("{:.0}%", rate * 100.0),
        None => "–".to_string(),
    };
    let overdue_slot = if s.overdue_count > 0 {
        theme.danger
    } else {
        theme.ok
    };
    // (title, big number, small text under it, color of the number)
    let cards: [(&str, String, String, Rgb); 4] = [
        (
            "Completed",
            s.total_all.to_string(),
            format!("week {} · month {}", s.total_week, s.total_month),
            theme.accent,
        ),
        (
            "Streak",
            format!("{} {}", s.current_streak, plural(s.current_streak, "day")),
            format!("best {}", s.longest_streak),
            theme.ok,
        ),
        (
            "Overdue",
            s.overdue_count.to_string(),
            "open tasks past due".to_string(),
            overdue_slot,
        ),
        (
            "On time",
            on_time,
            format!("{} on time · {} late", s.on_time, s.late),
            theme.task,
        ),
    ];

    // Split into 4 equal columns, or 2 rows of 2 columns when narrow.
    let slots: Vec<Rect> = if narrow {
        let [top, bottom] = Layout::vertical([Constraint::Fill(1); 2]).areas(area);
        let [a, b] = Layout::horizontal([Constraint::Fill(1); 2]).areas(top);
        let [c, d] = Layout::horizontal([Constraint::Fill(1); 2]).areas(bottom);
        vec![a, b, c, d]
    } else {
        Layout::horizontal([Constraint::Fill(1); 4])
            .split(area)
            .to_vec()
    };

    // `zip` pairs each card with its screen slot.
    for ((title, big, small, slot), rect) in cards.into_iter().zip(slots) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(color(theme.border)))
            .title(Span::styled(
                format!(" {title} "),
                Style::new().fg(color(theme.muted)),
            ));
        let lines = vec![
            Line::from(Span::styled(
                format!(" {big}"),
                Style::new().fg(color(slot)).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!(" {small}"),
                Style::new().fg(color(theme.muted)),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

/// The bar chart of completions per week (oldest on the left, this week on the right).
fn draw_chart(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.theme;
    let weeks = &app.stats.per_week;
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color(theme.border)))
        .title(Span::styled(
            " Completions per week ",
            Style::new()
                .fg(color(theme.accent))
                .add_modifier(Modifier::BOLD),
        ));

    if app.stats.total_all == 0 {
        let hint = Paragraph::new(Span::styled(
            " No completions yet. Finish a task with space and it shows up here.",
            Style::new().fg(color(theme.muted)),
        ))
        .block(block);
        frame.render_widget(hint, area);
        return;
    }

    // Fit all bars across the width: each bar gets an equal share minus a 1-column gap.
    let inner_width = area.width.saturating_sub(2);
    let count = weeks.len().max(1) as u16;
    let gap = 1;
    let bar_width = (inner_width.saturating_sub(gap * (count - 1)) / count).max(1);
    // Labels under the bars: "10/05" if there's room, else just the day of the month.
    let label_format = if bar_width >= 5 { "%m/%d" } else { "%d" };

    let bars: Vec<Bar> = weeks
        .iter()
        .enumerate()
        .map(|(i, week)| {
            // Highlight the current (last) week.
            let is_current = i + 1 == weeks.len();
            let slot = if is_current { theme.accent } else { theme.task };
            Bar::default()
                .value(u64::from(week.count))
                .label(Line::from(week.week_start.format(label_format).to_string()))
                .style(Style::new().fg(color(slot)))
                .value_style(Style::new().fg(color(theme.bg)).bg(color(slot)))
        })
        .collect();

    let chart = BarChart::default()
        .block(block)
        .data(BarGroup::default().bars(&bars))
        .bar_width(bar_width)
        .bar_gap(gap)
        .label_style(Style::new().fg(color(theme.muted)));
    frame.render_widget(chart, area);
}

/// One line: completions by priority, with the priority icons.
fn draw_priorities(frame: &mut Frame, area: Rect, app: &App, mode: IconMode) {
    let theme = app.theme;
    let p = app.stats.by_priority;
    let mut spans = vec![Span::styled(
        " By priority  ",
        Style::new().fg(color(theme.muted)),
    )];
    for (priority, count, slot) in [
        (Priority::High, p.high, theme.danger),
        (Priority::Medium, p.medium, theme.warn),
        (Priority::Low, p.low, theme.muted),
    ] {
        spans.push(Span::styled(
            format!(
                "{} {} ",
                priority_icon(priority).get(mode),
                priority.label()
            ),
            Style::new().fg(color(slot)),
        ));
        spans.push(Span::styled(
            format!("{count}   "),
            Style::new()
                .fg(color(theme.fg))
                .add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Key hints for this screen.
fn draw_footer(frame: &mut Frame, area: Rect, theme: &Theme) {
    let key = |k: &'static str| {
        Span::styled(
            k,
            Style::new()
                .fg(color(theme.accent))
                .add_modifier(Modifier::BOLD),
        )
    };
    let word = |w: &'static str| Span::styled(w, Style::new().fg(color(theme.muted)));
    let line = Line::from(vec![
        Span::raw(" "),
        key("Tab"),
        word(" back  "),
        key("t"),
        word(" theme  "),
        key("?"),
        word(" help  "),
        key("q"),
        word(" quit"),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// "1 day" vs "2 days".
fn plural(n: u32, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}
