//! # zerohour-core
//!
//! The shared "brain" of zerohour. Everything that is *logic* lives here; everything that is
//! *drawing on screen* lives in the `tui` crate (and later the Android app).
//!
//! Module map:
//! - [`model`]  — the data types: `Item` (a task or event), `Priority`, `RepeatRule`, `Completion`.
//! - [`repeat`] — date math: next occurrence of a repeat, days-until countdowns, parsing typed dates.
//! - [`db`]     — SQLite storage: schema, migrations, create/read/update/delete, completion log.
//! - [`stats`]  — statistics (streaks, per-week counts, overdue) computed from the completion log.
//! - [`theme`]  — named color slots and the built-in palettes (Everforest, Dracula, ...).
//! - [`icons`]  — every icon in one table with Nerd Font / Unicode / ASCII versions.
//!
//! A Rust "crate" is a package; a "module" is one file (or folder) inside it. `pub mod x;`
//! below tells the compiler "there is a file `x.rs`, and other crates may use what's public in it".

pub mod db;
pub mod icons;
pub mod model;
pub mod repeat;
pub mod stats;
pub mod theme;

/// The app's name, used for the data folder, window titles, and the header.
///
/// Keep the name in ONE place: here (plus the package names in the workspace `Cargo.toml`).
/// `&'static str` means "a borrowed string that lives for the whole program" — string literals
/// are baked into the binary, so they never go away.
pub const APP_NAME: &str = "zerohour";
