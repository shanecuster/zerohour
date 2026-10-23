//! zerohour — a terminal to-do list and countdown timer.
//!
//! This file is the program's entry point. It:
//! 1. reads command-line flags (`--db <path>`, `--demo`) and, for `zerohour remind`, hands
//!    over to [`run_remind`] (which never touches the terminal),
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
mod notify;
mod remind;
#[cfg(feature = "sync")]
mod sync;
mod ui;

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Stdout, stdout};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use chrono::{Local, TimeDelta, Utc};
use clap::{Parser, Subcommand};
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
    // `global = true`: also accepted after a subcommand (`zerohour remind --db x.db`).
    #[arg(long, value_name = "PATH", global = true)]
    db: Option<PathBuf>,

    /// Start with sample data so you can see the layout. Nothing is saved
    /// (unless combined with --db, which seeds that file).
    #[arg(long)]
    demo: bool,

    /// What to do instead of opening the app (`None` = open the app).
    #[command(subcommand)]
    command: Option<Command>,
}

/// The subcommands. Each variant becomes one (`Remind` -> `zerohour remind`), and its doc
/// comment becomes the `--help` text.
#[derive(Subcommand, Debug)]
enum Command {
    /// Show the reminders that are due now as desktop notifications, then exit. Meant to run
    /// every minute from a systemd user timer.
    Remind(RemindArgs),
}

/// Flags for `zerohour remind`.
#[derive(clap::Args, Debug)]
struct RemindArgs {
    /// Only print what would be shown now and what is planned next; show and record nothing.
    #[arg(long)]
    dry_run: bool,
}

