# Changelog

All notable changes to zerohour. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

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

## [0.1.0] - 2026-10-08

### Added

- Tasks and countdown events in one terminal UI, with priorities, notes, and due dates.
- Repeats (daily, weekly, monthly, yearly, every N days/weeks/months); repeating tasks create
  their next instance when completed, and repeating events roll over by themselves.
- Statistics screen: totals, streaks, completions per week, on-time rate, priority breakdown.
- Seven themes with a live-preview picker; Nerd Font, Unicode, and ASCII icon modes.
- Narrow mode for small terminals, `--demo` sample data, and `--db <path>`.
