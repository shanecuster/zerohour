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

---

## Phase 2: Optional CouchDB sync (done 2026-10-08)

Sync is **off by default** and only exists in builds made with `--features sync`. A normal
build (`cargo install --path tui`) contains no network code at all and behaves exactly as in
phase 1.

### What was built

**`core/src/sync/`: the sync logic** (no terminal code, so Android can reuse it)

| File | What it does |
|---|---|
| `mod.rs` | The settings (`SyncSettings`: URL, database, username, on/off) and their checks, the error messages (`SyncError`), and the entry points: `test_connection`, `sync_database`, `check_connection`. |
| `transport.rs` | The HTTP layer. A small `Transport` trait, plus the real version using `ureq`. Adds the login and sorts failures into offline, TLS problem, or other. |
| `couch.rs` | The only three CouchDB calls: `GET /{db}`, `_changes`, `_bulk_docs`. Turns 401/403/404 into clear messages. It never creates or deletes databases. |
| `doc.rs` | How an item or completion looks on the server: one JSON document each, `item:<uuid>` / `completion:<uuid>`, in the same formats as SQLite. |
| `merge.rs` | The conflict rules (see below). Pure functions, heavily tested. |
| `store.rs` | The bookkeeping in SQLite: each document's `_rev`, what still needs pushing, and the `_changes` checkpoint. |
| `engine.rs` | One full sync: check, pull, push, and pull+push again if another device wrote in between. |
| `secret.rs` | Where the password lives: OS keyring or a `0600` file, behind a `SecretStore` trait (Android will plug in its Keystore). |
| `fake.rs` | A fake CouchDB in memory, for the tests only. |

**`core/src/db.rs`:** migration 2 adds the `sync_docs` table. It's additive: existing data
is unchanged, and it's created in every build so one database file works in both kinds of
build.

**`tui/`:** `sync.rs` (the background worker and the header status), `ui/sync_settings.rs`
(the settings popup), and new keys in `input.rs` / `app.rs`.

**Tests:** 119 with sync compiled in (`cargo test --features sync`), 66 without. None needs a
real server: the sync tests run two simulated devices against the fake CouchDB, and recorded
response shapes (`core/src/sync/fixtures/`) check that real CouchDB answers parse.

### How a sync works (plain language)

1. **Check** that the database exists and the login works.
2. **Pull**: ask the server "what changed since checkpoint X?" (`_changes?since=X`), merge each
   document into SQLite, and save the new checkpoint in the *same* transaction, so a crash can
   never skip changes.
3. **Push**: send every item whose `updated_at` differs from the version last synced, plus
   every completion the server doesn't have (`_bulk_docs`). Each update carries the `_rev` we
   last saw. If someone else wrote first, CouchDB answers "conflict", and we pull and push again.

### Decisions made while building (worth knowing)

- **Conflict rules:** the newest `updated_at` wins the whole item. Deletes are sticky (an edit
  elsewhere can't bring a deleted item back). Completion is sticky (an edit elsewhere can't
  reopen a completed task). When the timestamps are equal, every device picks the same winner.
- **Clock skew protection:** an edit is always stamped at least 1 ms after the version it
  replaces (`model::next_updated_at`), so an edit you make after seeing a version always wins
  over it, even if your clock is behind. The status line and settings warn when your clock is
  more than a minute off from the server's.
- **Derived IDs:** completing a repeating task gives the next instance and the completion
  entry IDs computed from the old task's ID (UUID v5). If two offline devices complete the same
  task, they create the *same* rows, which merge into one.