/// The real `main`. Returning `ExitCode` lets us print our own friendly error message and exit
/// with status 1, instead of Rust's default debug-style error output.
fn main() -> ExitCode {
    let args = Args::parse();
    // `zerohour remind` runs on its own and never opens the app; no subcommand opens the app.
    let result = match &args.command {
        Some(Command::Remind(remind)) => run_remind(&args, remind),
        None => run(args),
    };
    match result {
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

    let path = db_path(args)?;
    // The DbError message already names the file and the problem.
    let db = Db::open(&path)?;
    if args.demo {
        demo::seed(&db, local_now(), &Local).context("couldn't load the demo data")?;
        return Ok((db, None));
    }
    Ok((db, Some(path)))
}

/// The database file to use: `--db`, or the default location.
fn db_path(args: &Args) -> Result<PathBuf> {
    match &args.db {
        Some(path) => Ok(path.clone()),
        None => default_db_path().context(
            "couldn't find your home folder to store data; pass --db <path> to choose a file",
        ),
    }
}

/// `zerohour remind`: shows the reminders due now (see `remind.rs`), or with `--dry-run`
/// prints them. Never sets up the terminal, never syncs, and exits quickly, so it is safe to
/// run while the app is open.
///
/// Exits with success when the notifications couldn't be shown (no D-Bus session, no
/// notification daemon): that just means nobody is at this desktop, and the next run tries
/// again. Only a database or flag problem is an error.
fn run_remind(args: &Args, remind: &RemindArgs) -> Result<()> {
    if args.demo {
        bail!("`remind` works on a real database; it can't be combined with --demo");
    }
    let path = db_path(args)?;
    let db = Db::open(&path)?;
    // Keep the lock until this function returns (`_lock` is dropped, and unlocked, then).
    let Some(_lock) = lock_beside(&path)? else {
        eprintln!("{APP_NAME}: another `{APP_NAME} remind` is running; skipping this one");
        return Ok(());
    };

    // The two local times the window is built from (see `remind::window`): now, and the
    // real moment GRACE_MINUTES ago, each as shown on this computer's clock.
    let now = Utc::now();
    let local_now = now.with_timezone(&Local).naive_local();
    let local_grace_ago = (now - TimeDelta::minutes(remind::GRACE_MINUTES))
        .with_timezone(&Local)
        .naive_local();

    if remind.dry_run {
        println!(
            "{}",
            remind::preview(&db, local_now, local_grace_ago)?.text()
        );
        return Ok(());
    }

    let outcome = remind::run_once(
        &db,
        &mut notify::NotifySend,
        local_now,
        local_grace_ago,
        now,
    )?;
    // One line per notification, for `journalctl --user -u zerohour-remind`. A quiet run
    // (the usual case) prints nothing.
    for reminder in &outcome.sent {
        println!("shown: {}: {}", reminder.title, reminder.body);
    }
    for (reminder, reason) in &outcome.failed {
        eprintln!(
            "{APP_NAME}: couldn't show \"{}\" ({reason}); trying again on the next run",
            reminder.title
        );
    }
    Ok(())
}

/// Takes the `zerohour remind` lock: a file next to the database (`zerohour.db` ->
/// `zerohour.db.remind-lock`) that only one run at a time can hold. Without it, a run started
/// by hand at the same moment as the timer's could show a reminder twice, since each would
/// check "already shown?" before the other recorded it.
///
/// Returns `None` when another run holds the lock. The operating system releases the lock
/// when the file is closed, even if the process crashes, so a stale lock can't block later
/// runs. The file itself stays (it's empty) and is reused.
fn lock_beside(db_path: &Path) -> Result<Option<File>> {
    let mut name = db_path.as_os_str().to_owned();
    name.push(".remind-lock");
    let lock_path = PathBuf::from(name);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("couldn't open the lock file {}", lock_path.display()))?;
    // `try_lock` never waits: it either gets the lock now or says someone else has it.
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("couldn't lock {}", lock_path.display()))
        }
    }
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
            command: None,
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
            command: None,
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

    /// `--db` is accepted before and after `remind`, and `--demo` is refused with it.
    #[test]
    fn remind_takes_db_either_side_and_refuses_demo() {
        for argv in [
            ["zerohour", "remind", "--db", "x.db"],
            ["zerohour", "--db", "x.db", "remind"],
        ] {
            let args = Args::try_parse_from(argv).unwrap();
            assert_eq!(args.db.as_deref(), Some(Path::new("x.db")));
            assert!(matches!(args.command, Some(Command::Remind(_))));
        }
        let args = Args::try_parse_from(["zerohour", "--demo", "remind", "--dry-run"]).unwrap();
        let Some(Command::Remind(remind)) = &args.command else {
            panic!("expected remind");
        };
        assert!(remind.dry_run);
        assert!(run_remind(&args, remind).is_err());
    }

    /// Only one `remind` run at a time holds the lock; it's free again once dropped.
    #[test]
    fn the_remind_lock_is_held_by_one_run_at_a_time() {
        let dir = std::env::temp_dir().join(format!("zerohour-lock-test-{}", uuid_like()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("z.db");
        let first = lock_beside(&db).unwrap();
        assert!(first.is_some());
        assert!(dir.join("z.db.remind-lock").exists());
        assert!(lock_beside(&db).unwrap().is_none(), "second run must skip");
        drop(first);
        assert!(lock_beside(&db).unwrap().is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A dry run on a fresh database file prints and records nothing, and upgrades it.
    #[test]
    fn a_dry_run_on_a_new_file_works() {
        let dir = std::env::temp_dir().join(format!("zerohour-remind-test-{}", uuid_like()));
        let file = dir.join("z.db");
        let args = Args::try_parse_from([
            "zerohour",
            "remind",
            "--dry-run",
            "--db",
            file.to_str().unwrap(),
        ])
        .unwrap();
        let Some(Command::Remind(remind)) = &args.command else {
            panic!("expected remind");
        };
        run_remind(&args, remind).unwrap();
        assert_eq!(Db::open(&file).unwrap().schema_version().unwrap(), 4);
        std::fs::remove_dir_all(&dir).unwrap();
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
