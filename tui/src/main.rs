//! zerohour — a terminal to-do list and countdown timer.
//!
//! This file is the program's entry point. It:
//! 1. reads command-line flags (`--db <path>`, `--demo`),
//! 2. opens the database (with a friendly message if that fails — before touching the terminal),
//! 3. switches the terminal into "TUI mode" (raw mode + alternate screen),
//! 4. runs the main loop: draw → wait for a key → update state → repeat,
//! 5. ALWAYS puts the terminal back to normal on exit, on error, and even on a panic (crash).
//!
//! Why step 5 matters: in raw mode the terminal doesn't echo what you type or handle Enter
//! normally. If the app crashed without restoring it, your shell would look broken until you
//! ran `reset`.

mod app;
mod input;
#[cfg(feature = "sync")]
mod sync;
mod ui;

use std::io::{Stdout, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use chrono::Local;
use clap::Parser;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{cursor, execute};
use zerohour_core::APP_NAME;
use zerohour_core::db::{Db, default_db_path};
use zerohour_core::demo;

use crate::app::{App, local_now};

/// Command-line flags. `clap` reads the doc comments below and turns them into `--help` text.
#[derive(Parser, Debug)]
#[command(version, about = "A terminal to-do list and countdown timer.")]
struct Args {
    /// Use this database file instead of the default one (handy for testing).
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,

    /// Start with sample data so you can see the layout. Nothing is saved
    /// (unless combined with --db, which seeds that file).
    #[arg(long)]
    demo: bool,
}

/// The real `main`. Returning `ExitCode` lets us print our own friendly error message and exit
/// with status 1, instead of Rust's default debug-style error output.
fn main() -> ExitCode {
    let args = Args::parse();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // `{:#}` prints the error plus its "context" chain on one line, e.g.
            // "zerohour: can't open the database at /x/y.db: file is not a database".
            eprintln!("{APP_NAME}: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Opens the database, sets up the terminal, runs the app, and restores the terminal.
///
/// `Result<()>` + `?`: each `?` means "if this failed, stop here and return the error to the
/// caller". It's Rust's replacement for exceptions — errors are values you must deal with.
fn run(args: Args) -> Result<()> {
    // Everything that can fail in a "normal" way happens BEFORE we take over the terminal,
    // so error messages print cleanly in your shell.
    let (db, db_path) = open_database(&args)?;
    let mut app = App::new(db, args.demo, db_path)?;

    install_panic_hook();
    let mut terminal = setup_terminal().context("couldn't set up the terminal")?;
    // Run the loop, but don't `?` it yet: we must restore the terminal first, error or not.
    let result = main_loop(&mut terminal, &mut app);
    restore_terminal();
    result
}

/// Decides which database to open based on the flags.
///
/// Also returns the file's path for sync, or `None` in demo mode: sample data must never be
/// synced to a real server, even when `--demo` is combined with `--db`.
fn open_database(args: &Args) -> Result<(Db, Option<PathBuf>)> {
    // `--demo` without `--db`: a throwaway in-memory database filled with samples.
    if args.demo && args.db.is_none() {
        let db = Db::open_in_memory()?;
        demo::seed(&db, local_now(), &Local).context("couldn't load the demo data")?;
        return Ok((db, None));
    }

    let path = match &args.db {
        Some(path) => path.clone(),
        None => default_db_path().context(
            "couldn't find your home folder to store data; pass --db <path> to choose a file",
        )?,
    };
    // The DbError message already names the file and the problem.
    let db = Db::open(&path)?;
    if args.demo {
        demo::seed(&db, local_now(), &Local).context("couldn't load the demo data")?;
        return Ok((db, None));
    }
    Ok((db, Some(path)))
}

/// The main loop: draw, wait up to a second for input, handle it, repeat until quit.
///
/// Waiting with a timeout (instead of blocking forever) lets `app.tick()` notice when the date
/// changes at midnight, so countdowns roll over even if you leave the app open.
fn main_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    // Whether the terminal currently sends us mouse events (see `App::wants_mouse`).
    let mut mouse_captured = false;
    while !app.should_quit {
        // Capture the mouse only while the date picker is open, so text selection with the
        // mouse works normally everywhere else.
        if app.wants_mouse() != mouse_captured {
            mouse_captured = !mouse_captured;
            if mouse_captured {
                execute!(stdout(), EnableMouseCapture)?;
            } else {
                execute!(stdout(), DisableMouseCapture)?;
            }
        }
        // `draw` gives us a `Frame` to paint on; the closure paints the whole UI.
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(app.poll_interval())? {
            match event::read()? {
                // Only react to key *presses*. Some terminals (notably on Windows) also send
                // key *release* events, which would otherwise trigger every action twice.
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                // A resize just needs a redraw, which the next loop iteration does.
                _ => {}
            }
        }
        app.tick();
    }
    Ok(())
}

/// Puts the terminal into TUI mode.
///
/// - Raw mode: keys arrive one at a time immediately (no waiting for Enter, no echo).
/// - Alternate screen: a separate screen buffer, so your shell history reappears untouched
///   when the app exits (like `vim` or `less`).
fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout()))?)
}

