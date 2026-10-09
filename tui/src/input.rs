//! Key bindings for the main and stats screens.
//!
//! Translating raw key presses into named [`Action`]s here keeps all the bindings in one place:
//! to change a key, edit [`main_action`] and the help text in `ui/help.rs` — nothing else.
//!
//! Popups (the add form, theme picker, confirmations) read keys directly because their keys
//! depend on what's on screen (typing text into a field, for example).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Everything the user can ask for on the main/stats screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Move the selection up one row.
    Up,
    /// Move the selection down one row.
    Down,
    /// Jump to the first row.
    Top,
    /// Jump to the last row.
    Bottom,
    /// Open the add dialog.
    Add,
    /// Edit the selected item.
    Edit,
    /// Complete the selected task.
    Complete,
    /// Delete the selected item (asks first).
    Delete,
    /// Switch between the main screen and the stats screen.
    ToggleStats,
    /// Open the theme picker.
    Theme,
    /// Cycle icon mode: Nerd Font -> Unicode -> ASCII.
    CycleIcons,
    /// Show/hide one-off events whose date has passed.
    TogglePassed,
    /// Show the help overlay.
    Help,
    /// Open the sync settings (only in builds with the `sync` feature).
    #[cfg(feature = "sync")]
    SyncSettings,
    /// Leave the app.
    Quit,
}

/// Maps a key press to an action, or `None` if the key does nothing here.
///
/// `match` on a tuple lets us check the key and its modifiers (Ctrl, Shift...) together.
/// `KeyCode::Char('j') | KeyCode::Down` means "either of these".
pub fn main_action(key: KeyEvent) -> Option<Action> {
    // Ctrl+C always quits, like in most terminal programs.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(Action::Quit);
    }
    let action = match key.code {
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::Char('g') | KeyCode::Home => Action::Top,
        KeyCode::Char('G') | KeyCode::End => Action::Bottom,
        KeyCode::Char('a') => Action::Add,
        KeyCode::Char('e') | KeyCode::Enter => Action::Edit,
        KeyCode::Char(' ') | KeyCode::Char('x') => Action::Complete,
        KeyCode::Char('d') | KeyCode::Delete => Action::Delete,
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('s') => Action::ToggleStats,
        KeyCode::Char('t') => Action::Theme,
        KeyCode::Char('i') => Action::CycleIcons,
        KeyCode::Char('p') => Action::TogglePassed,
        KeyCode::Char('?') | KeyCode::F(1) => Action::Help,
        // `#[cfg]` on a match arm: in local-only builds this key simply does nothing, exactly
        // as before sync existed.
        #[cfg(feature = "sync")]
        KeyCode::Char('c') => Action::SyncSettings,
        // Esc deliberately does NOT quit: it's the "close this popup" key, and pressing it
        // one time too many shouldn't exit the app.
        KeyCode::Char('q') => Action::Quit,
        _ => return None,
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn vim_keys_and_arrows_both_move() {
        assert_eq!(main_action(press(KeyCode::Char('j'))), Some(Action::Down));
        assert_eq!(main_action(press(KeyCode::Down)), Some(Action::Down));
        assert_eq!(main_action(press(KeyCode::Char('k'))), Some(Action::Up));
        assert_eq!(main_action(press(KeyCode::Up)), Some(Action::Up));
    }

    #[test]
    fn ctrl_c_quits_and_unknown_keys_do_nothing() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(main_action(ctrl_c), Some(Action::Quit));
        assert_eq!(main_action(press(KeyCode::Char('z'))), None);
    }
}
