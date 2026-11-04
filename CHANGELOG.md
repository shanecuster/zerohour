# Changelog

All notable changes to zerohour. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **Nth-weekday repeats**: "the 4th Thursday of November" (Thanksgiving), "the last Monday of May",
  or "the 1st Friday of every month", for tasks and events. Choose *nth weekday* in the add/edit
  dialog, then the ordinal (1st to 4th, or last), weekday and month (or every month); the next two
  dates show underneath. A full date already in the date field fills the rows in. Unscheduled start
  dates move forward on save and empty dates are filled in, as with weekday repeats. Listed as
  "↻ 4th Thu of Nov" / "↻ every 1st Fri". Stored as `nth:4:thu:nov` / `nth:1:fri:monthly` in the
  existing column (no database change). Synced as document `v: 3`, so older builds skip just those
  items with the "need a newer zerohour" note; all other items keep their format.

- **Desktop reminders**: `zerohour remind` shows the reminders due now as desktop notifications
  (`notify-send`), each one once, and exits; a systemd user timer runs it every minute
  (`zerohour remind --print-units` prints the unit files; nothing is installed for you). It catches
  up on the last 15 minutes but skips anything older, so waking the computer never floods you, and
  tries again when no notification service is running. `--dry-run` shows what would be sent and
  what's next. `n` in the app opens the Reminders settings (on/off, early reminder, day time,
  overdue list) and sends a test notification. Off until switched on; settings stay on this
  computer. Database schema version 4 adds the never-synced `reminders_sent` table; existing data
  is unchanged.

- **Reminder planning in core** (used by the Android app and `zerohour remind`): tasks with a due time are reminded at that time, plus an optional earlier reminder;
  repeating ones on each scheduled day while still open; tasks due on a day without a time at a
  set time of day (09:00 by default); an optional daily list of overdue tasks. Off until switched
  on, set per device and never synced. No database change.

- **Better `--demo` data** for screenshots, all fictional and relative to today: 8 tasks (one due
  later today, one overdue, a Mon–Thu 2:00 PM "Submit weekly report", one with notes), 5 upcoming
  events (Christmas, a product launch, a monthly book club, a concert, a trip), and 12 weeks of
  completion history with a 9-day streak, a few late completions, and a mixed priority breakdown.
  Still in-memory unless combined with `--db`, and never synced.

- **Title banner**: a big "zerohour" at the top of the main screen, in a gradient from the theme's
  accent color to its orange (yellow in Gruvbox, whose accent is orange), with a tagline counting down to the next event ("7 days to
  Your Birthday", "Launch day is today!", or the date when there's none). Half-block letters, or
  plain ASCII in ASCII icon mode. Shows on terminals at least 30 rows tall, not in narrow mode;
  `b` switches it off and on (saved as `banner`, on by default).

- **Due times** on tasks (optional, local wall-clock time): "today 2:00 PM · in 3h", "tomorrow 9:30 AM",
  "today 2:00 PM" once the time has come. Overdue only once the due day is over, like a task without a
  time. 12-hour by default; `T` switches to 24-hour.
  Database schema version 3 adds the column; existing data is unchanged.
- **Weekday repeats**: repeat on chosen days, with presets (weekdays, Mon–Thu) and day checkboxes in the
  add/edit dialog. Unscheduled start dates move forward on save; empty dates are filled in. Works for events too.
- **Calendar picker** for the date field (Ctrl+D or F2): arrows or h/j/k/l, PgUp/PgDn, `t` for today,
  `w` for the week start (Sunday by default), Enter to pick. Mouse clicks and the scroll wheel work while it's open.
- **Layout**: Tasks and Events now share the screen 50/50 by default; `v` switches to fit-to-content (the old layout).
- `CLAUDE.md` with the project's working rules.

- **Optional CouchDB sync** between devices, in builds made with `--features sync`
  (`cargo install --path tui --features sync`). Off by default; local-only builds contain no network code.
  - Sync settings screen (`c`): server URL, database, username, password, on/off switch,
    *Test connection*, and *Reset sync state*.
  - `r` syncs now; with sync on, the app also syncs once at startup in the background.
  - Header indicator: never synced / syncing / synced HH:MM / offline / sync error.
    Offline or failed syncs never block the app.
  - Conflicts: the newest change wins; deletions and completions are sticky.
  - The password is kept in the OS keyring, or in a `0600` file when no keyring is available.
  - Warnings for `http://` URLs and for a clock that is more than a minute off from the server's.
- Database schema version 2: a `sync_docs` table for sync bookkeeping. It's added automatically,
  and existing data is unchanged.

### Changed

- **Events colors**: the Events section (its box, markers and countdowns more than a week away) is
  no longer pink. Everforest, Dracula, Gruvbox, Catppuccin, Nord and Solarized use a soft red that
  stays clearly apart from the red of overdue tasks and High priority; Tokyo Night keeps its
  lavender. The title banner now has its own end color, so it no longer follows the Events color.
- A task with a due time is no longer late when its time passes: it turns overdue at midnight, when
  its due day ends, like a task without a time. After the time it reads "today 2:00 PM" (narrow:
  "today") in the normal due-today color, and from the next day "! 1d overdue". This applies to the
  red title, the Overdue count, the on-time rate of new completions, and the overdue digest. The
  reminder at the due time is unchanged. Completions already logged keep the on-time or late flag
  they were given, so a timed task finished late on its due day by an older build still counts as
  late. No database or sync change.
- Completing a repeating task gives the next instance and the completion entry IDs derived from
  the completed task. Nothing visible changes; it lets two devices that complete the same task
  merge instead of duplicating it.
- An edit is always stamped later than the version it replaces, even if this device's clock is
  behind. With a correct clock this is just the current time.
- Completing a repeating task with a due time uses date *and* time: the next instance is the first
  occurrence after both the old due time and now (a daily 2 PM task done late in the morning is next due
  today at 2 PM). Tasks without a time work exactly as before.
- Sync: items with a due time or a weekday repeat are written as document version 2; older builds skip
  only those items. Everything else stays version 1.

### Fixed

HEAD
- Hints and messages in dialogs no longer get cut off on narrower terminals: the add/edit dialog's
  date and time hints, the help overlay, the icon check, the delete confirmation (long titles),
  and the sync settings now wrap their text and grow to fit.
- In the task list, a long timed due label ("today 12:00 PM · in 11h") no longer runs into the
  priority after it: the due column keeps one blank column after its widest label.
  
  feature/due-column-width

## [0.1.0] - 2026-10-08

### Added

- Tasks and countdown events in one terminal UI, with priorities, notes, and due dates.
- Repeats (daily, weekly, monthly, yearly, every N days/weeks/months); repeating tasks create
  their next instance when completed, and repeating events roll over by themselves.
- Statistics screen: totals, streaks, completions per week, on-time rate, priority breakdown.
- Seven themes with a live-preview picker; Nerd Font, Unicode, and ASCII icon modes.
- Narrow mode for small terminals, `--demo` sample data, and `--db <path>`.
