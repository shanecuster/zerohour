# Changelog

All notable changes to zerohour. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **Better `--demo` data** for screenshots, all fictional and relative to today: 8 tasks (one due
  later today, one overdue, a Mon–Thu 2:00 PM "Submit weekly report", one with notes), 5 upcoming
  events (Christmas, a product launch, a monthly book club, a concert, a trip), and 12 weeks of
  completion history with a 9-day streak, a few late completions, and a mixed priority breakdown.
  Still in-memory unless combined with `--db`, and never synced.

- **Title banner**: a big "zerohour" at the top of the main screen, in a gradient from the theme's
  accent color to its event color, with a tagline counting down to the next event ("7 days to
  Your Birthday", "Launch day is today!", or the date when there's none). Half-block letters, or
  plain ASCII in ASCII icon mode. Shows on terminals at least 30 rows tall, not in narrow mode;
  `b` switches it off and on (saved as `banner`, on by default).

- **Due times** on tasks (optional, local wall-clock time): "today 2:00 PM · in 3h", "tomorrow 9:30 AM",
  "! overdue 2h". Overdue the minute the time passes. 12-hour by default; `T` switches to 24-hour.
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

## [0.1.0] - 2026-10-08

### Added

- Tasks and countdown events in one terminal UI, with priorities, notes, and due dates.
- Repeats (daily, weekly, monthly, yearly, every N days/weeks/months); repeating tasks create
  their next instance when completed, and repeating events roll over by themselves.
- Statistics screen: totals, streaks, completions per week, on-time rate, priority breakdown.
- Seven themes with a live-preview picker; Nerd Font, Unicode, and ASCII icon modes.
- Narrow mode for small terminals, `--demo` sample data, and `--db <path>`.
