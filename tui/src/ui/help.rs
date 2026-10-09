//! The `?` help overlay: every key, the date formats the add form understands, and a note
//! about fonts for icons. Any key closes it.
//!
//! If you change a key in `input.rs`, update the list here too.

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use zerohour_core::icons::{Icon, IconMode};
use zerohour_core::theme::Theme;

use super::{centered, color, popup_block};

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
    // Sync builds list extra keys, so the popup is a bit taller there.
    let height = if cfg!(feature = "sync") { 33 } else { 31 };
    let area = centered(frame.area(), 72, height);
    let block = popup_block(" Help ", theme, color(theme.accent));
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
        lines.push(Line::from(vec![
            Span::styled(format!("  {keys:<12}"), key_style),
            Span::raw(*what),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Dates you can type", heading)));
    lines.push(Line::from(Span::styled(
        "  2026-12-25 · 12/25 · today · tomorrow · +3d · +2w · +1m · fri",
        muted,
    )));
    lines.push(Line::from(Span::styled(
        "  or press Ctrl+D / F2 in the date field for a calendar",
        muted,
    )));
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
    lines.push(Line::from(Span::styled(
        "  Nerd Font icons need a Nerd Font set in your terminal; an app can't bundle one. \
         Pick a \"Mono\" variant (e.g. JetBrainsMono Nerd Font Mono) so icons stay one \
         column wide and the borders line up.",
        muted,
    )));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Press any key to close.", muted)));

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}
