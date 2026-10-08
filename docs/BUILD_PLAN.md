# zerohour — Build Plan

> App name: **zerohour** (a countdown to zero hour, with a nod to the owner's cybersecurity background).
> The installed command is `zerohour`. Keep the name in ONE place — the workspace `Cargo.toml` and an
> `APP_NAME` const — so it can be changed later if needed. If `zerohour` is taken on crates.io, publish
> as `zerohour-tui` but keep the binary name `zerohour`.

This document is the build path for Claude Code. Read it fully before writing code.

---

## 0. NON-NEGOTIABLE RULE: HEAVY COMMENTING

The owner **learns by reading the code**. Every section of code must be documented.

- Every file starts with a `//!` module doc comment: what this file is for and how it fits the whole app.
- Every `struct`, `enum`, `trait`, and `fn` has a `///` doc comment explaining **what it does and why it exists**.
- Every non-obvious block inside a function gets an inline `//` comment explaining what the next few lines do.
- Explain Rust concepts the first time they appear (ownership/borrowing, `Result`/`?`, `Option`, traits, lifetimes, iterators, pattern matching). Short and plain-language, not textbook.
- Prefer clear, boring code over clever code. If something clever is needed, explain it.
- Comment the **why**, not just the what (e.g. "we store dates as UTC midnight so DST can't shift a countdown by a day").
- After each phase, update `NOTES.md` with a plain-language recap: what was built, new commands, and anything the owner should save to personal notes.

---

## 1. What we are building

A terminal app (Rust + Ratatui) that combines a **to-do list** and **countdowns to dates**:

- **Tasks**: things to do, with optional due date, priority, and notes.
- **Events**: dates to count down to ("12 days until Christmas"). Events may repeat; after the date passes, a repeating event automatically rolls over to the next occurrence (Christmas → "365 days until next Christmas").
- **Statistics**: completions, streaks, completions per week, overdue count, shown in a separate screen with charts.
- **Themes**: Everforest, Dracula, Gruvbox, Catppuccin, Nord, Solarized, Tokyo Night.
- **Optional sync** between devices via CouchDB (off by default).
- Later: a native Android app sharing the same Rust core.

Desktop TUI comes first. Android and sync come after the TUI works well.

---

## 2. Decisions already made (do not re-litigate)

| Topic | Decision |
|---|---|
| Language/UI | Rust, Ratatui + crossterm |
| Local storage | SQLite (via `rusqlite`) |
| Sync | Optional, off by default; CouchDB over HTTP (separate DB + user from any other CouchDB use). Phase 2. |
| Android | Native Kotlin + Jetpack Compose UI, calling the shared Rust core via UniFFI. Styled to look like a TUI (monospace Nerd Font, box-drawing borders, same themes). Phase 3. |
| Repeats | Yearly, monthly, weekly/daily, custom interval (every N days/weeks/months) |
| Repeating tasks | Completing one creates a **new instance** with the next due date; old instance stays as history |
| Repeating events | Auto-roll to next occurrence at display time; nothing needs to be stored or clicked |
| Add flow | The add dialog first asks **Task or Event** with a one-line explanation of each |
| Layout | Two sections: Tasks on top, Events underneath; each with its own color and marker |
| Tags | NOT in v1 (leave room for a future migration). Priorities and notes ARE in v1. |
| Look | Inspired by the owner's `tuxwire` app and `bat` output: clean lines, borders, tables |

---

## 3. Workspace layout

Cargo workspace so the Android app can reuse `core` later:

```
zerohour/
├── Cargo.toml              # workspace root
├── NOTES.md                # plain-language recap per phase (see rule in section 0)
├── core/                   # NO terminal code in here. Pure logic.
│   └── src/
│       ├── lib.rs
│       ├── model.rs        # Item, Kind, Priority, RepeatRule, Completion
│       ├── repeat.rs       # next-occurrence + countdown math (heavily tested)
│       ├── db.rs           # SQLite schema, migrations, CRUD
│       ├── stats.rs        # streaks, per-week counts, overdue count
│       ├── theme.rs        # named color slots + built-in palettes
│       ├── icons.rs        # icon table: nerd / unicode / ascii fallbacks (section 8b)
│       └── sync.rs         # (phase 2) CouchDB sync, behind a feature flag
└── tui/
    └── src/
        ├── main.rs         # terminal setup/teardown, main loop
        ├── app.rs          # app state + key handling
        ├── ui/
        │   ├── mod.rs
        │   ├── list.rs     # Tasks + Events sections
        │   ├── add.rs      # Task/Event add dialog
        │   ├── stats.rs    # stats screen with charts
        │   └── settings.rs # theme picker, sync settings
        └── input.rs        # key bindings
```

**Rule:** `core` must never import ratatui or crossterm. This is what makes the Android app possible.

---

## 4. Data model (core/model.rs)

Every row is built for future sync from day one:

- `id`: UUID (string)
- `kind`: `Task` | `Event`
- `title`: string
- `notes`: optional string
- `priority`: `Low` | `Medium` | `High` (tasks; events may ignore it)
- `date`: optional date. Tasks: due date. Events: the target date (required for events).
- `repeat`: `RepeatRule` (see below)
- `keep_after_pass`: bool — for events: keep showing after the date passes (rolls forward if repeating)
- `completed_at`: optional timestamp (tasks)
- `created_at`, `updated_at`: timestamps (UTC). `updated_at` changes on EVERY edit.
- `deleted`: bool (soft delete, so deletions can sync later)

`RepeatRule`:

```rust
enum RepeatRule {
    None,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    EveryN { n: u32, unit: Unit },   // Unit = Days | Weeks | Months
}
```

`Completion` log (separate table, one row per completed task):
`id`, `item_id`, `completed_at`, `title_snapshot`, `priority_snapshot`, `was_overdue` (bool).
All statistics are computed from this log, so new stats never need a migration.

Dates are stored as plain calendar dates (no time zone drift). Timestamps are stored in UTC.

---

## 5. Repeat and countdown logic (core/repeat.rs)

This is the most error-prone part. Write it carefully and test it hard.

- `next_occurrence(rule, from_date, today) -> NaiveDate`: the first occurrence on or after `today`.
- `days_until(date, today) -> i64`: negative means overdue / passed.
- Events with a repeat rule: display the next occurrence on or after today. Christmas on Dec 26 shows 364 or 365 days until next Dec 25 (account for leap years).
- Month-end handling: a monthly repeat on Jan 31 lands on Feb 28/29, then Mar 31 (anchor to the ORIGINAL day-of-month, don't drift to 28 permanently).
- Feb 29 yearly: falls back to Feb 28 in non-leap years.
- Completing a repeating task: create a new task with `date = next_occurrence(rule, old_date, today)`.
- Make `today` a function parameter, never call "now" inside the logic, so tests are deterministic.

Required unit tests: leap years, Feb 29, month-end anchoring, year rollover, every-N intervals, overdue items, event exactly today (0 days), event yesterday.

---

## 6. Statistics (core/stats.rs)

All computed from the `completions` table:

- Total completed (all time, this week, this month)
- Completions per week for the last 12 weeks (bar chart data)
- Current streak and longest streak (consecutive days with at least one completion)
- Overdue count (open tasks with a past due date)
- Completion rate (completed on time vs overdue)
- Breakdown by priority

Stats functions take the data and `today` as inputs and return plain structs, so they are testable and reusable by Android.

---

## 7. Themes (core/theme.rs)

Code NEVER uses raw colors. It uses named slots:

`bg`, `fg`, `muted`, `border`, `accent`, `selection`, `task`, `event`, `ok`, `warn`, `danger`

Each theme is a table filling those slots with RGB values. Built-in themes: **Everforest** (default, owner's favorite), Dracula, Gruvbox, Catppuccin (Mocha), Nord, Solarized (dark), Tokyo Night.

- Theme choice is saved in the settings table and applied instantly when changed.
- Keep palettes in one data structure in `core` so the Android app can reuse them.
- Adding a theme later must mean adding ONE entry, nothing else.
- Task section color and Event section color must be clearly different in every theme. Also use markers (`☐` for tasks, `◷` for events) so color is never the only distinction.

---

## 8. TUI layout (tui/)

Style: like `bat` output and the owner's tuxwire — box-drawing borders, header with the app name, clean tables.

Main screen:

```
┌─ zerohour ───────────────────────────────────── Thu Oct 08 2026 ─┐
│ ┌─ Tasks ───────────────────────────────────────────────────────┐│
│ │ ☐ ▲ Renew car registration            due in 3d     High      ││
│ │ ☐ ● Order printer filament            due in 9d     Medium    ││
│ └───────────────────────────────────────────────────────────────┘│
│ ┌─ Events ──────────────────────────────────────────────────────┐│
│ │ ◷ Christmas                            78 days      ↻ yearly  ││
│ │ ◷ Trip to Colorado                     41 days                ││
│ └───────────────────────────────────────────────────────────────┘│
│ [a]dd  [space]done  [e]dit  [d]elete  [Tab]stats  [t]heme  [?]help│
└──────────────────────────────────────────────────────────────────┘
```

Behavior:
- `a` opens the add dialog. First step: choose **Task** or **Event** (`Tab` flips, default Task) with a one-line hint for each:
  - Task: "Something to do, optionally with a due date."
  - Event: "A date to count down to, like a birthday or holiday."
  Then the form shows the relevant fields (tasks: title, priority, due date, repeat, notes; events: title, date, repeat, keep-after-pass, notes).
- Navigation: `j`/`k` or arrows; `Tab` or `s` switches to the Stats screen; `t` opens the theme picker with live preview; `?` shows help.
- Overdue tasks are highlighted using the `danger` slot.
- Sort: tasks by priority then due date; events by days remaining.
- **Narrow mode:** below ~60 columns, drop secondary columns (priority text, repeat label) and truncate titles with `…`. This also acts as the design reference for the phone layout (~40–45 columns).
- Stats screen: summary numbers, bar chart of completions per week (Ratatui `BarChart`), streak display.
- Settings screen: theme, sync (phase 2).
- Make sure the terminal is always restored on exit AND on panic (install a panic hook).

---

## 8b. Icons (Nerd Font)

The owner likes Nerd Font icons. Handle them as a setting, never a hard requirement:

- **Icon mode setting** with three values: `nerd` (default for the owner), `unicode` (plain Unicode
  symbols such as `☐` and `◷`), and `ascii` (`[ ]` and `(o)`). Stored in the settings table.
- Define every icon ONCE in `core/src/icons.rs` as a table: icon name → (nerd glyph, unicode fallback,
  ascii fallback). UI code asks for an icon by name and never hard-codes a glyph.
- First-run hint: show a glyph and ask "Do you see an icon here?"; if not, switch to `unicode`.
- Recommend the **Nerd Font Mono** variants (e.g. "JetBrainsMono Nerd Font Mono"). Their icons are
  forced to single-cell width, so Ratatui borders and table columns stay aligned. Comment this in
  `icons.rs` so the owner understands why.
- A terminal app cannot bundle a font; the user sets it in their terminal. Say so in the help text.
- **Android:** bundle the Nerd Font in the APK so icons always render and look identical on every phone.
- Icons planned: task marker, event marker, repeat, priority levels, overdue, done, stats, settings.
  If categories or tags are added later, the same table (plus a user-editable mapping) supplies their icons.

---

## 9. Build phases

Work in order. Finish and test each phase before the next. After each phase: update `NOTES.md`.

### Phase 1 — Core + TUI, local only (build this first)
1. Create the workspace, `core` and `tui` crates.
2. `model.rs`, then `repeat.rs` WITH its tests.
3. `db.rs`: SQLite schema + migrations table (so future changes, like tags, are easy), CRUD, completion logging. Database lives in the platform data dir (use the `directories` crate); allow overriding via `--db <path>` for testing.
4. `theme.rs` with all palettes.
5. `stats.rs` with tests.
6. TUI: main screen, add dialog (Task/Event), complete/edit/delete, theme picker, stats screen, help overlay, narrow mode.
7. Seed a `--demo` flag that loads sample data so the layout can be judged immediately.

**Done when:** the owner can add tasks and events, complete a repeating task and see the new instance appear, watch an event roll over, switch all themes, view stats, and run `cargo test` green.

### Phase 2 — Optional CouchDB sync
- Off by default; settings screen has URL, database name, username, password.
- Separate database and user from any other CouchDB use (e.g. Obsidian sync).
- One CouchDB document per item, keyed by UUID; `completions` sync the same way.
- Pull with `_changes?since=<seq>`, push with `_bulk_docs`. Conflicts: newest `updated_at` wins. Soft-deleted items sync as deleted.
- Keep it in `core/sync.rs` behind a cargo feature (`sync`) so local-only users carry no network code.
- Store the password using the OS keyring if available, otherwise a file with `0600` permissions, and explain the tradeoff in comments.
- Handle offline gracefully: failed sync never blocks the app, just shows a status indicator.

### Phase 3 — Android
- Android Studio project (Kotlin + Jetpack Compose) in an `android/` folder.
- Rust `core` exposed via UniFFI; build with `cargo-ndk` (targets: `aarch64-linux-android`, `x86_64-linux-android`, `armv7-linux-androideabi`).
- Look like a TUI: bundled monospace Nerd Font (e.g. JetBrains Mono), box-drawing borders drawn on Canvas, the same theme palettes from `core`, block-character bar charts, `[ ]`/`[x]` checkboxes.
- Touch: tap to select, swipe to complete/delete, `[+]` button, bottom tab bar (Tasks / Events / Stats).
- Design for ~40–45 monospace columns in portrait with normal phone-readable font sizes; taller rows for fingers.
- Test on the Android Studio emulator and a real phone.

### Phase 4 — Polish
- Notifications/reminders, home-screen widget (Android), import/export (JSON), optional tags (new migration).

---

## 10. Suggested crates

`ratatui`, `crossterm`, `rusqlite` (with `bundled` feature), `chrono` (or `jiff`), `uuid`, `serde` + `serde_json`, `directories`, `clap` (CLI flags), `anyhow` / `thiserror` (errors), `reqwest` with `blocking` (phase 2 only, behind the `sync` feature).

Explain each dependency in a comment at its place in `Cargo.toml` (one line on why it's there).

---

## 11. Testing and quality

- `cargo test` covers `repeat.rs`, `stats.rs`, and `db.rs` (use an in-memory SQLite DB).
- `cargo clippy` and `cargo fmt` must pass.
- No `unwrap()` in non-test code without a comment explaining why it cannot fail; prefer `?` with clear error messages.
- A bad or missing DB file must produce a friendly message, never a panic.

---

## 12. Owner's reminders

- Flag anything worth saving to personal notes (install commands, fixes for hard problems) in ready-to-paste format: **title, command, when to use it**.
- Give a short recap at the end of each phase, because the owner can lose track of what a session covered.
- Ask before making big design changes that conflict with section 2.
