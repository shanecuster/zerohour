# CLAUDE.md — zerohour

Rust + ratatui terminal to-do list with countdowns, optional CouchDB sync, and (later) an
Android app sharing the same core. **Read `docs/BUILD_PLAN.md` and `NOTES.md` first**: the
build plan is the source of truth for structure and decisions, and NOTES.md records what each
phase built and the decisions made along the way.

## Branch rules

- Feature branches off master (or off the branch the person names). Before making any
  change, run `git branch --show-current`; if it's `master`, stop and create (or ask for) a
  feature branch first.
- **Never merge, push, force-push, or delete branches or tags.** Merging is done by the
  person after testing.
- Commit in small, single-purpose steps with clear messages, so any one step is easy to test
  or revert.

## Hard constraints

- `core/` must never import `ratatui`, `crossterm`, or anything from `tui/`. It has to stay
  reusable by the Android app.
- UI code never hard-codes a color or a glyph: colors come from `Theme` slots
  (`core/src/theme.rs`), icons from the icon table (`core/src/icons.rs`).
- Sync stays optional: everything network-related lives behind the `sync` cargo feature
  (and the OS keyring behind `keyring`). A build without `--features sync` must contain no
  network code, and local-only use must behave exactly as before.
- **No hostnames, URLs of real servers, or credentials anywhere in the repo**: not in code,
  tests, docs, NOTES.md, or commit messages. Use placeholders such as
  `https://couchdb.example.ts.net`.
- Schema changes are new, additive entries in `MIGRATIONS` (`core/src/db.rs`); never edit a
  released migration. Sync document format changes bump the document `v` so older builds skip
  what they can't read.
- No new dependencies and no changes to dependency features without asking first.

## How to work

- For any non-trivial feature, inspect and report first, then stop and wait for answers
  before building.
- Ask before: adding a dependency, deviating from the build plan, or anything that could
  touch the person's real database, config, or keyring.
- If the plan and the code disagree, say so and ask; don't silently pick one.
- When something fails, find and explain the root cause rather than patching around it.
- At the end of a piece of work, give a short report: what changed, how to test it, anything
  surprising. After each phase, update `NOTES.md` (plain-language recap, new commands, and
  ready-to-paste notes: title, command, when to use it).

## Documentation standard (hard requirement)

The person building this is learning Rust *through* this codebase, so every section of code
is documented:

- Every file starts with a `//!` module comment: what it's for and how it fits the app.
- Every `struct`, `enum`, `trait`, field, and `fn` gets a `///` doc comment saying what it
  does and why it exists.
- Every non-obvious block inside a function gets a `//` comment explaining the next few
  lines. Explain Rust concepts the first time they appear in a file (ownership and borrowing,
  `Result`/`?`, `Option`, traits, lifetimes, iterator chains, pattern matching).
- Comment the *why*, not just the *what*. Comments must stay accurate: when code changes,
  update its comments in the same commit.
- Prefer clear, boring code over clever code; if something clever is needed, explain it.
- No `unwrap()` in non-test code without a comment explaining why it can't fail.

## Building and testing

All three checks must pass **both without and with sync** after every commit:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check

cargo test --features sync
cargo clippy --all-targets --features sync -- -D warnings
```

Tests need no network and no real server: sync tests use the in-memory fake CouchDB in
`core/src/sync/fake.rs` (a refused connection to `127.0.0.1` is fine for offline handling).
Date logic takes `today`/`now` as parameters so tests never depend on the clock.

**Never develop against the real database, config, or keyring.**

- Run the app against a throwaway database: `./target/debug/zerohour --db /tmp/.../z.db`, or
  `--demo` for sample data.
- For sync, override `XDG_CONFIG_HOME` and point `DBUS_SESSION_BUS_ADDRESS` at a nonexistent
  socket, so the password file and keyring lookups stay away from the real ones.
- **Before testing any migration, back up the real database**
  (`~/.local/share/zerohour/zerohour.db` → `~/zerohour-backups/zerohour-<timestamp>.db`) and
  test only on copies. Never run a new build against the real file.

To drive the TUI from a script, use a **named** tmux session and kill only that session by
name (`tmux kill-session -t <name>`). Never kill sessions you didn't create, and never run
`tmux kill-server`.

## Environment notes

- Primary machine: Linux. Data lives in `~/.local/share/zerohour/zerohour.db`; the sync
  password in the OS keyring or `~/.config/zerohour/sync-password` (0600).
- Keep `CHANGELOG.md` in its existing style, and update `README.md` and `NOTES.md` when
  behavior, keys, or settings change.
