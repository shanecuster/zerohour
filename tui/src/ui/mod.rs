//! Drawing: turns the app state into what you see on screen.
//!
//! Ratatui is an *immediate mode* UI library: every frame we describe the whole screen from
//! scratch based on the current state, and ratatui works out which terminal cells changed. There
//! are no widgets that "live" between frames — so drawing code only reads state, and all
//! changes to state happen in `app.rs` in response to keys.
//!
//! Submodules:
//! - [`list`]     — the main screen: Tasks section above, Events section below.
//! - [`add`]      — the add/edit dialog (also owns that dialog's state and keys).
//! - [`calendar`] — the month-grid date picker that opens over the add/edit dialog.
//! - [`stats`]    — the stats screen with the weekly bar chart.
//! - [`settings`] — the theme picker and the first-run icon check.
//! - [`help`]     — the `?` help overlay.
//! - `sync_settings` — the sync settings popup (only with the `sync` feature).
//!
//! This file has the entry point [`draw`] plus small helpers every screen uses.

pub mod add;
pub mod calendar;
pub mod help;
pub mod list;
pub mod settings;
pub mod stats;
#[cfg(feature = "sync")]
pub mod sync_settings;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zerohour_core::APP_NAME;
use zerohour_core::theme::{Rgb, Theme};

use crate::app::{App, Overlay, Screen};

/// Below this many columns the main screen switches to "narrow mode": secondary columns are
/// dropped and titles are shortened with `…`. It's also the model for the phone layout.
pub const NARROW_WIDTH: u16 = 60;

/// Draws one complete frame. Called by the main loop every time something may have changed.
///
/// `&mut App` because tables remember their scroll position in the app between frames.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let theme = app.theme;
    let area = frame.area();

    // Paint the theme's background and default text color across the whole terminal first,
    // so every cell (even ones no widget touches) uses the theme.
    frame.render_widget(Block::new().style(base_style(theme)), area);

    match app.screen {
        Screen::Main => list::draw(frame, area, app),
        Screen::Stats => stats::draw(frame, area, app),
    }

    // Popups go on top of the screen. `match &app.overlay` borrows the overlay so we can look
    // inside it without taking it out of `app`.
    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => help::draw(frame, theme, app.icon_mode),
        Overlay::Form(form) => add::draw(frame, form, theme, app.icon_mode, app.today),
        Overlay::ThemePicker { selected, .. } => {
            settings::draw_theme_picker(frame, theme, *selected, app.icon_mode);
        }
        Overlay::ConfirmDelete { title, .. } => settings::draw_confirm_delete(frame, theme, title),
        Overlay::IconCheck => settings::draw_icon_check(frame, theme),
        #[cfg(feature = "sync")]
        Overlay::SyncSettings(form) => {
            sync_settings::draw(frame, form, &app.sync, theme, app.icon_mode);
        }
    }
}

/// Converts a core color into a ratatui color (24-bit "true color").
///
/// This conversion is the ONLY place the TUI turns theme slots into terminal colors.
pub fn color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.0, rgb.1, rgb.2)
}

/// Normal text on the theme background.
pub fn base_style(theme: &Theme) -> Style {
    Style::new().fg(color(theme.fg)).bg(color(theme.bg))
}

/// The outer frame of a full screen: rounded border, app name on the left, date on the right,
/// and an optional status message on the bottom border.
pub fn screen_block<'a>(app: &'a App, subtitle: &'a str) -> Block<'a> {
    let theme = app.theme;
    let title = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            APP_NAME,
            Style::new()
                .fg(color(theme.accent))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(subtitle, Style::new().fg(color(theme.muted))),
        Span::raw(" "),
    ]);
    // Right side of the top border: the sync indicator (sync builds, when switched on), then
    // the date. `mut` is only needed in sync builds, hence the `allow`.
    #[allow(unused_mut)]
    let mut right: Vec<Span> = Vec::new();
    #[cfg(feature = "sync")]
    if let Some(indicator) = sync_indicator(app) {
        right.push(indicator);
    }
    right.push(Span::styled(
        format!(" {} ", app.today.format("%a %b %d %Y")),
        Style::new().fg(color(theme.muted)),
    ));
    let date = Line::from(right).alignment(Alignment::Right);

    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color(theme.border)))
        .title(title)
        .title(date);
    if let Some(status) = &app.status {
        let slot = if status.is_error {
            theme.danger
        } else {
            theme.ok
        };
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {} ", status.text),
                Style::new().fg(color(slot)),
            ))
            .alignment(Alignment::Left),
        );
    }
    block
}

