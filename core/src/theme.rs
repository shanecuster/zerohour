//! Color themes: named color "slots" and the built-in palettes.
//!
//! UI code NEVER uses raw colors. It asks for a slot ("the task color", "the danger color") and
//! the current theme decides the actual RGB value. Switching themes is then just swapping which
//! [`Theme`] is current — nothing else in the UI changes.
//!
//! The palettes live here in `core` (not in the TUI) so the Android app can use the exact same
//! colors.
//!
//! **Adding a theme = adding ONE entry to [`THEMES`].** The theme picker, the settings, and the
//! tests all read that list, so nothing else needs touching.

/// A 24-bit color: red, green, blue, each 0–255.
///
/// A "tuple struct": fields are unnamed and accessed as `.0`, `.1`, `.2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Builds a color from a hex number like `0x2d353b` (the same digits as CSS `#2d353b`),
    /// so palettes can be copied straight from a theme's official docs.
    ///
    /// `const fn` means it can run at compile time, which lets [`THEMES`] be a constant.
    /// `>>` shifts bits right and `& 0xff` keeps the lowest 8 bits, pulling out each channel.
    pub const fn hex(value: u32) -> Rgb {
        Rgb(
            ((value >> 16) & 0xff) as u8,
            ((value >> 8) & 0xff) as u8,
            (value & 0xff) as u8,
        )
    }

    /// The color a fraction `t` of the way from `self` to `other`: `t = 0.0` gives `self`,
    /// `1.0` gives `other`, `0.5` the midpoint. Values outside 0..=1 are clamped.
    ///
    /// Used for gradients (the title banner fades from the `accent` slot to the `event` slot),
    /// so in-between colors always come from the theme instead of being hard-coded.
    pub fn lerp(self, other: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        // Mix one channel: start + (end - start) * t, rounded back to a whole 0-255 value.
        // `f32::from(u8)` widens without loss; the result always fits back in 0..=255.
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Rgb(
            mix(self.0, other.0),
            mix(self.1, other.1),
            mix(self.2, other.2),
        )
    }
}

/// One complete theme: a name plus a color for every slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Display name, also what's saved in settings.
    pub name: &'static str,
    /// Background of the whole app.
    pub bg: Rgb,
    /// Normal text.
    pub fg: Rgb,
    /// De-emphasized text: hints, secondary columns, empty-state messages.
    pub muted: Rgb,
    /// Box borders.
    pub border: Rgb,
    /// Highlights: the header, key hints, focused form field.
    pub accent: Rgb,
    /// Background of the selected row.
    pub selection: Rgb,
    /// The Tasks section color. Must differ clearly from `event`.
    pub task: Rgb,
    /// The Events section color. Must differ clearly from `task`.
    pub event: Rgb,
    /// Good things: done, on time, streaks.
    pub ok: Rgb,
    /// Attention: due soon, medium priority.
    pub warn: Rgb,
    /// Problems: overdue, high priority, delete confirmations.
    pub danger: Rgb,
}

/// The default theme's name (the owner's favorite).
pub const DEFAULT_THEME: &str = "Everforest";

/// Every built-in theme. The first entry is the default.
///
/// To add a theme, copy one block, rename it, and change the colors. That's all.
pub const THEMES: &[Theme] = &[
    // https://github.com/sainnhe/everforest (dark, medium contrast)
    Theme {
        name: "Everforest",
        bg: Rgb::hex(0x2d353b),
        fg: Rgb::hex(0xd3c6aa),
        muted: Rgb::hex(0x859289),
        border: Rgb::hex(0x56635f),
        accent: Rgb::hex(0xa7c080),
        selection: Rgb::hex(0x475258),
        task: Rgb::hex(0x7fbbb3),
        event: Rgb::hex(0xd699b6),
        ok: Rgb::hex(0xa7c080),
        warn: Rgb::hex(0xdbbc7f),
        danger: Rgb::hex(0xe67e80),
    },
    // https://draculatheme.com/contribute
    Theme {
        name: "Dracula",
        bg: Rgb::hex(0x282a36),
        fg: Rgb::hex(0xf8f8f2),
        muted: Rgb::hex(0x6272a4),
        border: Rgb::hex(0x6272a4),
        accent: Rgb::hex(0xbd93f9),
        selection: Rgb::hex(0x44475a),
        task: Rgb::hex(0x8be9fd),
        event: Rgb::hex(0xff79c6),
        ok: Rgb::hex(0x50fa7b),
        warn: Rgb::hex(0xf1fa8c),
        danger: Rgb::hex(0xff5555),
    },
    // https://github.com/morhetz/gruvbox (dark)
    Theme {
        name: "Gruvbox",
        bg: Rgb::hex(0x282828),
        fg: Rgb::hex(0xebdbb2),
        muted: Rgb::hex(0x928374),
        border: Rgb::hex(0x665c54),
        accent: Rgb::hex(0xfe8019),
        selection: Rgb::hex(0x504945),
        task: Rgb::hex(0x83a598),
        event: Rgb::hex(0xd3869b),
        ok: Rgb::hex(0xb8bb26),
        warn: Rgb::hex(0xfabd2f),
        danger: Rgb::hex(0xfb4934),
    },
    // https://catppuccin.com/palette (Mocha flavor)
    Theme {
        name: "Catppuccin",
        bg: Rgb::hex(0x1e1e2e),
        fg: Rgb::hex(0xcdd6f4),
        muted: Rgb::hex(0x7f849c),
        border: Rgb::hex(0x585b70),
        accent: Rgb::hex(0xcba6f7),
        selection: Rgb::hex(0x45475a),
        task: Rgb::hex(0x89b4fa),
        event: Rgb::hex(0xf5c2e7),
        ok: Rgb::hex(0xa6e3a1),
        warn: Rgb::hex(0xf9e2af),
        danger: Rgb::hex(0xf38ba8),
    },
    // https://www.nordtheme.com/docs/colors-and-palettes
    Theme {
        name: "Nord",
        bg: Rgb::hex(0x2e3440),
        fg: Rgb::hex(0xd8dee9),
        muted: Rgb::hex(0x7b88a1),
        border: Rgb::hex(0x4c566a),
        accent: Rgb::hex(0x81a1c1),
        selection: Rgb::hex(0x434c5e),
        task: Rgb::hex(0x88c0d0),
        event: Rgb::hex(0xb48ead),
        ok: Rgb::hex(0xa3be8c),
        warn: Rgb::hex(0xebcb8b),
        danger: Rgb::hex(0xbf616a),
    },
    // https://ethanschoonover.com/solarized/ (dark)
    Theme {
        name: "Solarized",
        bg: Rgb::hex(0x002b36),
        fg: Rgb::hex(0x93a1a1),
        muted: Rgb::hex(0x657b83),
        border: Rgb::hex(0x586e75),
        accent: Rgb::hex(0x268bd2),
        selection: Rgb::hex(0x073642),
        task: Rgb::hex(0x2aa198),
        event: Rgb::hex(0xd33682),
        ok: Rgb::hex(0x859900),
        warn: Rgb::hex(0xb58900),
        danger: Rgb::hex(0xdc322f),
    },
    // https://github.com/folke/tokyonight.nvim (night)
    Theme {
        name: "Tokyo Night",
        bg: Rgb::hex(0x1a1b26),
        fg: Rgb::hex(0xc0caf5),
        muted: Rgb::hex(0x565f89),
        border: Rgb::hex(0x3b4261),
        accent: Rgb::hex(0x7aa2f7),
        selection: Rgb::hex(0x283457),
        task: Rgb::hex(0x7dcfff),
        event: Rgb::hex(0xbb9af7),
        ok: Rgb::hex(0x9ece6a),
        warn: Rgb::hex(0xe0af68),
        danger: Rgb::hex(0xf7768e),
    },
];

