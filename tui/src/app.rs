//! App state and what each key does.
//!
//! [`App`] holds everything the UI needs: the database handle, the sorted lists shown on screen,
//! which row is selected, which screen/popup is open, and the current theme and icon mode.
//!
//! The flow is always the same:
//! 1. `main.rs` reads a key and calls [`App::handle_key`].
//! 2. We change state (and write to the database if needed), then call [`App::refresh`] to
//!    reload the lists from the database.
//! 3. `main.rs` redraws the screen from the new state (see `ui/mod.rs`).
//!
//! Database errors never crash the app: they are shown as a red status message instead.

use anyhow::Result;
use chrono::{Local, NaiveDate, NaiveDateTime, Timelike, Utc};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use zerohour_core::calendar::WeekStart;
use zerohour_core::clock::TimeFormat;
use zerohour_core::db::{Db, settings_keys};
use zerohour_core::icons::IconMode;
use zerohour_core::model::{Item, Kind};
use zerohour_core::repeat::{days_until, event_is_visible, event_target};
use zerohour_core::stats::{self, Stats};
use zerohour_core::theme::{THEMES, Theme, theme_by_name, theme_index};

use crate::input::{Action, main_action};
#[cfg(feature = "sync")]
use crate::sync::{Job, SyncUi, WorkerMsg};
use crate::ui::add::{Form, FormData, FormOutcome, FormPrefs};
#[cfg(feature = "sync")]
use crate::ui::sync_settings::{SyncForm, SyncFormOutcome};

/// Which full screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Tasks + Events.
    Main,
    /// Statistics and charts.
    Stats,
}

/// How the main screen divides its height between the Tasks and Events boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Equal halves (the default), so neither section gets squeezed.
    Split,
    /// The Events box is sized to its rows (up to half the screen); Tasks get the rest.
    Fit,
}

impl LayoutMode {
    /// The text saved in the settings table.
    pub fn as_str(self) -> &'static str {
        match self {
            LayoutMode::Split => "split",
            LayoutMode::Fit => "fit",
        }
    }

    /// Reads the saved text; anything unknown (or nothing saved) means the default, `Split`.
    pub fn parse(s: Option<&str>) -> LayoutMode {
        match s {
            Some("fit") => LayoutMode::Fit,
            _ => LayoutMode::Split,
        }
    }

    /// The other mode (what `v` switches to).
    pub fn toggled(self) -> LayoutMode {
        match self {
            LayoutMode::Split => LayoutMode::Fit,
            LayoutMode::Fit => LayoutMode::Split,
        }
    }
}

/// Which popup (if any) is open on top of the screen. Only one at a time.
///
/// Variants carry the popup's own state, e.g. the theme picker remembers the theme that was
/// active when it opened, so `Esc` can put it back.
#[derive(Debug, Clone)]
pub enum Overlay {
    None,
    Help,
    /// The add/edit dialog.
    Form(Form),
    /// Theme picker: `selected` is being previewed live; `original` is restored on `Esc`.
    ThemePicker {
        selected: usize,
        original: usize,
    },
    /// "Really delete?" for the item with this id/title.
    ConfirmDelete {
        id: String,
        title: String,
    },
    /// First-run "do you see this icon?" question.
    IconCheck,
    /// The sync settings popup (only in builds with the `sync` feature).
    #[cfg(feature = "sync")]
    SyncSettings(SyncForm),
}

/// A one-line message shown on the bottom border (e.g. "Done: Pay rent").
#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    /// Errors are drawn in the theme's `danger` color.
    pub is_error: bool,
}

/// An event plus its precomputed countdown, so drawing doesn't redo date math.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub item: Item,
    /// The date being counted down to (rolled forward if repeating).
    pub target: Option<NaiveDate>,
    /// Days from today to `target`: 0 = today, negative = already passed.
    pub days: Option<i64>,
}

