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

Options:

- `--db <path>`: use a different database file
- `--demo`: load sample data without saving anything

Your data is stored in `~/.local/share/zerohour/zerohour.db`.

## Project layout

- `core/`: all the logic (data model, repeat and countdown math, storage, stats, themes, icons). It has no terminal code, so it can be reused by other front ends.
- `tui/`: the Ratatui terminal interface.

## Roadmap

- [x] Core logic and terminal UI
- [ ] Optional sync between devices (self-hosted CouchDB)
- [ ] Native Android app sharing the same Rust core
- [ ] Notifications and reminders

## Development

```sh
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

## License

MIT. See [LICENSE](LICENSE).