- **Deleting a document in Fauxton** (CouchDB's web UI) soft-deletes the item on your devices,
  and the soft-deleted version goes back to the server. Completions are append-only, so a
  deleted completion is simply uploaded again.
- **Demo mode never syncs**, even with `--db`, so sample data can't reach your server.
- **ureq instead of reqwest:** a small blocking HTTP client with no hidden async runtime.
  TLS is rustls with Mozilla's root certificates, which works with Tailscale's Let's Encrypt
  certificates. A self-signed certificate would need the OS trust store (not added yet).
- **The `keyring` cargo feature** is separate from `sync`, so the Android app can use sync
  without it. The TUI's `sync` feature turns both on.
- **The documents on the server are not encrypted.** Use your own server, over https or a
  private network.
- **Second devices should start empty**: sync merges everything, so identical items typed on
  two devices before syncing will show up twice.

### Keys (sync builds)

`r` sync now · `c` sync settings (in the popup: `↑↓` move, `Space` on/off, `Enter` to choose
Save / Test connection / Reset sync state, `Esc` close)

### Commands

```sh
cargo install --path tui --features sync          # install zerohour WITH sync
cargo run -p zerohour --features sync             # run with sync
cargo test --features sync                        # all tests, including sync
cargo clippy --all-targets --features sync -- -D warnings
cargo tree -p zerohour -e normal | grep -E 'ureq|rustls|keyring'   # nothing = local-only build
```

### Save to your personal notes

- **Title:** Install zerohour with sync
  **Command:** `cargo install --path tui --features sync` (from the project folder)
  **When:** on every device that should sync. Without the flag you get the local-only app.

- **Title:** Create the zerohour CouchDB database and user
  **Command:** see "Server setup (once)" in README.md (three `curl` commands, or Fauxton)
  **When:** once, before switching sync on for the first time. zerohour never creates databases.

- **Title:** Adding a new device to sync
  **Command:** install with `--features sync`, press `c`, fill in the settings, **Test
  connection**, switch on, **Save**. Do this *before* adding any tasks on the new device.
  **When:** setting up a laptop, desktop, or (later) the phone.

- **Title:** Where the sync password is stored
  **Command:** OS keyring (service "zerohour"), or `~/.config/zerohour/sync-password` if there
  is no keyring. The settings screen (`c`) shows which.
  **When:** changing the password, or cleaning up a machine.

- **Title:** Sync looks stuck or confused
  **Command:** `c` → **Reset sync state** → `y`, then `r`
  **When:** after restoring a backup, or if the server's database was recreated. Your data isn't
  touched; the next sync compares everything again.

---

## Scheduling: layout, calendar, due times, weekday repeats (done 2026-10-09)

Branch `feature/schedule`, built on top of `feature/sync`.

### What was built

| Where | What it does |
|---|---|
| `CLAUDE.md` | The working rules for this project (branch rules, checks with and without sync, testing only on copies of the database...). |
| `tui/src/ui/list.rs` | Tasks and Events split the height 50/50 (each box gets at least 5 lines when there's room). `v` switches to "fit", the old layout. Saved as `layout`. |
| `core/src/calendar.rs` | Month-grid math: which date goes in which of the 6×7 cells, Sunday- or Monday-first, moving by days and months. Shared with Android later. |
| `tui/src/ui/calendar.rs` | The date picker popup (Ctrl+D / F2 on the date field), plus mouse clicks and the scroll wheel while it's open. |
| `core/src/clock.rs` | Times of day: parsing "2pm"/"14:30", 12/24-hour display (`time_format`), "in 3h" texts. |
| `core/src/db.rs` | Migration 3: `items.due_time` (local "HH:MM", NULL = no time). |
| `core/src/repeat.rs` | Time-aware overdue (`is_overdue` now takes the local date-time), `next_instance_date` (the date+time rule), weekday-set occurrences, `scheduled_on_or_after` (moving a start date onto a repeat day). |
| `core/src/model.rs` | `DaySet` (7 bits, Monday = bit 0) and `RepeatRule::Weekdays`, stored as `days:mon,tue,wed,thu`. |
| `core/src/sync/doc.rs` | Document `v: 2` for items with a due time or weekday repeat; everything else stays `v: 1`. |

**Tests:** 102 without sync, 159 with (`cargo test --features sync`).

### Decisions made while building (worth knowing)

- **Wall-clock times.** A due time is stored as plain local time ("14:00"), not UTC. On a day the
  clocks change, an "in 3h" can be off by an hour; nothing else is affected.
- **The date+time rule** for the next instance: the first occurrence strictly after both the old
  due date+time and now. Example: a daily 2 PM task due yesterday, done at 10 AM today, is next due
  today at 2 PM; done at 3 PM, tomorrow. Tasks without a time keep the old date-only rule exactly.
- **Weekday repeats never back-fill.** Missing Tuesday and Wednesday doesn't create two tasks; you
  get one overdue task, and completing it moves to the next scheduled day after now.
- **Completing early** (e.g. Sunday, for Monday's) counts as doing Monday's, so the next one is
  Tuesday.
- **All seven days** saves as plain "daily", which every build understands.
- **Older builds and weekday repeats.** An older zerohour can't read `days:...` from the database,
  so it refuses to open a file that contains one ("bad data"), rather than guessing. Over sync,
  older builds just skip those items (document `v: 2`).
- **Mouse capture** is only on while the calendar is open, so selecting text with the mouse works
  everywhere else in the app.
- **`complete_task` and `stats::compute`** now take the local date-time instead of just today's
  date, because overdue depends on the time.

### Keys

`v` layout (split / fit) · `T` 12/24-hour times · in the add dialog: `Ctrl+D` / `F2` calendar,
`Space` toggles a day · in the calendar: arrows/`hjkl`, `PgUp`/`PgDn`, `t` today, `w` week start,
`Enter` pick, `Esc` close, mouse click / scroll wheel

### Save to your personal notes

- **Title:** Back up the zerohour database before trying a new build
  **Command:** `mkdir -p ~/zerohour-backups && cp -p ~/.local/share/zerohour/zerohour.db ~/zerohour-backups/zerohour-$(date +%Y%m%d-%H%M%S).db`
  **When:** before running a version with a new database migration (like this one, schema 3).

- **Title:** Try a new build on a copy of your data
  **Command:** `cp ~/zerohour-backups/<backup>.db /tmp/z-test.db && ./target/debug/zerohour --db /tmp/z-test.db`
  **When:** to check a new version against your real items without touching the real file.

- **Title:** A task every Mon–Thu at 2 PM
  **Command:** `a` → Task → title → Due time `2pm` → Repeat `←→` to "Mon–Thu" → `Enter`
  **When:** recurring work-day tasks. Completing Thursday's makes Monday's.

### Planned later: the desktop `zerohour remind` command (Phase 4, not built)

The reminder *planner* and its settings are now built in core (see "Phase 3, step 11a" below).
What is still planned for the desktop:

- `zerohour remind`: a command that sends any due reminders and exits, run every minute by a
  systemd user timer (`Persistent=true`; `zerohour remind --print-units` prints the unit files).
- What it reminds about comes from core's planner (`reminders::plan`), the same as the phone.
- Desktop notifications with `notify-send`; optional ntfy (its own cargo feature, server and topic
  as settings, token in the `SecretStore`), enabled on one device only to avoid duplicates.
- De-duplication in a local, never-synced table `reminders_sent (item_id, kind, fire_at, sent_at)`;
  a row is written only after a successful send, reminders older than a grace window are skipped,
  and moving a task's time creates a new reminder.

---

## Title banner (done 2026-10-09)

Branch `feature/banner`. A big "zerohour" above the Tasks box, modeled on tuxwire's banner (art
hard-coded as text lines, no figlet library), plus a tagline.

### What was built

| Where | What it does |
|---|---|
| `core/src/banner.rs` | The letters, in two fonts: a 3-row half-block design (`▀ ▄ █`, 31 columns) for Nerd/Unicode icon modes, and figlet's `smslant` (4 rows, 37 columns, plain ASCII) for ASCII mode. |
| `core/src/theme.rs` | `Rgb::lerp`: mixes two theme colors, for gradients. |
| `tui/src/ui/banner.rs` | When to show it (`fits`), the accent→event gradient, and the tagline. |
| `tui/src/ui/list.rs` | Puts the banner above the Tasks box; the 50/50 split shares what's left. |

### Rules worth knowing

- **Shows only when** the terminal is at least 30 rows tall, not in narrow mode (60+ columns),
  and `b` hasn't switched it off. Then it costs 4 rows (5 in ASCII mode), tagline included.
- **Colors** are a left-to-right gradient from the theme's `accent` slot to its `event` slot,
  so every theme has its own, and nothing is hard-coded.
- **Tagline:** the soonest visible upcoming event: "X is today!", "1 day to X", "N days to X";
  today's date if there's none. Hidden events and passed ones (`p`) are skipped.
- The `zerohour` title in the top border stays.

### Keys

`b` banner on/off (saved as `banner` = on|off, default on)

### Save to your personal notes

- **Title:** Regenerate figlet text for a banner
  **Command:** `figlet -f smslant zerohour` (list fonts: `ls /usr/share/figlet/fonts`)
  **When:** making ASCII art for a project's title; paste the output into the source as a constant.

## Demo data for screenshots (done 2026-10-09)

`zerohour --demo` now shows a fuller, entirely fictional sample set (no real names, places,
employers or servers), built so the main screen and the stats screen both look populated.

### What was built

| File | What it does |
|---|---|
| `tui/src/demo.rs` | The sample data. `seed(db, now)` takes the local date and time as a parameter, so tests can seed the demo as of any moment. |
| `tui/src/main.rs` | Passes `local_now()` to `seed`; new tests check that `--demo` never returns a sync path. |

- **Tasks (8):** High, Medium and Low; one due later today at a set time ("in 2h"), one
  overdue (due yesterday), "Submit weekly report" repeating Mon–Thu at 2:00 PM, one with notes,
  one with no date.
- **Events (5):** Christmas (yearly), a product launch, a monthly book club, a concert and a
  trip. Every event is stored at its *next* occurrence, so none is ever in the past.
- **History:** completions from the Monday that starts the oldest bar on the stats chart up to
  yesterday (12 weeks), shaped by a per-week load × per-weekday shape (Sundays are off). Gives
  ~90 completions, a current streak of exactly 9 days (also the best), ~7% late, and a priority
  mix of about 40% Medium, 35% High, 25% Low.

### Rules worth knowing

- **Never in the past:** the due-today task falls back to 11:59 PM late in the evening; the
  weekly report moves to the next Mon–Thu once 2:00 PM has passed, so the overdue task stays
  the only one.
- **Nothing is completed "today"**, since a completion later than now would be in the future.
  The streak counts back from yesterday, which is how the stats screen treats an unfinished day.
- **Safety is unchanged:** `--demo` alone uses an in-memory database; with `--db` it seeds that
  file. Either way demo mode never syncs, and the keyring/password file are only looked at from
  the sync screen, which demo mode blocks.
- Tests seed at fixed moments (a Monday morning, a Thursday afternoon, a late Sunday, Dec 26,
  Jan 1) and check: no past events, exactly one overdue task, history spanning the 12 charted
  weeks, streak 9, and the priority mix. One more test builds the real `App` and checks the
  stats it would draw.

### Save to your personal notes

- **Title:** Take zerohour screenshots with sample data
  **Command:** `cargo run -p zerohour -- --demo` (then `Tab` for the stats screen)
  **When:** for README images or to check a theme; the data is fictional and always current.

---

## Phase 3, step 1: shared screen logic in core (done 2026-10-22)

Branch `feature/core-view`. Preparation for the Android app: everything that decides *what*
the screens show moved from `tui/` into `core/`, so the phone can't drift from the terminal.
Nothing changes on screen.

### What was built

| Where | What it does |
|---|---|
| `core/src/theme.rs` | `Slot`: the *name* of a theme color (`Danger`, `Muted`...), and `Theme::slot` to look it up. Shared code says "draw this in Danger"; each app picks the actual color. |
| `core/src/view.rs` | The main screen's content: which tasks and events are listed and in what order (`open_tasks`, `event_rows`), the "due in 3d" / "78 days" texts and their color slots (`due_label`, `countdown_label`), the banner tagline, and small texts (dates, repeat labels). |
| `core/src/form.rs` | The add/edit form: the values as typed (`FormInput`), which fields appear, the repeat choices with the weekday presets, validation (`validate` → `FormData`), and the date/time previews. |
| `core/src/calendar.rs` | `WeekStart::weekdays`: the day order for the day checkboxes. |
| `tui/` | Calls the above. The TUI's `Form` now holds a core `FormInput` plus what only the terminal needs (focus, cursor, error line, calendar popup). |

**Tests:** 150 without sync, 208 with. The column-alignment tests now check core's labels.

### How "identical output" was checked

Before the change, a temporary test (never committed) drew about 47,600 screens:
- **Screens:** main, stats and the add/edit form, with demo data plus extra edge cases.
- **Variations:** 3 dates, 3 icon modes, 12/24-hour times, 2 themes, 6 terminal sizes, with
  and without passed events, and different selected rows.
- **Recorded:** every cell's character, colors and bold/italic, plus each form's validation
  result.

After each commit the same screens were drawn again and compared: all identical, with and
without sync.

### Decisions for Phase 3 (Android)

- **Sync password on Android**: kept on the Kotlin side, encrypted with a key that lives in the
  Android Keystore, and passed into Rust for each sync (`sync_database(..., password, ...)`).
  The phone doesn't implement Rust's `SecretStore` trait. That trait (OS keyring or `0600`
  file) stays a desktop thing. Android builds core with `sync` but without `keyring`.
- **Phone sync is https only**: no plain `http://`, not even on a private network.
- **Demo data** will move into core and be used only by Android *debug* builds, in memory,
  never syncing.
- **Touch**: one tap opens edit; swipe right completes; swipe left asks before deleting; the
  checkbox only shows state.
- **Reminders**: the core reminder planner (migration 4) and Android alarms are the *last* step
  of Phase 3. The desktop `zerohour remind` command and ntfy stay in Phase 4.
- **No machine-specific paths** (JDK, SDK) in committed files: environment variables or a
  git-ignored file only. Signing keys and keystores stay out of git.

---

## Phase 3, step 1b: demo data in core (done 2026-10-22)

Branch `feature/core-demo`. The `--demo` sample data moved from `tui/src/demo.rs` to
`core/src/demo.rs`, so the Android debug builds can show exactly the same tasks, events and
stats history. Nothing changes on screen.

### What was built

| Where | What it does |
|---|---|
| `core/src/demo.rs` | The seeder, unchanged in content. `seed(db, now, tz)` now also takes the time zone (like `stats::compute`), so core never reads the machine's zone, and it returns core's `DbError` instead of `anyhow`. |
| `tui/src/main.rs` | Calls `zerohour_core::demo::seed(&db, local_now(), &Local)`. The test that needs the TUI's `App` (`app_shows_the_same_stats`) moved here. |
| `docs/BUILD_PLAN.md` | Phase 3 now says the Android app is its own repository, `zerohour-android`, pinning core by commit. |

**Tests:** 150 without sync, 208 with (the same totals: five demo tests moved from `tui` to
`core`). The core demo tests now run in UTC and a fixed UTC−6 zone instead of the machine's.

### How "same data, same screens" was checked

- A temporary test (never committed) seeded the old TUI seeder and the new core one side by side
  at 3,365 moments spread over two years, in three time zones (Denver, Berlin, Auckland, so
  daylight-saving changes are covered), and compared every item and completion field except the
  random IDs: all identical.
- The old (`f372000`) and new release binaries ran `--demo` in tmux at 120×45 and 50×40, and the
  main and stats screens were captured with colors: identical.

### Decisions made while building (worth knowing)

- **Time zone as a parameter.** The TUI passes `chrono::Local`, which is what it used before, so
  `--demo` behaves exactly as it did. Android will pass `Local` too.
- **The reminder planner (migration 4) is not part of this.** It comes with the reminders step
  at the end of Phase 3, with a backup of the real database first.

---

## Phase 3, step 11a: the reminder planner in core (done 2026-10-22)

Branch `feature/reminders`. Core now works out *which reminders are due when*; the Android app
turns that into alarms and notifications. Nothing changes in the terminal app yet (its
`zerohour remind` command is Phase 4).

### What was built

| File | What it does |
|---|---|
| `core/src/reminders.rs` | `ReminderSettings` (load and save, with defaults and checks) and the planner, `plan(items, now, until, settings, time_format)`, which returns the reminders after `now` and up to `until`, earliest first. |
| `core/src/db.rs` | Four new setting keys: `reminders.enabled`, `reminders.lead_minutes`, `reminders.day_time`, `reminders.overdue_digest`. |

### What gets a reminder

- **A task with a due time:** at that time ("Due 2:00 PM"), plus `lead_minutes` earlier if set
  ("Due in 15 min (2:00 PM)"). A lead time can cross midnight.
- **A repeating task with a due time:** also at each later scheduled time while it stays open. A
  missed Monday 2 PM "Fill Out Your Timesheet" (Mon–Thu) still reminds on Tuesday at 2 PM. Every
  reminder names the stored open instance, so completing it creates the next one as usual.
- **A task due on a day without a time:** once, at `day_time` (09:00) on that day ("Due today").
  Only its open instance; once the day is over it is just overdue.
- **The overdue digest** (off by default): once a day at `day_time`, the tasks overdue at that
  moment ("3 overdue tasks", up to five titles then "and N more"). Not about one task, so it has no
  "Done".
- **Never:** completed or deleted tasks, tasks without a date, and events (a "morning of" event
  notice could be a separate, off-by-default setting later).

### Rules worth knowing

- **Only the future.** Nothing at or before "now" is returned, so a device that was off when a
  reminder was due never gets it replayed.
- **Floating times.** Reminders are local wall-clock times with no time zone, like due times.
  The app turns them into real moments with the device's current zone. On Android that is
  `java.time`: a time inside the spring-forward gap fires just after it, a time in the fall-back
  overlap fires at the first of the two, and after a time-zone change "2 PM" is 2 PM in the new
  zone.
- **Settings are per device.** They live in the `settings` table, which is never synced.
  Reminders are off until switched on; lead time 0 (stored 0–1440, anything else counts as 0);
  day time 09:00; digest off. Values that aren't understood fall back to the defaults.
- **`Reminder::key()`** ("item|Kind|2026-10-26T14:00") names a reminder the same way every time
  it is planned, so an app can schedule it exactly once and cancel the ones no longer planned.
- **No migration 4 now.** The `reminders_sent` table was for de-duplicating a command that polls
  every minute; an app that schedules alarms doesn't need it. It stays with the Phase 4 desktop
  command. Checked: no build, branch or database has ever used number 4, and the real database is
  at schema 3.
- **No sync change.** Reminders are worked out from items; no new document fields, no `v` bump.

### Save to your personal notes

- **Title:** Run only the reminder tests
  **Command:** `cargo test -p zerohour-core reminders`
  **When:** after changing anything in `core/src/reminders.rs` or the repeat rules.

