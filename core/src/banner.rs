//! The "zerohour" title banner: big letters drawn with text characters, shown at the top of
//! the main screen when there's room.
//!
//! The letters are hard-coded here as lines of text instead of being generated at run time
//! (no figlet library needed): the word never changes, so a constant is simpler. The art lives
//! in `core` (not in the UI) because UI code never hard-codes glyphs, and the Android app can
//! reuse it.
//!
//! Two fonts, picked by the icon mode:
//! - **Half-block** (Nerd and Unicode modes): 3 rows, built from `▀ ▄ █`, 31 columns wide.
//! - **smslant** (ASCII mode): 4 rows of plain ASCII, from `figlet -f smslant zerohour`
//!   (figlet's empty bottom row for letters that hang below the line was dropped, since no
//!   letter in "zerohour" does).

use crate::icons::IconMode;

/// The half-block wordmark (Unicode). Every row is exactly 31 characters wide.
const HALF_BLOCK: [&str; 3] = [
    "▀▀█ █▀▀ █▀█ █▀█ █ █ █▀█ █ █ █▀█",
    "▄▀  █▀▀ █▀▄ █ █ █▀█ █ █ █ █ █▀▄",
    "▀▀▀ ▀▀▀ ▀ ▀ ▀▀▀ ▀ ▀ ▀▀▀ ▀▀▀ ▀ ▀",
];

/// The smslant wordmark (plain ASCII). Every row is exactly 37 characters wide.
///
/// `r"..."` is a *raw string*: backslashes are taken literally instead of starting escapes
/// like `\n`, which keeps the art readable.
const SMSLANT: [&str; 4] = [
    r"                   __                ",
    r" ___ ___ _______  / /  ___  __ ______",
    r"/_ // -_) __/ _ \/ _ \/ _ \/ // / __/",
    r"/__/\__/_/  \___/_//_/\___/\_,_/_/   ",
];

/// The banner's rows for the given icon mode: half-block letters, or plain ASCII in ASCII
/// mode (for terminals or fonts without the block characters).
///
/// `&'static [&'static str]` is a borrowed list of string slices that live for the whole
/// program (they're baked into the binary), so callers can keep them without copying.
pub fn lines(mode: IconMode) -> &'static [&'static str] {
    match mode {
        IconMode::Nerd | IconMode::Unicode => &HALF_BLOCK,
        IconMode::Ascii => &SMSLANT,
    }
}

/// How many terminal columns the banner needs in this mode. Every row has the same width,
/// so the first row tells us. (`chars().count()` counts characters, not bytes: `▀` is 3
/// bytes in UTF-8 but one character, and one column on screen.)
pub fn width(mode: IconMode) -> usize {
    lines(mode).first().map_or(0, |row| row.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_of_each_font_has_the_same_width() {
        for (name, font) in [("half-block", &HALF_BLOCK[..]), ("smslant", &SMSLANT[..])] {
            let first = font[0].chars().count();
            for row in font {
                assert_eq!(row.chars().count(), first, "{name}: {row:?}");
            }
        }
        assert_eq!(width(IconMode::Unicode), 31);
        assert_eq!(width(IconMode::Ascii), 37);
    }

    #[test]
    fn the_ascii_font_is_pure_ascii() {
        for row in lines(IconMode::Ascii) {
            assert!(row.is_ascii(), "{row:?}");
        }
    }

    #[test]
    fn modes_pick_the_right_font() {
        assert_eq!(lines(IconMode::Nerd).len(), 3);
        assert_eq!(lines(IconMode::Unicode).len(), 3);
        assert_eq!(lines(IconMode::Ascii).len(), 4);
    }
}
