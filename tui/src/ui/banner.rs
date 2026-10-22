//! The "zerohour" title banner at the top of the main screen, plus a one-line tagline under
//! it ("7 days to Your Birthday").
//!
//! The letters come from `zerohour_core::banner` (half-block letters, or plain ASCII in ASCII
//! icon mode). This file decides whether there's room for them ([`fits`]), colors them, and
//! draws the tagline (whose words come from `zerohour_core::view`, shared with Android).
//!
//! **Colors** come only from theme slots: each column of the letters gets a color part of the
//! way from the `accent` slot (left edge) to the `event` slot (right edge), computed with
//! `Rgb::lerp`. So every theme gets its own gradient, and nothing is hard-coded.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use zerohour_core::banner;
use zerohour_core::icons::IconMode;
use zerohour_core::theme::{Rgb, Theme};
use zerohour_core::view;

use super::{NARROW_WIDTH, color, truncate};
use crate::app::App;

/// The banner only shows on terminals at least this many rows tall, so it never squeezes the
/// Tasks and Events boxes on small screens.
pub const MIN_ROWS: u16 = 30;

/// Rows the banner takes on screen in this icon mode: the letters (3 half-block rows, or 4
/// ASCII rows) plus 1 for the tagline.
pub fn height(mode: IconMode) -> u16 {
    // The fonts have 3 or 4 rows, so this conversion can never actually overflow.
    u16::try_from(banner::lines(mode).len()).unwrap_or(u16::MAX) + 1
}

/// Should the banner show on a screen of `width` × `rows` cells (the whole terminal)?
///
/// Yes when all of these hold:
/// - the terminal is at least [`MIN_ROWS`] tall;
/// - it isn't in narrow mode (under [`NARROW_WIDTH`] columns);
/// - the letters fit inside the screen border with a column to spare on each side;
/// - after the banner, the outer border (2), the key-hint footer (1) and a notes line (1),
///   the two section boxes still get at least 5 rows each. With 30+ rows that always holds
///   for both fonts, but the check keeps it true if the fonts or limits ever change.
pub fn fits(width: u16, rows: u16, mode: IconMode) -> bool {
    const BORDER_AND_FOOTER: u16 = 2 + 1 + 1;
    const MIN_SECTIONS: u16 = 2 * 5;
    let wide_enough = width >= NARROW_WIDTH && usize::from(width) >= banner::width(mode) + 4;
    let sections_left = rows.saturating_sub(BORDER_AND_FOOTER + height(mode));
    rows >= MIN_ROWS && wide_enough && sections_left >= MIN_SECTIONS
}

/// Draws the banner and tagline into `area` (which is [`height`] rows tall).
pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.theme;
    let rows = banner::lines(app.icon_mode);
    let width = banner::width(app.icon_mode);

    let mut lines: Vec<Line> = rows
        .iter()
        .map(|row| {
            // One span per character, so each column can have its own color.
            let spans: Vec<Span> = row
                .chars()
                .enumerate()
                .map(|(column, ch)| {
                    Span::styled(
                        ch.to_string(),
                        Style::new().fg(color(column_color(theme, column, width))),
                    )
                })
                .collect();
            Line::from(spans)
        })
        .collect();

    // The tagline: the next event's countdown, or today's date if there's none. Core picks
    // the event (visible, upcoming ones only) and writes the words.
    let text = view::tagline_text(view::next_upcoming(&app.events), app.today);
    lines.push(Line::from(Span::styled(
        truncate(&text, usize::from(area.width)),
        Style::new().fg(color(theme.muted)),
    )));

    // Every row has the same width, so centering each line keeps the letters lined up.
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

/// The gradient color for one column of the letters: the theme's `accent` slot at the left
/// edge (column 0), its `event` slot at the right edge (column `width - 1`), and evenly mixed
/// in between.
fn column_color(theme: &Theme, column: usize, width: usize) -> Rgb {
    // How far across the banner this column is: 0.0 at the left, 1.0 at the right.
    // (`width.max(2) - 1` is never 0, so this never divides by zero.)
    let t = column as f32 / (width.max(2) - 1) as f32;
    theme.accent.lerp(theme.event, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_differ_by_font_plus_one_for_the_tagline() {
        assert_eq!(height(IconMode::Nerd), 3 + 1);
        assert_eq!(height(IconMode::Unicode), 3 + 1);
        assert_eq!(height(IconMode::Ascii), 4 + 1);
    }

    #[test]
    fn show_rule_accounts_for_each_fonts_height() {
        for mode in [IconMode::Unicode, IconMode::Ascii] {
            // At exactly 30 rows it shows, and leaves the sections at least 5 rows each:
            // 30 - border 2 - footer 1 - notes 1 - banner (4 or 5) = 22 or 21.
            assert!(fits(80, MIN_ROWS, mode), "{mode:?}");
            let left = MIN_ROWS - 4 - height(mode);
            assert!(left / 2 >= 5, "{mode:?}: {left} rows left");
            assert_eq!(left, if mode == IconMode::Ascii { 21 } else { 22 });
            // One row less: hidden.
            assert!(!fits(80, MIN_ROWS - 1, mode), "{mode:?}");
        }
    }

    #[test]
    fn never_in_narrow_mode_or_when_too_narrow_for_the_letters() {
        for mode in [IconMode::Unicode, IconMode::Ascii] {
            assert!(!fits(NARROW_WIDTH - 1, 40, mode), "narrow mode");
            assert!(fits(NARROW_WIDTH, 40, mode));
        }
    }

    #[test]
    fn gradient_runs_from_the_accent_slot_to_the_event_slot() {
        use zerohour_core::theme::THEMES;
        for theme in THEMES.iter() {
            assert_eq!(column_color(theme, 0, 31), theme.accent, "{}", theme.name);
            assert_eq!(column_color(theme, 30, 31), theme.event, "{}", theme.name);
            assert_eq!(
                column_color(theme, 15, 31),
                theme.accent.lerp(theme.event, 0.5)
            );
        }
    }
}
