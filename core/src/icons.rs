//! Icons: every symbol the app shows, defined once, in three styles.
//!
//! UI code asks for an icon by *name* (`Icon::Task`) plus the user's [`IconMode`], and gets
//! back the glyph to draw. It never hard-codes a glyph, so switching modes changes every icon at
//! once and adding a new style later is a one-file change.
//!
//! The three modes:
//! - **Nerd** — Nerd Font icons (Font Awesome glyphs patched into programming fonts). Prettiest,
//!   but only show up if the terminal uses a Nerd Font. Otherwise you'll see boxes or `?`.
//! - **Unicode** — plain Unicode symbols like `☐` and `◷` that almost every font has.
//! - **ASCII** — `[ ]` and `(o)`. Works literally everywhere, including old consoles.
//!
//! **Why "Nerd Font Mono"?** Normal Nerd Fonts draw many icons wider than one character cell.
//! The terminal still advances one cell, so a wide icon overlaps the next character and table
//! columns and box borders stop lining up. The *Mono* variants (e.g. "JetBrainsMono Nerd Font
//! Mono") squeeze every icon into exactly one cell, keeping Ratatui's layout aligned.
//!
//! A terminal app can't ship a font — the user picks the font in their terminal's settings. The
//! Android app (phase 3) will bundle the font in the APK so icons always render there.

/// Which set of glyphs to use. Saved in settings as `"nerd"`, `"unicode"`, or `"ascii"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconMode {
    Nerd,
    Unicode,
    Ascii,
}

impl IconMode {
    /// All modes, in the order the UI cycles through them.
    pub const ALL: [IconMode; 3] = [IconMode::Nerd, IconMode::Unicode, IconMode::Ascii];

    /// The text stored in the settings table.
    pub fn as_str(self) -> &'static str {
        match self {
            IconMode::Nerd => "nerd",
            IconMode::Unicode => "unicode",
            IconMode::Ascii => "ascii",
        }
    }

    /// Reads a mode from settings text; `None` if unknown.
    pub fn parse(s: &str) -> Option<IconMode> {
        match s {
            "nerd" => Some(IconMode::Nerd),
            "unicode" => Some(IconMode::Unicode),
            "ascii" => Some(IconMode::Ascii),
            _ => None,
        }
    }

    /// The next mode in the cycle (Nerd -> Unicode -> ASCII -> Nerd).
    pub fn next(self) -> IconMode {
        match self {
            IconMode::Nerd => IconMode::Unicode,
            IconMode::Unicode => IconMode::Ascii,
            IconMode::Ascii => IconMode::Nerd,
        }
    }
}

/// The name of every icon the app uses.
///
/// Future categories/tags can get their icons by adding variants here (plus, later, a
/// user-editable mapping stored in settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Marker in front of every open task.
    Task,
    /// Marker for a completed task.
    Done,
    /// Marker in front of every event.
    Event,
    /// Shown next to the repeat label ("↻ yearly").
    Repeat,
    PriorityLow,
    PriorityMedium,
    PriorityHigh,
    /// Shown on overdue tasks.
    Overdue,
    /// The stats screen.
    Stats,
    /// The settings / theme picker.
    Settings,
    /// Sync in progress (and the generic "sync" marker).
    Sync,
    /// Last sync succeeded.
    Synced,
    /// Last sync failed.
    SyncError,
    /// The sync server couldn't be reached.
    Offline,
    /// Reminders (the `n` settings popup).
    Reminder,
}

impl Icon {
    /// The icon table: (Nerd Font glyph, Unicode fallback, ASCII fallback) for each icon.
    ///
    /// Written as a `match` rather than a list so the compiler refuses to build if an icon is
    /// missing a row — you can't add an `Icon` variant and forget its glyphs.
    ///
    /// The Nerd glyphs are written as `\u{f096}` escapes because most editors can't display
    /// them; the comment names each one (these names are searchable at nerdfonts.com/cheat-sheet).
    fn glyphs(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Icon::Task => ("\u{f096}", "☐", "[ ]"),      // nf-fa-square_o
            Icon::Done => ("\u{f046}", "☑", "[x]"),      // nf-fa-check_square_o
            Icon::Event => ("\u{f073}", "◷", "(o)"),     // nf-fa-calendar
            Icon::Repeat => ("\u{f01e}", "↻", "~"),      // nf-fa-repeat
            Icon::PriorityLow => ("\u{f063}", "▼", "v"), // nf-fa-arrow_down
            Icon::PriorityMedium => ("\u{f111}", "●", "-"), // nf-fa-circle
            Icon::PriorityHigh => ("\u{f062}", "▲", "^"), // nf-fa-arrow_up
            Icon::Overdue => ("\u{f071}", "!", "!"),     // nf-fa-exclamation_triangle
            Icon::Stats => ("\u{f080}", "▤", "#"),       // nf-fa-bar_chart
            Icon::Settings => ("\u{f013}", "≡", "*"),    // nf-fa-cog
            Icon::Sync => ("\u{f021}", "⇅", "<>"),       // nf-fa-refresh
            Icon::Synced => ("\u{f0c2}", "✓", "ok"),     // nf-fa-cloud
            Icon::SyncError => ("\u{f06a}", "✗", "!!"),  // nf-fa-exclamation_circle
            Icon::Offline => ("\u{f127}", "○", "--"),    // nf-fa-chain_broken
            Icon::Reminder => ("\u{f0f3}", "♪", "(!)"),  // nf-fa-bell
        }
    }

    /// The glyph to draw for this icon in the given mode.
    pub fn get(self, mode: IconMode) -> &'static str {
        // Destructure the 3-tuple into three named variables.
        let (nerd, unicode, ascii) = self.glyphs();
        match mode {
            IconMode::Nerd => nerd,
            IconMode::Unicode => unicode,
            IconMode::Ascii => ascii,
        }
    }
}

/// Convenience: the priority marker icon for a priority.
pub fn priority_icon(priority: crate::model::Priority) -> Icon {
    use crate::model::Priority;
    match priority {
        Priority::Low => Icon::PriorityLow,
        Priority::Medium => Icon::PriorityMedium,
        Priority::High => Icon::PriorityHigh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ICONS: [Icon; 15] = [
        Icon::Task,
        Icon::Done,
        Icon::Event,
        Icon::Repeat,
        Icon::PriorityLow,
        Icon::PriorityMedium,
        Icon::PriorityHigh,
        Icon::Overdue,
        Icon::Stats,
        Icon::Settings,
        Icon::Sync,
        Icon::Synced,
        Icon::SyncError,
        Icon::Offline,
        Icon::Reminder,
    ];

    #[test]
    fn ascii_mode_is_pure_ascii() {
        for icon in ALL_ICONS {
            assert!(icon.get(IconMode::Ascii).is_ascii(), "{icon:?}");
        }
    }

    #[test]
    fn nerd_and_unicode_are_single_characters() {
        // One character each keeps columns predictable (Nerd *Mono* fonts render them 1 cell).
        for icon in ALL_ICONS {
            assert_eq!(icon.get(IconMode::Nerd).chars().count(), 1, "{icon:?}");
            assert_eq!(icon.get(IconMode::Unicode).chars().count(), 1, "{icon:?}");
        }
    }

    #[test]
    fn task_and_event_markers_differ_in_every_mode() {
        for mode in IconMode::ALL {
            assert_ne!(Icon::Task.get(mode), Icon::Event.get(mode));
        }
    }

    #[test]
    fn mode_round_trip_and_cycle() {
        for mode in IconMode::ALL {
            assert_eq!(IconMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(IconMode::Ascii.next(), IconMode::Nerd);
    }
}