/// The small sync status in the header: never synced / syncing / synced 14:03 / offline /
/// sync error. `None` (nothing drawn) when sync is switched off, so a local-only setup looks
/// exactly as it always did.
#[cfg(feature = "sync")]
fn sync_indicator(app: &App) -> Option<Span<'static>> {
    use crate::sync::SyncState;
    use zerohour_core::icons::Icon;

    if !app.sync.visible() {
        return None;
    }
    let theme = app.theme;
    let (icon, text, slot) = match &app.sync.state {
        SyncState::Never => (Icon::Sync, "never synced".to_string(), theme.muted),
        SyncState::Syncing => (Icon::Sync, "syncing…".to_string(), theme.accent),
        SyncState::Synced(time) => {
            let local = time.with_timezone(&chrono::Local);
            // Today: just the time. Earlier: the date, so a stale sync is obvious.
            let when = if local.date_naive() == app.today {
                local.format("%H:%M").to_string()
            } else {
                local.format("%b %d").to_string()
            };
            let slot = if app.sync.clock_skew_secs.is_some() {
                theme.warn
            } else {
                theme.ok
            };
            (Icon::Synced, format!("synced {when}"), slot)
        }
        SyncState::Offline => (Icon::Offline, "offline".to_string(), theme.warn),
        SyncState::Error(_) => (Icon::SyncError, "sync error".to_string(), theme.danger),
    };
    Some(Span::styled(
        format!(" {} {text} ·", icon.get(app.icon_mode)),
        Style::new().fg(color(slot)),
    ))
}

/// A bordered popup box with a title, in the given border color.
pub fn popup_block<'a>(title: &str, theme: &Theme, border: Color) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .title(Span::styled(
            title.to_string(),
            Style::new().fg(border).add_modifier(Modifier::BOLD),
        ))
        .style(base_style(theme))
        .padding(Padding::horizontal(1))
}

/// A rectangle of (at most) `width` x `height`, centered inside `area`. Used for popups.
/// If the terminal is smaller than the popup, the popup shrinks to fit.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Shortens `text` to at most `max` terminal columns, ending with `…` if anything was cut.
///
/// We measure *display width*, not bytes or characters: "é" is 2 bytes but 1 column, and some
/// characters (like many CJK ones) take 2 columns. `unicode-width` knows the rules.
pub fn truncate(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        // Leave one column for the ellipsis.
        if used + w > max - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// The *end* of `text` that fits in `max` columns, with a leading `…` if cut. Used for text
/// inputs: while typing, the end of the text (where the cursor is) is what matters.
pub fn tail_fit(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    // `.rev()` walks the characters from the end.
    for ch in text.chars().rev() {
        let w = ch.width().unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        kept.push(ch);
        used += w;
    }
    // We collected backwards, so flip it and put the ellipsis in front.
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// Display width of a string in terminal columns (as a u16 for layout math).
pub fn width_u16(text: &str) -> u16 {
    u16::try_from(text.width()).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_adds_ellipsis_only_when_needed() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("hello", 0), "");
        assert_eq!(truncate("日本語", 4), "日…"); // wide characters count as 2
    }

    #[test]
    fn tail_fit_keeps_the_end() {
        assert_eq!(tail_fit("abc", 5), "abc");
        assert_eq!(tail_fit("abcdefgh", 4), "…fgh");
    }

    #[test]
    fn centered_shrinks_to_fit() {
        let area = Rect::new(0, 0, 40, 10);
        assert_eq!(centered(area, 20, 4), Rect::new(10, 3, 20, 4));
        assert_eq!(centered(area, 100, 100), area);
    }
}
