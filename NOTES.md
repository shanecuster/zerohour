# zerohour — Notes

A plain-language recap of each build phase: what exists, how to run it, and what's worth
remembering.

---

## Phase 1: Core + TUI, local only (done 2026-10-08)

### What was built

**`core/`: the logic, no terminal code** (so the Android app can reuse it later)

| File | What it does |
|---|---|
| `model.rs` | The data types: `Item` (a task or an event), `Priority`, `RepeatRule`, `Completion`. Every item has a UUID, `updated_at`, and a `deleted` flag, so it's ready for sync. |
| `repeat.rs` | Date math: next occurrence of a repeat, days-until countdowns, overdue checks, and parsing typed dates (`12/25`, `+3d`, `fri`...). Has the most tests. |
| `db.rs` | SQLite storage: schema, the migrations table, add/edit/soft-delete, and completing a task (marks it done, logs it, and creates the next instance in one transaction). |
| `stats.rs` | Totals, streaks, completions for each of the last 12 weeks, on-time rate, and the priority breakdown. All of it is computed from the completion log. |
| `theme.rs` | The 7 themes as named color slots. To add a theme, add one entry to `THEMES`. |
| `icons.rs` | Every icon in one table, with Nerd Font, Unicode, and ASCII versions. |

**`tui/`: the terminal app** (the `zerohour` command)

| File | What it does |
|---|---|
| `main.rs` | Reads the flags, opens the database, sets up the terminal, runs the main loop, and always restores the terminal, even after a crash (panic hook). |
| `app.rs` | All app state, plus what each key does. Database errors show as a red status line and never crash the app. |
| `input.rs` | Key bindings for the main and stats screens. |
| `demo.rs` | The sample data for `--demo`. |
| `ui/list.rs` | The main screen, with Tasks on top and Events below. Has a narrow mode for widths under 60 columns. |
| `ui/add.rs` | The add/edit dialog: first Task or Event, then the fields. |
| `ui/stats.rs` | Summary cards, a weekly bar chart, and the priority breakdown. |
| `ui/settings.rs` | The theme picker (live preview), the first-run icon check, and the delete confirmation. |
| `ui/help.rs` | The `?` overlay. |

**Tests:** 59 in total (`cargo test`). They cover leap years, Feb 29, month-end anchoring, year
rollover, every-N intervals, overdue items, events today and yesterday, streaks, time zones,
the database round trips, and a non-database file.

### Commands

```sh
cargo run -p zerohour -- --demo          # try it with sample data (nothing saved)
cargo run -p zerohour                    # real use; data in ~/.local/share/zerohour/zerohour.db
cargo run -p zerohour -- --db test.db    # use a specific database file
cargo test                               # run all tests
cargo clippy --all-targets               # lint
cargo fmt --all                          # format
cargo install --path tui                 # install `zerohour` into ~/.cargo/bin
```

### Keys

`a` add · `space`/`x` complete · `e`/`Enter` edit · `d` delete · `j`/`k` or arrows to move ·
`g`/`G` jump to top/bottom · `p` show passed events · `Tab`/`s` stats · `t` theme · `i` icon mode ·
`?` help · `q` quit

### Decisions made while building (worth knowing)

- **Next date for a repeating task**: the first occurrence that falls after BOTH the old due
  date and today. So a daily task you finish today is next due tomorrow, and a weekly task
  done 3 days early skips to the following week instead of landing on the same date again.
  The plan said `next_occurrence(rule, old_date, today)`, which would return the same date
  for early completions, so this is a small, deliberate refinement. It's in `next_task_date`.
- **"Keep after pass"** only applies to one-off events. Repeating events always roll forward,
  so the form hides that checkbox for them. One-off events that have passed are hidden unless
  the box was ticked; press `p` to see them anyway (so you can still edit or delete them).
- **Streaks** count back from today, or from yesterday if you haven't finished anything today
  yet. The day isn't over, so the streak isn't broken.
- **Stats use your local time zone** to decide which day a completion belongs to. Timestamps are
  stored in UTC.
- **Esc doesn't quit** the app. It closes popups; `q` quits.
- **The Nord theme** uses frost cyan for tasks. A test caught that its blue and purple were too
  similar to tell apart.
- **crossterm** comes in through `ratatui::crossterm` rather than as its own dependency. That
  guarantees it's the same version ratatui uses.

### Save to your personal notes

- **Title:** Run zerohour with sample data
  **Command:** `cargo run -p zerohour -- --demo`
  **When:** to check the layout or themes without touching your real data.

- **Title:** Install zerohour as a command
  **Command:** `cargo install --path tui` (from the project folder)
  **When:** after changes, to update the `zerohour` command in `~/.cargo/bin`.

- **Title:** Where zerohour keeps its data
  **Command:** `~/.local/share/zerohour/zerohour.db` (open with `sqlite3` to inspect)
  **When:** for backups, debugging, or starting fresh (delete the file).

- **Title:** Terminal stuck in a weird mode after a crash
  **Command:** `reset`
  **When:** if any TUI app ever leaves your shell not echoing keys. zerohour has a panic hook to
  prevent this, but it's a good general fix.

- **Title:** Nerd Font icons show as boxes
  **Command:** install a Nerd Font "Mono" variant (e.g. `JetBrainsMono Nerd Font Mono`) and set
  it as the terminal font; or press `i` in zerohour to switch to Unicode/ASCII icons.
  **When:** icons look broken, or columns don't line up.

### Next: Phase 2 (optional CouchDB sync)

Settings screen fields, `core/sync.rs` behind a `sync` cargo feature, `_changes` pull,
`_bulk_docs` push, and newest-`updated_at`-wins conflict handling. The data model is already
ready for it (UUIDs, `updated_at`, soft deletes).