/// The whole application state.
pub struct App {
    /// Where everything is stored.
    db: Db,
    /// Today's local date. Refreshed each tick so the countdowns roll over at midnight.
    pub today: NaiveDate,
    /// The local date and time, refreshed every tick. Tasks with a due time turn overdue the
    /// minute it passes, without waiting for a key press.
    pub now: NaiveDateTime,
    /// 12-hour ("2:00 PM", the default) or 24-hour ("14:00") times; `T` switches.
    pub time_format: TimeFormat,
    /// Open tasks, sorted: highest priority first, then soonest due date.
    pub tasks: Vec<Item>,
    /// Visible events, sorted by days remaining.
    pub events: Vec<EventRow>,
    /// The selected row, counting tasks first then events (so j/k flows from one into the other).
    pub selected: usize,
    /// Scroll state of the Tasks table (ratatui keeps the scroll offset in here).
    pub task_table: TableState,
    /// Scroll state of the Events table.
    pub event_table: TableState,
    pub screen: Screen,
    pub overlay: Overlay,
    /// The active theme (a borrow of an entry in core's `THEMES` list).
    pub theme: &'static Theme,
    pub icon_mode: IconMode,
    /// Statistics, recomputed on every refresh.
    pub stats: Stats,
    pub status: Option<Status>,
    /// Also show one-off events whose date has passed (toggled with `p`).
    pub show_passed: bool,
    /// Split 50/50 or fit-to-content (toggled with `v`, saved in settings).
    pub layout: LayoutMode,
    /// First day of the week in the date picker (switched with `w` in the picker).
    pub week_start: WeekStart,
    /// Set to true to leave the main loop.
    pub should_quit: bool,
    /// Running with `--demo` (in-memory data, nothing is saved).
    pub demo: bool,
    /// Sync settings, status and background jobs (only with the `sync` feature).
    #[cfg(feature = "sync")]
    pub sync: SyncUi,
    /// Show a status message when the running sync finishes successfully. True for syncs
    /// you asked for; false for the quiet one at startup (errors are always shown).
    #[cfg(feature = "sync")]
    announce_sync: bool,
}

impl App {
    /// Builds the app from an open database: loads settings, then the lists.
    ///
    /// `db_path` is the database file (for sync's background connection); `None` in demo
    /// mode, which never syncs.
    pub fn new(db: Db, demo: bool, db_path: Option<std::path::PathBuf>) -> Result<App> {
        // Local-only builds have no use for the path. `let _ =` marks it as deliberately unused.
        #[cfg(not(feature = "sync"))]
        let _ = db_path;
        #[cfg(feature = "sync")]
        let sync = SyncUi::new(&db, db_path)?;
        let theme = theme_by_name(&db.get_setting(settings_keys::THEME)?.unwrap_or_default());
        // No saved icon mode means this is the first run: ask whether Nerd Font icons show up.
        let saved_icons = db.get_setting(settings_keys::ICON_MODE)?;
        let icon_mode = saved_icons
            .as_deref()
            .and_then(IconMode::parse)
            .unwrap_or(IconMode::Nerd);
        let overlay = if saved_icons.is_none() && !demo {
            Overlay::IconCheck
        } else {
            Overlay::None
        };

        // Read before `db` moves into the struct below (after a move, `db` can't be used here).
        let layout = LayoutMode::parse(db.get_setting(settings_keys::LAYOUT)?.as_deref());
        let week_start = WeekStart::parse(db.get_setting(settings_keys::WEEK_START)?.as_deref());
        let time_format = TimeFormat::parse(db.get_setting(settings_keys::TIME_FORMAT)?.as_deref());

        let mut app = App {
            db,
            today: local_today(),
            now: local_now(),
            time_format,
            tasks: Vec::new(),
            events: Vec::new(),
            selected: 0,
            task_table: TableState::default(),
            event_table: TableState::default(),
            screen: Screen::Main,
            overlay,
            theme,
            icon_mode,
            stats: Stats::default(),
            status: None,
            show_passed: false,
            layout,
            week_start,
            should_quit: false,
            demo,
            #[cfg(feature = "sync")]
            sync,
            #[cfg(feature = "sync")]
            announce_sync: false,
        };
        if demo {
            app.set_status("Demo mode: sample data, nothing is saved. Press ? for help.");
        }
        app.refresh()?;
        // With sync on, sync once at startup, in the background. The app is usable right
        // away; the lists update when the sync finishes. Offline? Just a header indicator.
        #[cfg(feature = "sync")]
        if app.sync.visible() {
            let settings = app.sync.settings.clone();
            app.sync.start(Job::Sync, settings);
        }
        Ok(app)
    }

