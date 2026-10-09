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

### Next: Phase 3 (Android)

The Android app can reuse `zerohour_core::sync` as-is (enable the `sync` feature, not
`keyring`), and provide its own `SecretStore` backed by the Android Keystore.
