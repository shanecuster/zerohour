//! The sync settings popup (`c`): server URL, database, username, password, an on/off switch,
//! and the actions "Save", "Test connection" and "Reset sync state".
//!
//! Only compiled with the `sync` cargo feature.
//!
//! Like the add form, this file owns the popup's state ([`SyncForm`]), its keys
//! ([`SyncForm::handle_key`]) and its drawing ([`draw`]). The app only sees a
//! [`SyncFormOutcome`] and does the actual work (saving, starting background jobs).
//!
//! The password field starts empty. Leaving it empty keeps the saved password; typing one
//! replaces it. It's always drawn as dots.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;
use zerohour_core::icons::{Icon, IconMode};
use zerohour_core::sync::SyncSettings;
use zerohour_core::theme::Theme;

use super::{
    centered, color, popup_block, popup_height, popup_text_width, tail_fit, wrap_text,
    wrapped_lines,
};
use crate::sync::{SyncState, SyncUi};

/// One row of the form, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Enabled,
    Url,
    Database,
    Username,
    Password,
    Save,
    Test,
    Reset,
}

impl Row {
    /// Every row in order (focus moves through this list).
    const ALL: [Row; 8] = [
        Row::Enabled,
        Row::Url,
        Row::Database,
        Row::Username,
        Row::Password,
        Row::Save,
        Row::Test,
        Row::Reset,
    ];

    /// The label on the left (inputs) or the button text (actions).
    fn label(self) -> &'static str {
        match self {
            Row::Enabled => "Sync",
            Row::Url => "Server URL",
            Row::Database => "Database",
            Row::Username => "Username",
            Row::Password => "Password",
            Row::Save => "Save",
            Row::Test => "Test connection",
            Row::Reset => "Reset sync state…",
        }
    }

    /// True for the button rows at the bottom.
    fn is_action(self) -> bool {
        matches!(self, Row::Save | Row::Test | Row::Reset)
    }
}

/// What the app should do after a key press in the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncFormOutcome {
    /// Nothing; keep the form open.
    Continue,
    /// Close without saving.
    Close,
    /// Save these settings, plus the new password if one was typed.
    Save {
        settings: SyncSettings,
        password: Option<String>,
    },
    /// Test these (possibly unsaved) settings, with the typed password if any.
    Test {
        settings: SyncSettings,
        password: Option<String>,
    },
    /// The user confirmed "reset sync state".
    Reset,
}

/// The state of the sync settings popup.
#[derive(Debug, Clone)]
pub struct SyncForm {
    pub enabled: bool,
    pub url: String,
    pub database: String,
    pub username: String,
    /// Newly typed password; empty = keep the saved one.
    pub password: String,
    /// Index into [`Row::ALL`].
    pub focus: usize,
    /// True while asking "really reset?".
    pub confirm_reset: bool,
    /// A message under the form (validation problems, "saved"...): (text, is_error).
    pub message: Option<(String, bool)>,
}

impl SyncForm {
    /// A form filled with the saved settings.
    pub fn new(settings: &SyncSettings) -> SyncForm {
        SyncForm {
            enabled: settings.enabled,
            url: settings.url.clone(),
            database: settings.database.clone(),
            username: settings.username.clone(),
            password: String::new(),
            focus: 0,
            confirm_reset: false,
            message: None,
        }
    }

    /// The settings as currently typed into the form.
    pub fn settings(&self) -> SyncSettings {
        SyncSettings {
            enabled: self.enabled,
            url: self.url.trim().to_string(),
            database: self.database.trim().to_string(),
            username: self.username.trim().to_string(),
        }
    }

    /// The typed password, or `None` if the field is empty ("keep the saved one").
    fn typed_password(&self) -> Option<String> {
        (!self.password.is_empty()).then(|| self.password.clone())
    }

    /// The focused row.
    fn focused(&self) -> Row {
        Row::ALL[self.focus]
    }

    /// The text behind an input row, or `None` for the switch and the buttons.
    fn text_mut(&mut self, row: Row) -> Option<&mut String> {
        match row {
            Row::Url => Some(&mut self.url),
            Row::Database => Some(&mut self.database),
            Row::Username => Some(&mut self.username),
            Row::Password => Some(&mut self.password),
            _ => None,
        }
    }

