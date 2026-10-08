//! Settings popups: the theme picker, the first-run icon check, and the delete confirmation.
//!
//! The theme picker previews live: moving the selection changes `app.theme` immediately (see
//! `App::theme_picker_key`), so the whole screen behind the popup shows what you'll get.
//! Phase 2 will add the sync settings here.

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use zerohour_core::icons::{Icon, IconMode};
use zerohour_core::theme::{THEMES, Theme};

use super::{centered, color, popup_block};

/// The theme list with color swatches. `selected` is the row being previewed.
pub fn draw_theme_picker(frame: &mut Frame, theme: &Theme, selected: usize, icons: IconMode) {
    let height = THEMES.len() as u16 + 6;
    let area = centered(frame.area(), 46, height);
    let title = format!(" {} Theme ", Icon::Settings.get(icons));
    let block = popup_block(&title, theme, color(theme.accent));

    let mut lines = Vec::new();
    // `.enumerate()` gives each item's position along with the item: (0, first), (1, second)...
    for (i, t) in THEMES.iter().enumerate() {
        let is_selected = i == selected;
        let name_style = if is_selected {
            Style::new()
                .fg(color(theme.accent))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(color(theme.fg))
        };
        // Swatches show each theme's OWN colors (not the current theme's), so you can
        // compare them before moving the selection there.
        let swatch = |rgb| Span::styled("██", Style::new().fg(color(rgb)));
        lines.push(Line::from(vec![
            Span::styled(if is_selected { "› " } else { "  " }, name_style),
            Span::styled(format!("{:<14}", t.name), name_style),
            swatch(t.bg),
            swatch(t.task),
            swatch(t.event),
            swatch(t.accent),
            swatch(t.danger),
        ]));
    }
    lines.push(Line::default());
    let muted = Style::new().fg(color(theme.muted));
    lines.push(Line::from(Span::styled(
        "↑↓ preview · Enter keep · Esc cancel",
        muted,
    )));
    lines.push(Line::from(vec![
        Span::styled("i ", Style::new().fg(color(theme.accent))),
        Span::styled(format!("icons: {}", icons.as_str()), muted),
    ]));

    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// First-run question: can this terminal show Nerd Font icons?
pub fn draw_icon_check(frame: &mut Frame, theme: &Theme) {
    let area = centered(frame.area(), 64, 13);
    let block = popup_block(" Icon check ", theme, color(theme.accent));
    let accent = Style::new()
        .fg(color(theme.accent))
        .add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(color(theme.muted));

    // Show a few Nerd Font glyphs. If the font lacks them, they appear as boxes or "?".
    let samples = [Icon::Task, Icon::Event, Icon::Repeat, Icon::Stats]
        .map(|icon| icon.get(IconMode::Nerd))
        .join("  ");

    let lines = vec![
        Line::from("Do you see icons between the brackets (not boxes or ?)"),
        Line::default(),
        Line::from(vec![
            Span::raw("        [  "),
            Span::styled(samples, Style::new().fg(color(theme.task))),
            Span::raw("  ]"),
        ]),
        Line::default(),
        Line::from(vec![
            Span::styled("y ", accent),
            Span::raw("yes, use Nerd Font icons"),
        ]),
        Line::from(vec![
            Span::styled("n ", accent),
            Span::raw(format!(
                "no, use plain symbols ({} {})",
                Icon::Task.get(IconMode::Unicode),
                Icon::Event.get(IconMode::Unicode)
            )),
        ]),
        Line::default(),
        Line::from(Span::styled(
            "Want the icons? Set your terminal font to a Nerd Font \"Mono\" variant, \
             e.g. JetBrainsMono Nerd Font Mono. Change modes anytime with i.",
            muted,
        )),
    ];
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

/// "Delete this item?" confirmation, in the danger color.
pub fn draw_confirm_delete(frame: &mut Frame, theme: &Theme, title: &str) {
    let area = centered(frame.area(), 50, 6);
    let block = popup_block(" Delete? ", theme, color(theme.danger));
    let accent = Style::new()
        .fg(color(theme.danger))
        .add_modifier(Modifier::BOLD);
    let lines = vec![
        Line::from(format!("Delete \"{title}\"?")),
        Line::default(),
        Line::from(vec![
            Span::styled("y ", accent),
            Span::raw("delete   "),
            Span::styled(
                "n ",
                Style::new()
                    .fg(color(theme.accent))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("keep"),
        ]),
    ];
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}