    /// Reloads tasks, events and stats from the database and re-sorts them.
    ///
    /// Called after every change. Keeps the same item selected if it still exists.
    pub fn refresh(&mut self) -> Result<()> {
        let selected_id = self.selected_item().map(|item| item.id.clone());
        let today = self.today;
        let items = self.db.list_items()?;

        // Open tasks only: completed ones stay in the database as history (and in stats).
        let mut tasks: Vec<Item> = items
            .iter()
            .filter(|i| i.kind == Kind::Task && !i.is_completed())
            .cloned()
            .collect();
        // Sort by priority (High first), then due date (soonest first, no date last), then
        // title. `then_with` only looks at the next rule when the previous one is a tie.
        tasks.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| match (a.date, b.date) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                })
                // Same day: timed tasks first, earliest first; untimed ones after them.
                .then_with(|| match (a.due_time, b.due_time) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                })
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });

        let mut events: Vec<EventRow> = items
            .iter()
            .filter(|i| i.kind == Kind::Event)
            .filter(|i| self.show_passed || event_is_visible(i, today))
            .map(|i| {
                let target = event_target(i, today);
                EventRow {
                    item: i.clone(),
                    target,
                    days: target.map(|t| days_until(t, today)),
                }
            })
            .collect();
        // Soonest first; passed events (negative days) go to the bottom, most recent first.
        // `sort_by_key` sorts by a computed key; tuples compare element by element.
        events.sort_by_key(|e| match e.days {
            Some(d) if d >= 0 => (0, d),
            Some(d) => (1, -d),
            None => (2, 0),
        });

        let completions = self.db.list_completions()?;
        self.stats = stats::compute(&completions, &items, self.now, &Local);
        self.tasks = tasks;
        self.events = events;

        // Put the selection back on the same item, or keep it in range if that item is gone.
        match selected_id {
            Some(id) => self.select_id(&id),
            None => self.clamp_selection(),
        }
        Ok(())
    }

    /// Called by the main loop after every key and at least once a second. If the date
    /// changed (we passed midnight), reload so countdowns update and repeating events roll
    /// over. With sync, also picks up results from the background worker.
    pub fn tick(&mut self) {
        let now = local_now();
        // Reload when the minute changes (the overdue count in the stats can change as due
        // times pass) and, of course, at midnight.
        let changed = now.date() != self.today || now.minute() != self.now.minute();
        self.now = now;
        if changed {
            self.today = now.date();
            if let Err(e) = self.refresh() {
                self.set_error(e);
            }
        }
        #[cfg(feature = "sync")]
        if let Err(e) = self.poll_sync() {
            self.set_error(e);
        }
    }

    /// How long the main loop may wait for a key before calling [`App::tick`] again.
    /// Short while a sync job runs, so its result shows up promptly.
    pub fn poll_interval(&self) -> std::time::Duration {
        #[cfg(feature = "sync")]
        if self.sync.busy() {
            return std::time::Duration::from_millis(200);
        }
        std::time::Duration::from_secs(1)
    }

    /// Entry point for every key press. Errors become a status message instead of a crash.
    pub fn handle_key(&mut self, key: KeyEvent) {
        if let Err(e) = self.try_handle_key(key) {
            self.set_error(e);
        }
    }

    /// Routes the key to the open popup, or to the current screen if no popup is open.
    fn try_handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Ctrl+C quits from anywhere, even inside a popup.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return Ok(());
        }

        // `match &mut self.overlay` borrows the popup state mutably so it can be updated in
        // place. Each arm finishes using that borrow before calling other `self` methods —
        // Rust's borrow checker insists on that, which prevents a whole class of bugs where
        // state is changed from two places at once.
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Form(form) => {
                let outcome = form.handle_key(key, self.today);
                match outcome {
                    FormOutcome::Continue => {}
                    FormOutcome::Cancel => self.overlay = Overlay::None,
                    FormOutcome::Save(data) => {
                        self.overlay = Overlay::None;
                        self.save_form(data)?;
                    }
                    FormOutcome::WeekStartChanged(start) => {
                        self.week_start = start;
                        self.db
                            .set_setting(settings_keys::WEEK_START, start.as_str())?;
                    }
                }
                return Ok(());
            }
            Overlay::ThemePicker { selected, original } => {
                let (selected, original) = (*selected, *original);
                self.theme_picker_key(key, selected, original)?;
                return Ok(());
            }
            Overlay::ConfirmDelete { id, title } => {
                let (id, title) = (id.clone(), title.clone());
                match key.code {
                    KeyCode::Char('y') | KeyCode::Enter => {
                        self.overlay = Overlay::None;
                        self.db.delete_item(&id, Utc::now())?;
                        self.refresh()?;
                        self.set_status(format!("Deleted \"{title}\""));
                    }
                    KeyCode::Char('n') | KeyCode::Esc => self.overlay = Overlay::None,
                    _ => {}
                }
                return Ok(());
            }
            Overlay::IconCheck => {
                match key.code {
                    KeyCode::Char('y') => self.set_icon_mode(IconMode::Nerd)?,
                    KeyCode::Char('n') => self.set_icon_mode(IconMode::Unicode)?,
                    // Anything else: decide later (we'll ask again next start).
                    _ => {}
                }
                self.overlay = Overlay::None;
                return Ok(());
            }
            Overlay::Help => {
                // Any key closes help.
                self.overlay = Overlay::None;
                return Ok(());
            }
            #[cfg(feature = "sync")]
            Overlay::SyncSettings(form) => {
                let outcome = form.handle_key(key);
                self.sync_form_outcome(outcome)?;
                return Ok(());
            }
        }

        // No popup open: translate the key into an action for the current screen.
        if let Some(action) = main_action(key) {
            self.status = None; // a new action clears the old message
            self.handle_action(action)?;
        }
        Ok(())
    }

    /// Does what an [`Action`] asks for.
    fn handle_action(&mut self, action: Action) -> Result<()> {
        // These work on every screen.
        match action {
            Action::Quit => {
                self.should_quit = true;
                return Ok(());
            }
            Action::ToggleStats => {
                self.screen = match self.screen {
                    Screen::Main => Screen::Stats,
                    Screen::Stats => Screen::Main,
                };
                return Ok(());
            }
            Action::Theme => {
                let index = theme_index(self.theme.name);
                self.overlay = Overlay::ThemePicker {
                    selected: index,
                    original: index,
                };
                return Ok(());
            }
            Action::CycleIcons => {
                let next = self.icon_mode.next();
                self.set_icon_mode(next)?;
                self.set_status(format!("Icons: {}", next.as_str()));
                return Ok(());
            }
            Action::Help => {
                self.overlay = Overlay::Help;
                return Ok(());
            }
            #[cfg(feature = "sync")]
            Action::SyncSettings => {
                self.open_sync_settings();
                return Ok(());
            }
            #[cfg(feature = "sync")]
            Action::SyncNow => {
                self.sync_now();
                return Ok(());
            }
            _ => {}
        }

        // The rest only make sense on the main screen.
        if self.screen != Screen::Main {
            return Ok(());
        }
        let rows = self.row_count();
        match action {
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Down => {
                if self.selected + 1 < rows {
                    self.selected += 1;
                }
            }
            Action::Top => self.selected = 0,
            Action::Bottom => self.selected = rows.saturating_sub(1),
            Action::Add => self.overlay = Overlay::Form(Form::new_add(self.form_prefs())),
            Action::Edit => {
                if let Some(item) = self.selected_item() {
                    self.overlay = Overlay::Form(Form::edit(item, self.form_prefs()));
                }
            }
            Action::Complete => self.complete_selected()?,
            Action::Delete => {
                if let Some(item) = self.selected_item() {
                    self.overlay = Overlay::ConfirmDelete {
                        id: item.id.clone(),
                        title: item.title.clone(),
                    };
                }
            }
            Action::ToggleTimeFormat => {
                self.time_format = self.time_format.toggled();
                self.db
                    .set_setting(settings_keys::TIME_FORMAT, self.time_format.as_str())?;
                self.set_status(match self.time_format {
                    TimeFormat::Hour12 => "Times: 12-hour (2:00 PM)",
                    TimeFormat::Hour24 => "Times: 24-hour (14:00)",
                });
            }
            Action::ToggleLayout => {
                self.layout = self.layout.toggled();
                self.db
                    .set_setting(settings_keys::LAYOUT, self.layout.as_str())?;
                self.set_status(match self.layout {
                    LayoutMode::Split => "Layout: tasks and events share the screen 50/50",
                    LayoutMode::Fit => "Layout: events box fits its content",
                });
            }
            Action::TogglePassed => {
                self.show_passed = !self.show_passed;
                self.refresh()?;
                self.set_status(if self.show_passed {
                    "Showing passed events"
                } else {
                    "Hiding passed events"
                });
            }
            // Already handled above; listed so the compiler checks we covered every action.
            Action::Quit
            | Action::ToggleStats
            | Action::Theme
            | Action::CycleIcons
            | Action::Help => {}
            #[cfg(feature = "sync")]
            Action::SyncSettings | Action::SyncNow => {}
        }
        Ok(())
    }

    /// Keys inside the theme picker. Moving the selection previews the theme immediately.
    fn theme_picker_key(&mut self, key: KeyEvent, selected: usize, original: usize) -> Result<()> {
        let count = THEMES.len();
        let new_selected = match key.code {
            KeyCode::Up | KeyCode::Char('k') => (selected + count - 1) % count,
            KeyCode::Down | KeyCode::Char('j') => (selected + 1) % count,
            KeyCode::Char('i') => {
                self.set_icon_mode(self.icon_mode.next())?;
                selected
            }
            KeyCode::Enter => {
                // Keep it: save the choice.
                self.db
                    .set_setting(settings_keys::THEME, THEMES[selected].name)?;
                self.overlay = Overlay::None;
                self.set_status(format!("Theme: {}", THEMES[selected].name));
                return Ok(());
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('t') => {
                // Cancel: put the original theme back.
                self.theme = &THEMES[original];
                self.overlay = Overlay::None;
                return Ok(());
            }
            _ => selected,
        };
        self.theme = &THEMES[new_selected];
        self.overlay = Overlay::ThemePicker {
            selected: new_selected,
            original,
        };
        Ok(())
    }

    /// Writes the add/edit form's result to the database and selects the saved item.
    fn save_form(&mut self, data: FormData) -> Result<()> {
        let now = Utc::now();
        let id = match &data.editing {
            Some(id) => {
                // Load the stored item, overwrite the editable fields, save.
                let mut item = self
                    .db
                    .get_item(id)?
                    .ok_or_else(|| anyhow::anyhow!("that item no longer exists"))?;
                data.apply_to(&mut item);
                self.db.update_item(&mut item, now)?;
                self.set_status(format!("Saved \"{}\"", item.title));
                item.id
            }
            None => {
                let mut item = Item::new(data.kind, data.title.clone(), now);
                data.apply_to(&mut item);
                self.db.insert_item(&item)?;
                self.set_status(format!("Added \"{}\"", item.title));
                item.id
            }
        };
        self.refresh()?;
        self.select_id(&id);
        Ok(())
    }

    /// Completes the selected task. A repeating task gets a new instance with the next due
    /// date, which is then selected so you can see it appear.
    fn complete_selected(&mut self) -> Result<()> {
        let Some(item) = self.selected_item() else {
            return Ok(());
        };
        if item.kind == Kind::Event {
            self.set_status("Events count down on their own; there's nothing to complete.");
            return Ok(());
        }
        let (id, title) = (item.id.clone(), item.title.clone());
        let next = self.db.complete_task(&id, local_now(), Utc::now())?;
        self.refresh()?;
        match next {
            Some(next) => {
                let mut due = next
                    .date
                    .map(|d| d.format("%a %b %d").to_string())
                    .unwrap_or_default();
                if let Some(time) = next.due_time {
                    due.push(' ');
                    due.push_str(&zerohour_core::clock::format_time(time, self.time_format));
                }
                self.select_id(&next.id);
                self.set_status(format!("Done: \"{title}\". Next one due {due}"));
            }
            None => self.set_status(format!("Done: \"{title}\"")),
        }
        Ok(())
    }

    /// Opens the sync settings popup and asks the worker where passwords are stored.
    #[cfg(feature = "sync")]
    fn open_sync_settings(&mut self) {
        if self.sync.db_path.is_none() {
            self.set_status("Sync is off in demo mode, so sample data never reaches your server.");
            return;
        }
        self.overlay = Overlay::SyncSettings(SyncForm::new(&self.sync.settings));
        self.sync.test_result = None;
        let settings = self.sync.settings.clone();
        self.sync.start(Job::Probe, settings);
    }

    /// The `r` key: start a sync now (in the background), or say why not.
    #[cfg(feature = "sync")]
    fn sync_now(&mut self) {
        if self.sync.db_path.is_none() {
            self.set_status("Sync is off in demo mode, so sample data never reaches your server.");
        } else if !self.sync.settings.enabled {
            self.set_status("Sync is switched off. Press c to set it up.");
        } else if self.sync.busy() {
            self.set_status("Already syncing…");
        } else {
            let settings = self.sync.settings.clone();
            self.sync.start(Job::Sync, settings);
            self.announce_sync = true;
            self.set_status("Syncing…");
        }
    }

    /// Acts on what the user did in the sync settings popup.
    #[cfg(feature = "sync")]
    fn sync_form_outcome(&mut self, outcome: SyncFormOutcome) -> Result<()> {
        match outcome {
            SyncFormOutcome::Continue => {}
            SyncFormOutcome::Close => self.overlay = Overlay::None,
            SyncFormOutcome::Save { settings, password } => {
                if self.sync.busy() {
                    self.sync_form_message(
                        "A sync job is still running; try again in a moment.",
                        true,
                    );
                    return Ok(());
                }
                settings.save(&self.db)?;
                self.sync.settings_changed(settings.clone(), &self.db)?;
                if !settings.enabled {
                    // Switching off also clears the header indicator (`SyncUi::visible`).
                    self.sync_form_message(
                        "Saved. Sync is off: this device stays local-only.",
                        false,
                    );
                }
                if password.is_some() || settings.enabled {
                    if settings.enabled {
                        self.sync_form_message("Saved. Syncing…", false);
                        self.announce_sync = true;
                    }
                    let then_sync = settings.enabled;
                    self.sync.start(
                        Job::Save {
                            password,
                            then_sync,
                        },
                        settings,
                    );
                }
            }
            SyncFormOutcome::Test { settings, password } => {
                self.sync.test_result = None;
                if !self.sync.start(Job::Test { password }, settings) {
                    self.sync_form_message(
                        "A sync job is still running; try again in a moment.",
                        true,
                    );
                } else {
                    self.sync_form_message("Testing…", false);
                }
            }
            SyncFormOutcome::Reset => {
                if self.sync.busy() {
                    self.sync_form_message("A sync is running; reset when it's done.", true);
                    return Ok(());
                }
                self.db.clear_sync_state()?;
                self.sync.synced_docs = 0;
                self.sync_form_message(
                    "Sync state reset. Your data is untouched; the next sync re-checks everything.",
                    false,
                );
            }
        }
        Ok(())
    }

    /// Shows a message inside the sync settings popup, if it's open.
    #[cfg(feature = "sync")]
    fn sync_form_message(&mut self, text: &str, is_error: bool) {
        if let Overlay::SyncSettings(form) = &mut self.overlay {
            form.message = Some((text.to_string(), is_error));
        }
    }

    /// Reads results from the background sync worker and reacts to them.
    #[cfg(feature = "sync")]
    fn poll_sync(&mut self) -> Result<()> {
        for message in self.sync.poll() {
            match message {
                WorkerMsg::Synced(Ok(report)) => {
                    // Other devices' changes are in the file now; show them.
                    self.refresh()?;
                    self.sync.synced_docs = self.db.synced_doc_count()?;
                    let skew = report.clock_skew_secs.map(|s| {
                        format!(
                            " ⚠ This clock is {} min off the server's; fix it so the newest edit wins.",
                            (s.abs() + 59) / 60
                        )
                    });
                    if self.announce_sync || skew.is_some() {
                        self.set_status(format!(
                            "Synced: {} in, {} out.{}",
                            report.pulled,
                            report.pushed,
                            skew.unwrap_or_default()
                        ));
                    }
                    if report.skipped_newer > 0 {
                        self.set_error(anyhow::anyhow!(
                            "{} synced item(s) need a newer zerohour; update this device.",
                            report.skipped_newer
                        ));
                    }
                    self.sync_form_message("Saved and synced.", false);
                    self.announce_sync = false;
                }
                WorkerMsg::Synced(Err(error)) => {
                    // Shown in the header too; the status line says why.
                    let text = format!("Sync: {error}");
                    self.sync_form_message(&text, !error.is_offline());
                    if error.is_offline() {
                        self.set_status(text);
                    } else {
                        self.set_error(anyhow::anyhow!(text));
                    }
                    self.announce_sync = false;
                }
                WorkerMsg::PasswordSaved => self.sync_form_message("Password saved.", false),
                WorkerMsg::Failed(text) => self.sync_form_message(&text, true),
                // The result itself is stored in `self.sync.test_result`, which the popup draws;
                // clear the "Testing…" message so it doesn't cover the result.
                WorkerMsg::Tested(_) => {
                    if let Overlay::SyncSettings(form) = &mut self.overlay {
                        form.message = None;
                    }
                }
                // Only updates `self.sync`, which the popup draws from directly.
                WorkerMsg::Storage { .. } => {}
            }
        }
        Ok(())
    }

    /// Changes and saves the icon mode.
    fn set_icon_mode(&mut self, mode: IconMode) -> Result<()> {
        self.icon_mode = mode;
        self.db
            .set_setting(settings_keys::ICON_MODE, mode.as_str())?;
        Ok(())
    }

    /// The display settings the add/edit form needs.
    fn form_prefs(&self) -> FormPrefs {
        FormPrefs {
            week_start: self.week_start,
            time_format: self.time_format,
        }
    }

    /// Total rows on the main screen (tasks + events).
    pub fn row_count(&self) -> usize {
        self.tasks.len() + self.events.len()
    }

    /// The item under the selection, if there are any rows.
    pub fn selected_item(&self) -> Option<&Item> {
        if self.selected < self.tasks.len() {
            self.tasks.get(self.selected)
        } else {
            self.events
                .get(self.selected - self.tasks.len())
                .map(|row| &row.item)
        }
    }

    /// Moves the selection onto the item with this id (if it's visible).
    fn select_id(&mut self, id: &str) {
        let position = self.tasks.iter().position(|t| t.id == id).or_else(|| {
            self.events
                .iter()
                .position(|e| e.item.id == id)
                .map(|p| p + self.tasks.len())
        });
        if let Some(p) = position {
            self.selected = p;
        }
        self.clamp_selection();
    }

    /// Keeps `selected` within the list (e.g. after the last row was deleted).
    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.row_count().saturating_sub(1));
    }

    /// Shows a normal status message. `impl Into<String>` accepts both `&str` and `String`.
    fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            is_error: false,
        });
    }

    /// Shows an error as a status message. `{:#}` prints the error with its causes.
    fn set_error(&mut self, error: impl Into<anyhow::Error>) {
        self.status = Some(Status {
            text: format!("Error: {:#}", error.into()),
            is_error: true,
        });
    }
}

/// Today's date in the computer's local time zone.
pub fn local_today() -> NaiveDate {
    local_now().date()
}

/// The local date and time ("wall-clock" time, no time zone attached).
///
/// This is the ONE place the TUI asks the clock for the local time; everything in core
/// receives it as a parameter.
pub fn local_now() -> NaiveDateTime {
    Local::now().naive_local()
}
