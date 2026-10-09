# zerohour

A terminal to-do list with built-in countdowns, written in Rust with [Ratatui](https://ratatui.rs).

Track **tasks** (things to do) and **events** (dates to count down to) in one clean, themeable TUI.
Add Christmas once and it counts down every year: after the date passes, it rolls over to the next one automatically.

> **Status:** early development. Screenshots coming soon.

## Features

- **Tasks and events in one view.** Tasks on top, events underneath, each with its own color and icon.
- **Countdowns that repeat.** Yearly, monthly, weekly, daily, or every N days/weeks/months.
- **Repeating tasks.** Completing one creates the next instance with the next due date.
- **Priorities and notes** on every item.
- **Statistics.** Completions per week, streaks, overdue count, and more, with a bar chart.
- **Themes.** Everforest, Dracula, Gruvbox, Catppuccin, Nord, Solarized, Tokyo Night, with a live-preview picker.
- **Nerd Font icons** with Unicode and ASCII fallbacks.
- **Narrow mode** for small terminal windows.
- **Local-first.** Your data lives in a single SQLite file on your machine.
- **Optional sync** between devices through your own CouchDB server. Off by default, and left out of the build unless you ask for it.

## Install

Requires a recent [Rust toolchain](https://rustup.rs).

```sh
git clone https://github.com/shanecuster/zerohour.git
cd zerohour
cargo install --path tui
```

Then run:

```sh
zerohour
```

To include the optional [sync](#sync-optional) feature, install with:

```sh
cargo install --path tui --features sync
```

Without `--features sync`, the app contains no network code at all.

Try it with sample data (nothing is saved):

```sh
zerohour --demo
```

For the best look, set your terminal to a **Nerd Font Mono** font (for example, "JetBrainsMono Nerd Font Mono").
Without one, press `i` in the app to switch to Unicode or ASCII icons.

## Usage

Press `?` inside the app for the full list of keybindings. The basics:

| Key | Action |
|---|---|
| `a` | Add a task or event |
| `Space` / `x` | Complete a task |
| `e` / `Enter` | Edit the selected item |
| `d` | Delete the selected item |
| `j` / `k` or arrows | Move up and down |
| `g` / `G` | Jump to top / bottom |
| `p` | Show or hide passed events |
| `Tab` / `s` | Switch to the stats screen |
| `t` | Pick a theme |
| `i` | Cycle icon mode (Nerd Font / Unicode / ASCII) |
| `?` | Help |
| `q` | Quit |
| `r` | Sync now (sync builds only) |
| `c` | Sync settings (sync builds only) |

Options:

- `--db <path>`: use a different database file
- `--demo`: load sample data without saving anything

Your data is stored in `~/.local/share/zerohour/zerohour.db`.

## Sync (optional)

zerohour can keep several devices in sync through a [CouchDB](https://couchdb.apache.org) server that you run yourself.
Sync is **off by default**. Until you switch it on, the app never touches the network.

### Before you start

- **Your data is stored unencrypted on the server.** Each task, event, and completion is a plain JSON document.
  Anyone with admin access to the server, or to its disk or backups, can read them.
  Use a server you control, reach it over `https://` or a private network such as Tailscale, and use a separate database and user just for zerohour.
- **Start every additional device with an empty database.** Sync merges everything, and every item has a unique ID.
  If two devices both already contain "Christmas", you will end up with two of them.
  Set up sync on a new device *before* adding anything there, and it simply downloads everything.
- **Keep device clocks correct** (enable NTP / automatic time). When the same item is changed on two devices,
  the newest change wins, so a clock that runs fast wins conflicts it shouldn't. zerohour warns you if your clock
  is more than a minute off from the server's.

### Server setup (once)

zerohour never creates or deletes databases. Create one database and one user for it, for example with `curl`
(`-u admin` asks for the admin password, so it stays out of your shell history; Fauxton, CouchDB's web UI, works too):

```sh
# The database
curl -u admin -X PUT https://couchdb.example.ts.net/zerohour

# A user just for zerohour (pick your own password; this one is a placeholder)
curl -u admin -X PUT https://couchdb.example.ts.net/_users/org.couchdb.user:zerohour-sync \
     -H 'Content-Type: application/json' \
     -d '{"name": "zerohour-sync", "password": "CHANGE-ME", "roles": [], "type": "user"}'

# Only that user may read and write the database
curl -u admin -X PUT https://couchdb.example.ts.net/zerohour/_security \
     -H 'Content-Type: application/json' \
     -d '{"admins": {"names": [], "roles": []}, "members": {"names": ["zerohour-sync"], "roles": []}}'
```

### Turning it on

1. Press `c` to open the sync settings.
2. Fill in the server URL (e.g. `https://couchdb.example.ts.net`), database name, username, and password.
3. Choose **Test connection**, then switch **Sync** on and choose **Save**. The first sync starts right away.

After that, zerohour syncs in the background when it starts, and whenever you press `r`.
The header shows the status: *never synced*, *syncing…*, *synced 14:03*, *offline*, or *sync error*.
A failed or offline sync never blocks the app; your data stays on your device and syncs next time.

`http://` URLs work, but zerohour warns you: the password and your data travel unencrypted unless the network
itself is encrypted (Tailscale, WireGuard, a VPN).

### How conflicts are resolved

- The newest change to an item wins.
- A deletion always sticks, even if another device edited the item later.
- A completed task stays completed, even if another device edited it later.
- Completing the same repeating task on two offline devices creates one next instance, not two.

### Where the password is kept

Never in the database file. zerohour uses your OS keyring (Secret Service / GNOME Keyring / KWallet on Linux,
Keychain on macOS, Credential Manager on Windows). If no keyring is available, for example on a headless machine,
it uses `~/.config/zerohour/sync-password`, a plain-text file readable only by you (`0600`).
The settings screen shows which one is in use.

### Starting over

**Reset sync state** in the settings forgets which versions the server has. Your data isn't touched.
The next sync compares everything again, without creating duplicates. Changing the server URL or database name does this automatically.

## Project layout

- `core/`: all the logic (data model, repeat and countdown math, storage, stats, themes, icons, and the optional sync in `core/src/sync/`). It has no terminal code, so it can be reused by other front ends.
- `tui/`: the Ratatui terminal interface.

## Roadmap

- [x] Core logic and terminal UI
- [x] Optional sync between devices (self-hosted CouchDB)
- [ ] Native Android app sharing the same Rust core
- [ ] Notifications and reminders

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# The same with sync compiled in
cargo test --features sync
cargo clippy --all-targets --features sync -- -D warnings
```

The sync tests use an in-memory fake CouchDB, so no server is needed. See [CHANGELOG.md](CHANGELOG.md) for what changed.

## License

MIT. See [LICENSE](LICENSE).