    /// Handles one key press.
    pub fn handle_key(&mut self, key: KeyEvent) -> SyncFormOutcome {
        // While confirming a reset, only y/n (or Enter/Esc) mean anything.
        if self.confirm_reset {
            self.confirm_reset = false;
            return match key.code {
                KeyCode::Char('y') | KeyCode::Enter => SyncFormOutcome::Reset,
                _ => {
                    self.message = Some(("Reset cancelled.".into(), false));
                    SyncFormOutcome::Continue
                }
            };
        }

        let row = self.focused();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = Row::ALL.len();
        match key.code {
            KeyCode::Esc => return SyncFormOutcome::Close,
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % count,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + count - 1) % count,
            // Space flips the switch; on other rows it's just a character (or nothing).
            KeyCode::Char(' ') if row == Row::Enabled => self.enabled = !self.enabled,
            KeyCode::Enter => match row {
                Row::Enabled => self.enabled = !self.enabled,
                Row::Save => return self.save(),
                Row::Test => {
                    self.message = None;
                    return SyncFormOutcome::Test {
                        settings: self.settings(),
                        password: self.typed_password(),
                    };
                }
                Row::Reset => self.confirm_reset = true,
                // Enter in a text field moves on to the next row, like in most forms.
                _ => self.focus = (self.focus + 1) % count,
            },
            KeyCode::Char('u') if ctrl => {
                if let Some(text) = self.text_mut(row) {
                    text.clear();
                }
            }
            KeyCode::Backspace => {
                if let Some(text) = self.text_mut(row) {
                    text.pop();
                }
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(text) = self.text_mut(row) {
                    text.push(c);
                }
            }
            _ => {}
        }
        SyncFormOutcome::Continue
    }

    /// "Save": checks the settings first if sync is being switched on.
    fn save(&mut self) -> SyncFormOutcome {
        let settings = self.settings();
        // Switched off, half-filled settings are fine to keep for later.
        if settings.enabled
            && let Err(error) = settings.validate()
        {
            self.message = Some((error.to_string(), true));
            return SyncFormOutcome::Continue;
        }
        SyncFormOutcome::Save {
            settings,
            password: self.typed_password(),
        }
    }
}

/// Draws the popup. `sync` supplies the live status (storage, test result, last sync).
///
/// Everything long (the storage path, status, warnings, messages) wraps to the popup's width,
/// and the popup is as tall as its lines, so nothing is cut off on a narrow terminal.
pub fn draw(frame: &mut Frame, form: &SyncForm, sync: &SyncUi, theme: &Theme, icons: IconMode) {
    let width = 74.min(frame.area().width);
    let text_width = popup_text_width(width);

    const LABEL_WIDTH: usize = 11;
    let value_width = text_width.saturating_sub(LABEL_WIDTH + 3);
    let accent = Style::new()
        .fg(color(theme.accent))
        .add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(color(theme.muted));
    let warn = Style::new().fg(color(theme.warn));

    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in Row::ALL.iter().enumerate() {
        let focused = i == form.focus;
        let marker = Span::styled(if focused { "› " } else { "  " }, accent);
        if row.is_action() {
            // Buttons: a blank line before the first one, then "[ Save ]" etc.
            if *row == Row::Save {
                lines.push(Line::default());
            }
            let style = if focused {
                accent
            } else {
                Style::new().fg(color(theme.fg))
            };
            lines.push(Line::from(vec![
                marker,
                Span::styled(format!("[ {} ]", row.label()), style),
            ]));
            continue;
        }
        let label_style = if focused { accent } else { muted };
        let value = input_value(form, sync, *row, focused, value_width);
        // A value that's too wide (e.g. the on/off explanation on a narrow screen) wraps
        // under the value column; typed text is already cut to fit by `input_value`.
        let value_style = Style::new().fg(color(theme.fg));
        // Only wrap when needed: wrapping joins words with single spaces, which mustn't
        // change what you typed.
        let parts = if value.width() > value_width {
            wrap_text(&value, value_width, 0)
        } else {
            vec![value]
        };
        let mut parts = parts.into_iter();
        lines.push(Line::from(vec![
            marker,
            Span::styled(format!("{:<LABEL_WIDTH$} ", row.label()), label_style),
            Span::styled(parts.next().unwrap_or_default(), value_style),
        ]));
        for part in parts {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(LABEL_WIDTH + 3)),
                Span::styled(part, value_style),
            ]));
        }
    }

    lines.push(Line::default());
    // Where the password lives, and the sync status.
    let storage = sync.storage.as_deref().unwrap_or("checking…");
    // Continuation lines are indented 4 so they line up after "  ⚠ " / under the text.
    lines.extend(wrapped_lines(
        &format!("  Password storage: {storage}"),
        text_width,
        4,
        muted,
    ));
    lines.extend(wrapped_lines(
        &format!(
            "  Status: {} · {} documents synced",
            state_text(&sync.state),
            sync.synced_docs
        ),
        text_width,
        4,
        muted,
    ));
    if form.settings().is_plain_http() {
        lines.extend(wrapped_lines(
            "  ⚠ http:// sends your password and data unencrypted, unless the network itself \
             is encrypted (Tailscale, VPN). Prefer https://.",
            text_width,
            4,
            warn,
        ));
    }
    if let Some(seconds) = sync.clock_skew_secs {
        let direction = if seconds > 0 { "ahead of" } else { "behind" };
        lines.extend(wrapped_lines(
            &format!(
                "  ⚠ This clock is {} min {direction} the server's. The newest edit wins a \
                 conflict, so fix the clock (enable NTP).",
                (seconds.abs() + 59) / 60
            ),
            text_width,
            4,
            warn,
        ));
    }

    // The newest message wins: confirmation question, form message, or test result.
    let (text, slot) = if form.confirm_reset {
        (
            "Forget which versions the server has? Your tasks stay; the next sync re-checks \
             everything. y = reset, n = cancel"
                .to_string(),
            theme.warn,
        )
    } else if let Some((text, is_error)) = &form.message {
        (
            text.clone(),
            if *is_error { theme.danger } else { theme.ok },
        )
    } else if let Some((ok, text)) = &sync.test_result {
        (text.clone(), if *ok { theme.ok } else { theme.danger })
    } else if sync.busy() {
        ("Working…".to_string(), theme.muted)
    } else {
        (String::new(), theme.muted)
    };
    lines.extend(wrapped_lines(
        &format!("  {text}"),
        text_width,
        2,
        Style::new().fg(color(slot)),
    ));
    lines.extend(wrapped_lines(
        "  ↑↓ move · Space on/off · Enter select · Esc close",
        text_width,
        2,
        muted,
    ));

    let area = centered(frame.area(), width, popup_height(lines.len()));
    let title = format!(" {} Sync settings ", Icon::Sync.get(icons));
    let block = popup_block(&title, theme, color(theme.accent));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The text shown for one input row.