/// Finds a theme by name (case-insensitive). Unknown names fall back to the default theme,
/// so a typo in the settings can never break the app.
///
/// The return type `&'static Theme` is a borrow of an entry in the [`THEMES`] constant, which
/// lives for the whole program — so the caller can keep it as long as it likes.
pub fn theme_by_name(name: &str) -> &'static Theme {
    THEMES
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(name))
        .unwrap_or(&THEMES[0])
}

/// The position of a theme in [`THEMES`] (used by the theme picker), defaulting to 0.
pub fn theme_index(name: &str) -> usize {
    THEMES
        .iter()
        .position(|t| t.name.eq_ignore_ascii_case(name))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_goes_from_one_color_to_the_other() {
        let black = Rgb(0, 0, 0);
        let white = Rgb(255, 255, 255);
        assert_eq!(black.lerp(white, 0.0), black);
        assert_eq!(black.lerp(white, 1.0), white);
        assert_eq!(black.lerp(white, 0.5), Rgb(128, 128, 128));
        assert_eq!(
            white.lerp(black, 0.25),
            Rgb(191, 191, 191),
            "works downwards too"
        );
        assert_eq!(black.lerp(white, 7.0), white, "clamped");
        assert_eq!(
            Rgb(10, 200, 30).lerp(Rgb(10, 200, 30), 0.3),
            Rgb(10, 200, 30)
        );
    }

    /// A rough "how different are two colors" score: the sum of channel differences.
    fn distance(a: Rgb, b: Rgb) -> u32 {
        a.0.abs_diff(b.0) as u32 + a.1.abs_diff(b.1) as u32 + a.2.abs_diff(b.2) as u32
    }

    #[test]
    fn default_theme_is_first() {
        assert_eq!(THEMES[0].name, DEFAULT_THEME);
    }

    #[test]
    fn all_seven_themes_exist_with_unique_names() {
        let names: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
        for expected in [
            "Everforest",
            "Dracula",
            "Gruvbox",
            "Catppuccin",
            "Nord",
            "Solarized",
            "Tokyo Night",
        ] {
            assert!(names.contains(&expected), "missing theme {expected}");
        }
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate theme names");
    }

    #[test]
    fn task_and_event_colors_are_clearly_different_in_every_theme() {
        for t in THEMES {
            assert!(
                distance(t.task, t.event) > 100,
                "{}: task and event colors are too similar",
                t.name
            );
        }
    }

    #[test]
    fn text_stands_out_from_background_in_every_theme() {
        for t in THEMES {
            assert!(distance(t.fg, t.bg) > 200, "{}: fg too close to bg", t.name);
            assert_ne!(t.selection, t.bg, "{}: selection invisible", t.name);
        }
    }

    #[test]
    fn lookup_is_case_insensitive_and_falls_back() {
        assert_eq!(theme_by_name("nord").name, "Nord");
        assert_eq!(theme_by_name("tokyo night").name, "Tokyo Night");
        assert_eq!(theme_by_name("no such theme").name, DEFAULT_THEME);
        assert_eq!(theme_index("Gruvbox"), 2);
    }

    #[test]
    fn hex_splits_channels() {
        assert_eq!(Rgb::hex(0x2d353b), Rgb(0x2d, 0x35, 0x3b));
    }
}
