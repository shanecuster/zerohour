//! The `?` help overlay: every key, the date formats the add form understands, and a note
//! about fonts for icons. Any key closes it.
//!
//! If you change a key in `input.rs`, update the list here too.

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use zerohour_core::icons::{Icon, IconMode};
use zerohour_core::theme::Theme;

use super::{
    centered, color, popup_block, popup_height, popup_text_width, wrap_text, wrapped_lines,
};

/// Main-screen keys: (keys, what they do).
const KEYS: &[(&str, &str)] = &[
    ("a", "add a task or event"),
    ("space / x", "complete the selected task"),
    ("e / Enter", "edit the selected item"),
    ("d", "delete the selected item"),
    ("j k / ↑ ↓", "move · g G jump to top/bottom"),
    ("p", "show/hide one-off events that have passed"),
    ("v", "layout: split 50/50 ↔ events fit their content"),
    ("T", "times: 12-hour ↔ 24-hour"),
    ("b", "title banner on/off (shows at 30+ rows)"),
    ("Tab / s", "switch to stats and back"),
    ("t", "theme picker (live preview)"),
    ("n", "reminders: on/off, early, day time, test"),
    ("i", "icons: Nerd Font → Unicode → ASCII"),
    ("?", "this help"),
    ("q / Ctrl+C", "quit"),
];

/// Keys that only exist in builds with the `sync` feature.
#[cfg(feature = "sync")]
const SYNC_KEYS: &[(&str, &str)] = &[
    ("r", "sync now"),
    ("c", "sync settings (server, login, on/off)"),
];
/// Local-only builds have no sync keys.
#[cfg(not(feature = "sync"))]
const SYNC_KEYS: &[(&str, &str)] = &[];

/// Draws the help popup.
pub fn draw(frame: &mut Frame, theme: &Theme, icons: IconMode) {
    // 72 columns or the terminal width. Long lines wrap, so the height is counted from the
    // lines once they're built.
    let width = 72.min(frame.area().width);
    let text_width = popup_text_width(width);
    /// Width of the key column: 2 spaces + 12 for the keys.
    const KEY_COLUMN: usize = 14;
    let key_style = Style::new()
        .fg(color(theme.accent))
        .add_modifier(Modifier::BOLD);
    let heading = Style::new()
        .fg(color(theme.fg))
        .add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(color(theme.muted));

    let mut lines = vec![Line::from(Span::styled("Keys", heading))];
    // `.chain(...)` continues one list with another; sync builds add their keys at the end.
    for (keys, what) in KEYS.iter().chain(SYNC_KEYS) {
        // The description wraps within its own column, so continuation lines stay lined up
        // under it instead of running back under the keys.
        let parts = wrap_text(what, text_width.saturating_sub(KEY_COLUMN), 0);
        for (i, part) in parts.into_iter().enumerate() {
            let keys_cell = if i == 0 {
                format!("  {keys:<12}")
            } else {
                " ".repeat(KEY_COLUMN)
            };
            lines.push(Line::from(vec![
                Span::styled(keys_cell, key_style),
                Span::raw(part),
            ]));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Dates you can type", heading)));
    lines.extend(wrapped_lines(
        "  2026-12-25 · 12/25 · today · tomorrow · +3d · +2w · +1m · fri",
        text_width,
        2,
        muted,
    ));
    lines.extend(wrapped_lines(
        "  or press Ctrl+D / F2 in the date field for a calendar",
        text_width,
        2,
        muted,
    ));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Markers", heading)));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {} task  ", Icon::Task.get(icons)),
            Style::new().fg(color(theme.task)),
        ),
        Span::styled(
            format!("{} event  ", Icon::Event.get(icons)),
            Style::new().fg(color(theme.event)),
        ),
        Span::styled(format!("{} repeats  ", Icon::Repeat.get(icons)), muted),
        Span::styled(
            format!("{} overdue", Icon::Overdue.get(icons)),
            Style::new().fg(color(theme.danger)),
        ),
    ]));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Icons", heading)));
    lines.extend(wrapped_lines(
        "  Nerd Font icons need a Nerd Font set in your terminal; an app can't bundle one. \
         Pick a \"Mono\" variant (e.g. JetBrainsMono Nerd Font Mono) so icons stay one \
         column wide and the borders line up.",
        text_width,
        2,
        muted,
    ));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Press any key to close.", muted)));

    let area = centered(frame.area(), width, popup_height(lines.len()));
    let block = popup_block(" Help ", theme, color(theme.accent));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::{WIDTHS, assert_shows, screen_text};
    use zerohour_core::theme::THEMES;

    #[test]
    fn every_help_line_is_shown_in_full() {
        for width in WIDTHS {
            let screen = screen_text(width, 60, |frame| {
                draw(frame, &THEMES[0], IconMode::Unicode);
            });
            // Each key line reads "keys what" once the padding is collapsed.
            let key_lines: Vec<String> = KEYS
                .iter()
                .chain(SYNC_KEYS)
                .map(|(keys, what)| format!("{keys} {what}"))
                .collect();
            let mut expected: Vec<&str> = key_lines.iter().map(String::as_str).collect();
            expected.extend([
                "2026-12-25 · 12/25 · today · tomorrow · +3d · +2w · +1m · fri",
                "or press Ctrl+D / F2 in the date field for a calendar",
                "☐ task ◷ event ↻ repeats ! overdue",
                "Nerd Font icons need a Nerd Font set in your terminal; an app can't bundle one. \
                 Pick a \"Mono\" variant (e.g. JetBrainsMono Nerd Font Mono) so icons stay one \
                 column wide and the borders line up.",
                "Press any key to close.",
            ]);
            assert_shows(&screen, width, &expected);
        }
    }
}
