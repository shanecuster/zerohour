//! Settings popups: the theme picker, the first-run icon check, and the delete confirmation.
//!
//! The theme picker previews live: moving the selection changes `app.theme` immediately (see
//! `App::theme_picker_key`), so the whole screen behind the popup shows what you'll get.
//! Phase 2 will add the sync settings here.

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use zerohour_core::icons::{Icon, IconMode};
use zerohour_core::theme::{THEMES, Theme};

use super::{centered, color, popup_block, popup_height, popup_text_width, wrapped_lines};

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
    // 64 columns or the terminal width; the height follows from the wrapped text.
    let width = 64.min(frame.area().width);
    let text_width = popup_text_width(width);
    let accent = Style::new()
        .fg(color(theme.accent))
        .add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(color(theme.muted));

    // Show a few Nerd Font glyphs. If the font lacks them, they appear as boxes or "?".
    let samples = [Icon::Task, Icon::Event, Icon::Repeat, Icon::Stats]
        .map(|icon| icon.get(IconMode::Nerd))
        .join("  ");

    let mut lines = wrapped_lines(
        "Do you see icons between the brackets (not boxes or ?)",
        text_width,
        0,
        Style::new().fg(color(theme.fg)),
    );
    lines.extend([
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
    ]);
    lines.extend(wrapped_lines(
        "Want the icons? Set your terminal font to a Nerd Font \"Mono\" variant, \
         e.g. JetBrainsMono Nerd Font Mono. Change modes anytime with i.",
        text_width,
        0,
        muted,
    ));
    let area = centered(frame.area(), width, popup_height(lines.len()));
    let block = popup_block(" Icon check ", theme, color(theme.accent));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// "Delete this item?" confirmation, in the danger color.
///
/// A long title wraps onto more lines and the popup grows to fit, so the y/n keys below it are
/// never pushed out of view.
pub fn draw_confirm_delete(frame: &mut Frame, theme: &Theme, title: &str) {
    let width = 50.min(frame.area().width);
    let accent = Style::new()
        .fg(color(theme.danger))
        .add_modifier(Modifier::BOLD);
    let mut lines = wrapped_lines(
        &format!("Delete \"{title}\"?"),
        popup_text_width(width),
        0,
        Style::new().fg(color(theme.fg)),
    );
    lines.extend([
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
    ]);
    let area = centered(frame.area(), width, popup_height(lines.len()));
    let block = popup_block(" Delete? ", theme, color(theme.danger));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::{WIDTHS, assert_shows, screen_text};

    #[test]
    fn theme_picker_is_never_cut_off() {
        for width in WIDTHS {
            let screen = screen_text(width, 40, |frame| {
                draw_theme_picker(frame, &THEMES[0], 0, IconMode::Unicode);
            });
            let mut expected: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
            expected.extend(["↑↓ preview · Enter keep · Esc cancel", "icons: unicode"]);
            assert_shows(&screen, width, &expected);
        }
    }

    #[test]
    fn icon_check_is_never_cut_off() {
        for width in WIDTHS {
            let screen = screen_text(width, 40, |frame| draw_icon_check(frame, &THEMES[0]));
            assert_shows(
                &screen,
                width,
                &[
                    "Do you see icons between the brackets (not boxes or ?)",
                    "y yes, use Nerd Font icons",
                    "n no, use plain symbols (☐ ◷)",
                    "Want the icons? Set your terminal font to a Nerd Font \"Mono\" variant, \
                     e.g. JetBrainsMono Nerd Font Mono. Change modes anytime with i.",
                ],
            );
        }
    }

    #[test]
    fn delete_confirmation_with_a_long_title_is_never_cut_off() {
        let title = "Call the insurance company about the windshield claim, the rental car, \
                     and the repair shop's estimate before Friday afternoon";
        for width in WIDTHS {
            let screen = screen_text(width, 40, |frame| {
                draw_confirm_delete(frame, &THEMES[0], title);
            });
            let question = format!("Delete \"{title}\"?");
            assert_shows(&screen, width, &[question.as_str(), "y delete n keep"]);
        }
    }
}