/// Undoes [`setup_terminal`]. Safe to call more than once.
///
/// Errors are ignored on purpose (`let _ =`): this runs while exiting or crashing, and there's
/// nothing useful to do if restoring fails — we just try every step.
fn restore_terminal() {
    let _ = disable_raw_mode();
    // Turning mouse capture off when it's already off is harmless, so always do it.
    let _ = execute!(
        stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen,
        cursor::Show
    );
}

/// Makes sure a panic (a crash from a bug) restores the terminal before printing its message.
///
/// A panic hook is a function Rust calls when a panic happens. We keep the default hook (which
/// prints the message and location) and run it *after* restoring, so the message is readable.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    // `Box::new(move |info| ...)`: the closure takes ownership (`move`) of `default_hook`, and is
    // boxed (put on the heap) because the hook can be any size of closure.
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--demo` on its own seeds a throwaway in-memory database (it never opens the default
    /// file) and returns no path, so sync can't start.
    #[test]
    fn demo_alone_stays_in_memory_and_never_syncs() {
        let args = Args {
            db: None,
            demo: true,
        };
        let (db, path) = open_database(&args).unwrap();
        assert!(path.is_none());
        assert!(!db.list_items().unwrap().is_empty());
    }

    /// `--demo --db <file>` seeds that file, but still returns no path: demo data never syncs.
    #[test]
    fn demo_with_a_file_never_syncs() {
        let file = std::env::temp_dir().join(format!("zerohour-demo-test-{}.db", uuid_like()));
        let args = Args {
            db: Some(file.clone()),
            demo: true,
        };
        let (_db, path) = open_database(&args).unwrap();
        assert!(path.is_none());
        let _ = std::fs::remove_file(&file);
    }

    /// The stats the app itself computes (what the screen draws) match the demo's design, and
    /// match what core's stats give for the same seeded data in the machine's time zone.
    #[test]
    fn app_shows_the_same_stats() {
        use zerohour_core::demo::STREAK_DAYS;
        use zerohour_core::stats::{self, WEEKS_SHOWN};

        let db = Db::open_in_memory().unwrap();
        let now = local_now();
        demo::seed(&db, now, &Local).unwrap();
        let app = App::new(db, true, None).unwrap();
        assert_eq!(app.stats.current_streak, STREAK_DAYS as u32);
        assert_eq!(app.stats.overdue_count, 1);
        assert_eq!(app.stats.per_week.len(), WEEKS_SHOWN);

        // The same demo, seeded as of the app's own "now", with stats computed directly.
        let other = Db::open_in_memory().unwrap();
        demo::seed(&other, app.now, &Local).unwrap();
        let items = other.list_items().unwrap();
        let completions = other.list_completions().unwrap();
        let expected = stats::compute(&completions, &items, app.now, &Local);
        assert_eq!(app.stats.total_all, expected.total_all);
        assert_eq!(app.stats.by_priority, expected.by_priority);
    }

    /// A unique-enough suffix for a temp file name (process id + nanoseconds).
    fn uuid_like() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        format!("{}-{nanos}", std::process::id())
    }
}
