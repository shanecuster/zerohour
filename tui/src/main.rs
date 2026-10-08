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
mod demo;
mod input;
mod ui;

use std::io::{Stdout, stdout};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{cursor, execute};
use zerohour_core::APP_NAME;
use zerohour_core::db::{Db, default_db_path};

use crate::app::{App, local_today};

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
    let db = open_database(&args)?;
    let mut app = App::new(db, args.demo)?;

    install_panic_hook();
    let mut terminal = setup_terminal().context("couldn't set up the terminal")?;
    // Run the loop, but don't `?` it yet: we must restore the terminal first, error or not.
    let result = main_loop(&mut terminal, &mut app);
    restore_terminal();
    result
}

/// Decides which database to open based on the flags.
fn open_database(args: &Args) -> Result<Db> {
    // `--demo` without `--db`: a throwaway in-memory database filled with samples.
    if args.demo && args.db.is_none() {
        let db = Db::open_in_memory()?;
        demo::seed(&db, local_today()).context("couldn't load the demo data")?;
        return Ok(db);
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
        demo::seed(&db, local_today()).context("couldn't load the demo data")?;
    }
    Ok(db)
}

/// The main loop: draw, wait up to a second for input, handle it, repeat until quit.
///
/// Waiting with a timeout (instead of blocking forever) lets `app.tick()` notice when the date
/// changes at midnight, so countdowns roll over even if you leave the app open.
fn main_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    while !app.should_quit {
        // `draw` gives us a `Frame` to paint on; the closure paints the whole UI.
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(Duration::from_secs(1))? {
            match event::read()? {
                // Only react to key *presses*. Some terminals (notably on Windows) also send
                // key *release* events, which would otherwise trigger every action twice.
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
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
    let _ = execute!(stdout(), LeaveAlternateScreen, cursor::Show);
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