fn input_value(form: &SyncForm, sync: &SyncUi, row: Row, focused: bool, width: usize) -> String {
    let cursor = if focused { "▏" } else { "" };
    let fit = |text: &str| format!("{}{cursor}", tail_fit(text, width.saturating_sub(1)));
    match row {
        Row::Enabled => {
            let state = if form.enabled { "[x] on" } else { "[ ] off" };
            format!("{state}   (off = this device stays local-only)")
        }
        Row::Url if form.url.is_empty() && !focused => {
            "e.g. https://couchdb.example.ts.net".to_string()
        }
        Row::Url => fit(&form.url),
        Row::Database => fit(&form.database),
        Row::Username => fit(&form.username),
        Row::Password => {
            // Never draw the password itself: one dot per typed character.
            let dots = "•".repeat(form.password.chars().count());
            let saved = match sync.has_password {
                _ if !form.password.is_empty() => "",
                Some(true) => "(saved; type to replace)",
                Some(false) => "(not saved)",
                None => "",
            };
            format!("{}{saved}", fit(&dots))
        }
        _ => String::new(),
    }
}

/// The status in words, for the settings popup.
pub fn state_text(state: &SyncState) -> String {
    match state {
        SyncState::Never => "never synced".to_string(),
        SyncState::Syncing => "syncing…".to_string(),
        SyncState::Synced(time) => format!(
            "last synced {}",
            time.with_timezone(&chrono::Local).format("%b %d %H:%M")
        ),
        SyncState::Offline => "offline (server not reachable)".to_string(),
        SyncState::Error(message) => format!("error: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(form: &mut SyncForm, text: &str) {
        for c in text.chars() {
            form.handle_key(key(KeyCode::Char(c)));
        }
    }

    /// Moves focus to `row`.
    fn focus(form: &mut SyncForm, row: Row) {
        form.focus = Row::ALL.iter().position(|r| *r == row).unwrap();
    }

    fn filled() -> SyncForm {
        let mut form = SyncForm::new(&SyncSettings::default());
        form.handle_key(key(KeyCode::Char(' '))); // switch on
        focus(&mut form, Row::Url);
        type_text(&mut form, "https://couchdb.example.ts.net");
        focus(&mut form, Row::Database);
        type_text(&mut form, "zerohour");
        focus(&mut form, Row::Username);
        type_text(&mut form, "tester");
        form
    }

    #[test]
    fn save_returns_the_typed_settings() {
        let mut form = filled();
        focus(&mut form, Row::Password);
        type_text(&mut form, "placeholder");
        focus(&mut form, Row::Save);
        match form.handle_key(key(KeyCode::Enter)) {
            SyncFormOutcome::Save { settings, password } => {
                assert!(settings.enabled);
                assert_eq!(settings.url, "https://couchdb.example.ts.net");
                assert_eq!(password.as_deref(), Some("placeholder"));
            }
            other => panic!("expected save, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_password_field_keeps_the_saved_one() {
        let mut form = filled();
        focus(&mut form, Row::Save);
        assert!(matches!(
            form.handle_key(key(KeyCode::Enter)),
            SyncFormOutcome::Save { password: None, .. }
        ));
    }

    #[test]
    fn invalid_settings_are_not_saved_when_switched_on() {
        let mut form = filled();
        form.url = "couchdb.example.ts.net".into(); // no scheme
        focus(&mut form, Row::Save);
        assert_eq!(
            form.handle_key(key(KeyCode::Enter)),
            SyncFormOutcome::Continue
        );
        assert!(form.message.as_ref().is_some_and(|(_, is_error)| *is_error));

        // Switched off, unfinished settings can be kept.
        form.enabled = false;
        assert!(matches!(
            form.handle_key(key(KeyCode::Enter)),
            SyncFormOutcome::Save { .. }
        ));
    }

    #[test]
    fn reset_needs_confirmation() {
        let mut form = filled();
        focus(&mut form, Row::Reset);
        assert_eq!(
            form.handle_key(key(KeyCode::Enter)),
            SyncFormOutcome::Continue
        );
        assert!(form.confirm_reset);
        assert_eq!(
            form.handle_key(key(KeyCode::Char('n'))),
            SyncFormOutcome::Continue
        );
        assert!(!form.confirm_reset);
        form.handle_key(key(KeyCode::Enter));
        assert_eq!(
            form.handle_key(key(KeyCode::Char('y'))),
            SyncFormOutcome::Reset
        );
    }

    #[test]
    fn test_connection_uses_the_unsaved_values() {
        let mut form = filled();
        focus(&mut form, Row::Test);
        match form.handle_key(key(KeyCode::Enter)) {
            SyncFormOutcome::Test { settings, password } => {
                assert_eq!(settings.database, "zerohour");
                assert_eq!(password, None);
            }
            other => panic!("expected test, got {other:?}"),
        }
    }

    #[test]
    fn draws_at_any_size_and_never_shows_the_password() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use zerohour_core::db::Db;
        use zerohour_core::theme::THEMES;

        let db = Db::open_in_memory().unwrap();
        let sync = SyncUi::new(&db, Some("unused.db".into())).unwrap();
        let mut form = filled();
        form.url = "http://couchdb.example.ts.net".into();
        form.password = "placeholder".into();

        // Tiny terminals must not panic; the popup just shrinks.
        for (width, height) in [(100, 32), (40, 12), (8, 4)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw(frame, &form, &sync, &THEMES[0], IconMode::Unicode))
                .unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(
                !screen.contains("placeholder"),
                "password drawn as dots only"
            );
            if width == 100 {
                assert!(screen.contains("Server URL"));
                assert!(screen.contains("•••••••••••"));
                assert!(screen.contains("unencrypted"), "http:// warning shown");
                assert!(screen.contains("Reset sync state"));
            }
        }
    }

    #[test]
    fn nothing_in_the_popup_is_cut_off_at_any_width() {
        use crate::ui::test_support::{WIDTHS, assert_shows, screen_text};
        use zerohour_core::db::Db;
        use zerohour_core::theme::THEMES;

        let db = Db::open_in_memory().unwrap();
        let mut sync = SyncUi::new(&db, Some("unused.db".into())).unwrap();
        // Long values on purpose: a deep password-file path, an error status, clock skew.
        sync.storage =
            Some("file /home/someone/.config/zerohour/sync-password (only you can read it)".into());
        sync.state = SyncState::Error(
            "database not found; create it on the server first (zerohour never creates \
             databases)"
                .into(),
        );
        sync.clock_skew_secs = Some(300);
        let mut form = filled();
        form.url = "http://couchdb.example.ts.net:5984".into();
        form.confirm_reset = true;

        let expected = [
            "Sync",
            "[x] on (off = this device stays local-only)",
            "Server URL",
            "Database",
            "Username",
            "Password",
            "[ Save ]",
            "[ Test connection ]",
            "[ Reset sync state… ]",
            "Password storage: file /home/someone/.config/zerohour/sync-password (only you \
             can read it)",
            "Status: error: database not found; create it on the server first (zerohour \
             never creates databases) · 0 documents synced",
            "⚠ http:// sends your password and data unencrypted, unless the network itself \
             is encrypted (Tailscale, VPN). Prefer https://.",
            "⚠ This clock is 5 min ahead of the server's. The newest edit wins a conflict, \
             so fix the clock (enable NTP).",
            "Forget which versions the server has? Your tasks stay; the next sync re-checks \
             everything. y = reset, n = cancel",
            "↑↓ move · Space on/off · Enter select · Esc close",
        ];
        for width in WIDTHS {
            let screen = screen_text(width, 50, |frame| {
                draw(frame, &form, &sync, &THEMES[0], IconMode::Unicode);
            });
            assert_shows(&screen, width, &expected);
        }
    }

    #[test]
    fn esc_closes() {
        let mut form = filled();
        assert_eq!(form.handle_key(key(KeyCode::Esc)), SyncFormOutcome::Close);
    }
}
