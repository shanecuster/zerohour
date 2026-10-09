//! SQLite storage: the schema, migrations, and every read/write the app does.
//!
//! The rest of the app never writes SQL; it calls methods on [`Db`] like `insert_item` or
//! `complete_task`. Keeping all SQL in one file means a schema change only touches this file.
//!
//! **Migrations.** The database remembers which schema changes it has already applied (in the
//! `schema_migrations` table). On every start we apply any newer ones, in order. To change the
//! schema later (e.g. adding tags), append a new entry to [`MIGRATIONS`] — never edit an old
//! one, because existing databases have already run it.
//!
//! **Formats.** Calendar dates are stored as `YYYY-MM-DD` text. Timestamps are stored as UTC in
//! RFC 3339 text (`2026-10-08T14:03:00Z`). Text sorts correctly and is easy to read with the
//! `sqlite3` command-line tool when debugging.

use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::model::{Completion, Item, Kind, Priority, RepeatRule, new_id};
use crate::repeat::{is_overdue, next_task_date};

/// Everything that can go wrong in the storage layer.
///
/// `thiserror` writes the boring parts: `#[error("...")]` is the message shown to the user, and
/// `#[from]` lets the `?` operator turn a `rusqlite::Error` into a `DbError` automatically.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// An error from SQLite itself (file not a database, disk full, locked, ...).
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The data dir or the database file couldn't be created/opened.
    #[error("can't open the database at {path}: {reason}")]
    Open { path: PathBuf, reason: String },
    /// A row contained text we couldn't understand (e.g. an unknown priority).
    #[error("bad data in the database: {0}")]
    BadData(String),
    /// Tried to act on an item id that doesn't exist (or is deleted).
    #[error("item not found: {0}")]
    NotFound(String),
}

/// Short alias so function signatures read `Result<Item>` instead of `Result<Item, DbError>`.
pub type Result<T> = std::result::Result<T, DbError>;

/// Every schema change ever made, in order. Each entry is (version number, SQL to run).
///
/// Version 1 is the initial schema; version 2 adds the sync bookkeeping table.
/// NEVER edit an entry once released; add a new one instead.
const MIGRATIONS: &[(i64, &str)] = &[
    (
        1,
        "
    -- One row per task or event. Tasks and events share the table (see model::Item).
    CREATE TABLE items (
        id              TEXT PRIMARY KEY,      -- UUID, unique across devices
        kind            TEXT NOT NULL,         -- 'task' | 'event'
        title           TEXT NOT NULL,
        notes           TEXT,                  -- NULL = no notes
        priority        TEXT NOT NULL,         -- 'low' | 'medium' | 'high'
        date            TEXT,                  -- YYYY-MM-DD, NULL = no date
        repeat          TEXT NOT NULL,         -- see RepeatRule::to_db_string
        keep_after_pass INTEGER NOT NULL DEFAULT 0,
        completed_at    TEXT,                  -- RFC 3339 UTC, NULL = open
        created_at      TEXT NOT NULL,
        updated_at      TEXT NOT NULL,
        deleted         INTEGER NOT NULL DEFAULT 0
    );

    -- One row per completed task. All statistics are computed from this log.
    CREATE TABLE completions (
        id                TEXT PRIMARY KEY,
        item_id           TEXT NOT NULL,
        completed_at      TEXT NOT NULL,
        title_snapshot    TEXT NOT NULL,
        priority_snapshot TEXT NOT NULL,
        was_overdue       INTEGER NOT NULL
    );
    CREATE INDEX completions_completed_at ON completions (completed_at);

    -- Simple key/value settings (theme, icon mode, ...).
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    ",
    ),
    (
        2,
        "
    -- Sync bookkeeping (phase 2). One row per document this device has seen on, or pushed
    -- to, the CouchDB server. Purely additive: no existing table or column changes, and a
    -- database that never syncs simply leaves this table empty. Created in every build (with
    -- or without the `sync` feature) so one database file works with both kinds of build.
    CREATE TABLE sync_docs (
        doc_id            TEXT PRIMARY KEY,  -- 'item:<uuid>' or 'completion:<uuid>'
        rev               TEXT NOT NULL,     -- CouchDB _rev we last saw or wrote
        synced_updated_at TEXT               -- updated_at of the version that rev holds
    );
    ",
    ),
];

/// The column list for `items`, in the order [`item_from_row`] expects.
/// Keeping it in one constant means SELECTs can't get out of sync with the row reader.
const ITEM_COLUMNS: &str = "id, kind, title, notes, priority, date, repeat, keep_after_pass, \
                            completed_at, created_at, updated_at, deleted";

/// Well-known keys for the settings table, so typos become compile errors instead of bugs.
pub mod settings_keys {
    /// Name of the chosen theme, e.g. "Everforest".
    pub const THEME: &str = "theme";
    /// Icon mode: "nerd", "unicode" or "ascii". Missing = first run, ask the user.
    pub const ICON_MODE: &str = "icon_mode";
}

/// A handle to the open database. All storage goes through this.
///
/// It owns the SQLite `Connection`; when the `Db` is dropped (goes out of scope), Rust closes
/// the connection automatically. No manual cleanup needed — that's ownership at work.
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Opens (or creates) the database file at `path` and brings its schema up to date.
    ///
    /// Creates the parent folder if needed. Fails with a clear message if the file isn't a
    /// SQLite database or can't be written.
    pub fn open(path: &Path) -> Result<Db> {
        // `if let Some(x) = ...` runs the block only when the Option has a value.
        if let Some(parent) = path.parent() {
            // An empty parent means "current folder", which already exists.
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| DbError::Open {
                    path: path.to_path_buf(),
                    reason: format!("couldn't create folder {}: {e}", parent.display()),
                })?;
            }
        }
        let conn = Connection::open(path).map_err(|e| DbError::Open {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        // SQLite opens files lazily, so a non-database file is only detected on first use.
        // Running the migrations is that first use; we reword the error to name the file.
        Db::init(conn).map_err(|e| match e {
            DbError::Sqlite(inner) => DbError::Open {
                path: path.to_path_buf(),
                reason: inner.to_string(),
            },
            other => other,
        })
    }

    /// Opens a fresh database that lives only in memory. Used by tests and `--demo`.
    pub fn open_in_memory() -> Result<Db> {
        Db::init(Connection::open_in_memory()?)
    }

    /// Shared setup for both constructors: sane settings + migrations.
    fn init(conn: Connection) -> Result<Db> {
        // Wait up to 2s if another process (e.g. a second zerohour window) holds a lock,
        // instead of failing immediately with "database is locked".
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        let mut db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Applies any migrations this database hasn't seen yet, each in its own transaction.
    ///
    /// A transaction means "all or nothing": if a migration fails halfway, SQLite rolls back
    /// every statement in it, so the database is never left half-upgraded.
    fn migrate(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 version    INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL
             );",
        )?;
        // The highest version already applied (0 for a brand-new database).
        let current: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;

        // `.iter()` walks the list by reference; `.filter(...)` keeps only newer migrations.
        // `&&(v, _)` destructures the reference-to-a-tuple in the closure's argument.
        for (version, sql) in MIGRATIONS.iter().filter(|&&(v, _)| v > current) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![version, ts_to_db(Utc::now())],
            )?;
            tx.commit()?; // Without commit, dropping `tx` would roll everything back.
        }
        Ok(())
    }

    /// The highest schema version applied. Mostly useful for tests and debugging.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?)
    }

    // ---- items -------------------------------------------------------------------------

    /// Saves a new item.
    pub fn insert_item(&self, item: &Item) -> Result<()> {
        insert_item_on(&self.conn, item)
    }

    /// Saves changes to an existing item, stamping `updated_at = now`.
    ///
    /// `item: &mut Item` is a *mutable borrow*: we may change the caller's item (to set
    /// `updated_at`), but we don't take ownership of it.
    pub fn update_item(&self, item: &mut Item, now: DateTime<Utc>) -> Result<()> {
        item.updated_at = now;
        let changed = self.conn.execute(
            "UPDATE items SET kind = ?2, title = ?3, notes = ?4, priority = ?5, date = ?6,
                              repeat = ?7, keep_after_pass = ?8, completed_at = ?9,
                              created_at = ?10, updated_at = ?11, deleted = ?12
             WHERE id = ?1",
            params![
                item.id,
                item.kind.as_str(),
                item.title,
                item.notes,
                item.priority.as_str(),
                item.date.map(date_to_db),
                item.repeat.to_db_string(),
                item.keep_after_pass,
                item.completed_at.map(ts_to_db),
                ts_to_db(item.created_at),
                ts_to_db(item.updated_at),
                item.deleted,
            ],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound(item.id.clone()));
        }
        Ok(())
    }

    /// Soft-deletes an item: marks it deleted (and bumps `updated_at`) but keeps the row,
    /// so the deletion can be synced to other devices later.
    pub fn delete_item(&self, id: &str, now: DateTime<Utc>) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE items SET deleted = 1, updated_at = ?2 WHERE id = ?1 AND deleted = 0",
            params![id, ts_to_db(now)],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Loads one item by id (including deleted ones), or `None` if there's no such id.
    pub fn get_item(&self, id: &str) -> Result<Option<Item>> {
        let sql = format!("SELECT {ITEM_COLUMNS} FROM items WHERE id = ?1");
        // `.optional()` turns "no rows" into `Ok(None)` instead of an error.
        let raw = self
            .conn
            .query_row(&sql, [id], RawItem::from_row)
            .optional()?;
        // `.map(...)` converts the inner RawItem if present; `.transpose()` flips
        // `Option<Result<Item>>` into `Result<Option<Item>>` so `?` can be used on it.
        raw.map(RawItem::into_item).transpose()
    }

    /// Every item that isn't deleted, open and completed, tasks and events.
    pub fn list_items(&self) -> Result<Vec<Item>> {
        let sql = format!("SELECT {ITEM_COLUMNS} FROM items WHERE deleted = 0 ORDER BY created_at");
        let mut stmt = self.conn.prepare(&sql)?;
        // `query_map` runs the query and calls `RawItem::from_row` for each row, lazily.
        let rows = stmt.query_map([], RawItem::from_row)?;
        // Each row is a Result; collect them, stopping at the first error, then convert.
        let mut items = Vec::new();
        for raw in rows {
            items.push(raw?.into_item()?);
        }
        Ok(items)
    }

    // ---- completions -------------------------------------------------------------------

    /// Marks a task done, logs the completion, and — if it repeats — creates the next instance.
    ///
    /// All three writes happen in one transaction, so a crash can't leave a completed task
    /// without its log entry or its next instance. Returns the new instance, if one was made.
    ///
    /// `today` is the user's local date (for overdue/next-date math); `now` is the exact moment.
    pub fn complete_task(
        &mut self,
        id: &str,
        today: NaiveDate,
        now: DateTime<Utc>,
    ) -> Result<Option<Item>> {
        let mut task = self
            .get_item(id)?
            .filter(|item| !item.deleted && item.kind == Kind::Task)
            // `ok_or_else` turns None into an error (built lazily by the closure).
            .ok_or_else(|| DbError::NotFound(id.to_string()))?;
        if task.is_completed() {
            return Ok(None); // Already done; completing twice would double-count stats.
        }

        let completion = Completion {
            id: new_id(),
            item_id: task.id.clone(),
            completed_at: now,
            title_snapshot: task.title.clone(),
            priority_snapshot: task.priority,
            was_overdue: is_overdue(&task, today),
        };

        // Build the next instance (if repeating) before we start writing.
        let next = next_task_date(task.repeat, task.date, today).map(|date| {
            // `..task.clone()` = "every other field copied from the old task".
            Item {
                id: new_id(),
                date: Some(date),
                completed_at: None,
                created_at: now,
                updated_at: now,
                ..task.clone()
            }
        });

        task.completed_at = Some(now);
        task.updated_at = now;

        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE items SET completed_at = ?2, updated_at = ?3 WHERE id = ?1",
            params![task.id, ts_to_db(now), ts_to_db(now)],
        )?;
        insert_completion_on(&tx, &completion)?;
        if let Some(next) = &next {
            insert_item_on(&tx, next)?;
        }
        tx.commit()?;
        Ok(next)
    }

    /// Adds a completion log entry directly. Used by the demo data seeder and tests;
    /// normal completions go through [`Db::complete_task`].
    pub fn insert_completion(&self, completion: &Completion) -> Result<()> {
        insert_completion_on(&self.conn, completion)
    }

    /// The whole completion log, oldest first. This is the input for `stats::compute`.
    pub fn list_completions(&self) -> Result<Vec<Completion>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, item_id, completed_at, title_snapshot, priority_snapshot, was_overdue
             FROM completions ORDER BY completed_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, bool>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, item_id, completed_at, title, priority, was_overdue) = row?;
            out.push(Completion {
                id,
                item_id,
                completed_at: ts_from_db(&completed_at)?,
                title_snapshot: title,
                priority_snapshot: Priority::parse(&priority)
                    .ok_or_else(|| DbError::BadData(format!("unknown priority '{priority}'")))?,
                was_overdue,
            });
        }
        Ok(out)
    }

    // ---- settings ----------------------------------------------------------------------

    /// Reads a setting, or `None` if it was never set.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    /// Writes a setting, replacing any previous value ("upsert").
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

/// The default database location: the platform's data folder, e.g.
/// `~/.local/share/zerohour/zerohour.db` on Linux.
///
/// Returns `None` if the OS doesn't tell us a home folder (rare; then use `--db`).
pub fn default_db_path() -> Option<PathBuf> {
    // ProjectDirs::from(qualifier, organization, application). We only need the app name.
    directories::ProjectDirs::from("", "", crate::APP_NAME)
        .map(|dirs| dirs.data_dir().join(format!("{}.db", crate::APP_NAME)))
}

// ---- helpers ---------------------------------------------------------------------------
//
// These take `&Connection` so they work both on the plain connection and inside a
// transaction: a rusqlite `Transaction` "derefs" to a `Connection`, meaning `&tx` can be
// passed wherever `&Connection` is expected.

/// Inserts an item row using the given connection or transaction.
fn insert_item_on(conn: &Connection, item: &Item) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO items ({ITEM_COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
        ),
        params![
            item.id,
            item.kind.as_str(),
            item.title,
            item.notes,
            item.priority.as_str(),
            item.date.map(date_to_db),
            item.repeat.to_db_string(),
            item.keep_after_pass,
            item.completed_at.map(ts_to_db),
            ts_to_db(item.created_at),
            ts_to_db(item.updated_at),
            item.deleted,
        ],
    )?;
    Ok(())
}

/// Inserts a completion row using the given connection or transaction.
fn insert_completion_on(conn: &Connection, c: &Completion) -> Result<()> {
    conn.execute(
        "INSERT INTO completions
             (id, item_id, completed_at, title_snapshot, priority_snapshot, was_overdue)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            c.id,
            c.item_id,
            ts_to_db(c.completed_at),
            c.title_snapshot,
            c.priority_snapshot.as_str(),
            c.was_overdue,
        ],
    )?;
    Ok(())
}

/// An item row exactly as stored (all text), before we parse it into a real [`Item`].
///
/// Reading happens in two steps because rusqlite's row callback can only return rusqlite
/// errors; parsing our own formats (dates, enums) happens afterwards with our own errors.
struct RawItem {
    id: String,
    kind: String,
    title: String,
    notes: Option<String>,
    priority: String,
    date: Option<String>,
    repeat: String,
    keep_after_pass: bool,
    completed_at: Option<String>,
    created_at: String,
    updated_at: String,
    deleted: bool,
}

impl RawItem {
    /// Reads the columns listed in [`ITEM_COLUMNS`], in order.
    ///
    /// `Row<'_>` has a *lifetime* parameter: the row borrows from the running query and is only
    /// valid while it runs. `'_` tells Rust "work out that lifetime yourself".
    fn from_row(row: &Row<'_>) -> rusqlite::Result<RawItem> {
        Ok(RawItem {
            id: row.get(0)?,
            kind: row.get(1)?,
            title: row.get(2)?,
            notes: row.get(3)?,
            priority: row.get(4)?,
            date: row.get(5)?,
            repeat: row.get(6)?,
            keep_after_pass: row.get(7)?,
            completed_at: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
            deleted: row.get(11)?,
        })
    }

    /// Parses the text columns into proper types, failing with a clear message on bad data.
    fn into_item(self) -> Result<Item> {
        let bad = |what: &str, value: &str| DbError::BadData(format!("unknown {what} '{value}'"));
        Ok(Item {
            kind: Kind::parse(&self.kind).ok_or_else(|| bad("kind", &self.kind))?,
            priority: Priority::parse(&self.priority)
                .ok_or_else(|| bad("priority", &self.priority))?,
            repeat: RepeatRule::parse(&self.repeat).ok_or_else(|| bad("repeat", &self.repeat))?,
            date: self.date.as_deref().map(date_from_db).transpose()?,
            completed_at: self.completed_at.as_deref().map(ts_from_db).transpose()?,
            created_at: ts_from_db(&self.created_at)?,
            updated_at: ts_from_db(&self.updated_at)?,
            id: self.id,
            title: self.title,
            notes: self.notes,
            keep_after_pass: self.keep_after_pass,
            deleted: self.deleted,
        })
    }
}

/// Date -> `"2026-12-25"`.
fn date_to_db(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// `"2026-12-25"` -> Date.
fn date_from_db(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| DbError::BadData(format!("bad date '{s}'")))
}

/// Timestamp -> `"2026-10-08T14:03:00.123Z"` (RFC 3339, always UTC, always the same width
/// so that text sorting equals time sorting).
fn ts_to_db(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// RFC 3339 text -> UTC timestamp.
fn ts_from_db(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|ts| ts.with_timezone(&Utc))
        .map_err(|_| DbError::BadData(format!("bad timestamp '{s}'")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Unit;
    use chrono::TimeZone;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 8, 12, 0, 0).unwrap()
    }

    #[test]
    fn migrations_run_once_and_set_version() {
        let mut db = Db::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), 2);
        // Running again must be a no-op (no "table already exists" error).
        db.migrate().unwrap();
        assert_eq!(db.schema_version().unwrap(), 2);
    }

    #[test]
    fn version_1_database_upgrades_without_touching_existing_rows() {
        // Build a database exactly as a phase 1 build left it: only migration 1 applied.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute_batch(MIGRATIONS[0].1).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (1, ?1)",
            [ts_to_db(now())],
        )
        .unwrap();
        let old = Db { conn };
        let mut task = Item::new(Kind::Task, "old task", now());
        task.date = Some(d(2026, 10, 1));
        old.insert_item(&task).unwrap();
        let completion = Completion {
            id: new_id(),
            item_id: task.id.clone(),
            completed_at: now(),
            title_snapshot: "old task".into(),
            priority_snapshot: Priority::Medium,
            was_overdue: true,
        };
        old.insert_completion(&completion).unwrap();
        old.set_setting(settings_keys::THEME, "Nord").unwrap();

        // Re-run initialization the way `Db::open` does on the next start.
        let db = Db::init(old.conn).unwrap();
        assert_eq!(db.schema_version().unwrap(), 2);
        assert_eq!(db.list_items().unwrap(), vec![task]);
        assert_eq!(db.list_completions().unwrap(), vec![completion]);
        assert_eq!(
            db.get_setting(settings_keys::THEME).unwrap().as_deref(),
            Some("Nord")
        );
        // The new table exists and starts empty.
        let rows: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM sync_docs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn insert_get_update_round_trip() {
        let db = Db::open_in_memory().unwrap();
        let mut item = Item::new(Kind::Event, "Christmas", now());
        item.date = Some(d(2026, 12, 25));
        item.repeat = RepeatRule::Yearly;
        item.notes = Some("buy gifts".into());
        item.keep_after_pass = true;
        db.insert_item(&item).unwrap();

        let loaded = db.get_item(&item.id).unwrap().unwrap();
        assert_eq!(loaded, item);

        let later = now() + chrono::TimeDelta::hours(1);
        let mut edited = loaded.clone();
        edited.title = "Xmas".into();
        edited.repeat = RepeatRule::EveryN {
            n: 2,
            unit: Unit::Weeks,
        };
        db.update_item(&mut edited, later).unwrap();
        let reloaded = db.get_item(&item.id).unwrap().unwrap();
        assert_eq!(reloaded.title, "Xmas");
        assert_eq!(
            reloaded.repeat,
            RepeatRule::EveryN {
                n: 2,
                unit: Unit::Weeks
            }
        );
        assert_eq!(
            reloaded.updated_at, later,
            "updated_at must change on every edit"
        );
        assert_eq!(reloaded.created_at, item.created_at);
    }

    #[test]
    fn soft_delete_hides_but_keeps_row() {
        let db = Db::open_in_memory().unwrap();
        let item = Item::new(Kind::Task, "gone", now());
        db.insert_item(&item).unwrap();
        db.delete_item(&item.id, now()).unwrap();
        assert!(db.list_items().unwrap().is_empty());
        assert!(db.get_item(&item.id).unwrap().unwrap().deleted);
        assert!(matches!(
            db.delete_item(&item.id, now()),
            Err(DbError::NotFound(_))
        ));
    }

    #[test]
    fn completing_a_plain_task_logs_it() {
        let mut db = Db::open_in_memory().unwrap();
        let mut task = Item::new(Kind::Task, "pay rent", now());
        task.date = Some(d(2026, 10, 7)); // yesterday -> overdue
        task.priority = Priority::High;
        db.insert_item(&task).unwrap();

        let next = db.complete_task(&task.id, d(2026, 10, 8), now()).unwrap();
        assert!(next.is_none());
        assert!(db.get_item(&task.id).unwrap().unwrap().is_completed());

        let log = db.list_completions().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].item_id, task.id);
        assert_eq!(log[0].title_snapshot, "pay rent");
        assert_eq!(log[0].priority_snapshot, Priority::High);
        assert!(log[0].was_overdue);

        // Completing again does nothing.
        assert!(
            db.complete_task(&task.id, d(2026, 10, 8), now())
                .unwrap()
                .is_none()
        );
        assert_eq!(db.list_completions().unwrap().len(), 1);
    }

    #[test]
    fn completing_a_repeating_task_creates_next_instance() {
        let mut db = Db::open_in_memory().unwrap();
        let mut task = Item::new(Kind::Task, "water plants", now());
        task.date = Some(d(2026, 10, 8));
        task.repeat = RepeatRule::Weekly;
        task.notes = Some("the big fern too".into());
        db.insert_item(&task).unwrap();

        let next = db
            .complete_task(&task.id, d(2026, 10, 8), now())
            .unwrap()
            .expect("weekly task should create a new instance");
        assert_ne!(next.id, task.id);
        assert_eq!(next.date, Some(d(2026, 10, 15)));
        assert_eq!(next.title, "water plants");
        assert_eq!(next.notes.as_deref(), Some("the big fern too"));
        assert!(!next.is_completed());

        // Both rows exist: the old one as history, the new one open.
        let items = db.list_items().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items.iter().filter(|i| i.is_completed()).count(), 1);
        assert!(!db.list_completions().unwrap()[0].was_overdue);
    }

    #[test]
    fn settings_upsert() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_setting(settings_keys::THEME).unwrap(), None);
        db.set_setting(settings_keys::THEME, "Nord").unwrap();
        db.set_setting(settings_keys::THEME, "Dracula").unwrap();
        assert_eq!(
            db.get_setting(settings_keys::THEME).unwrap().as_deref(),
            Some("Dracula")
        );
    }

    #[test]
    fn opening_a_non_database_file_is_a_friendly_error() {
        let dir = std::env::temp_dir().join(format!("zerohour-test-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not-a-db.db");
        std::fs::write(
            &path,
            "this is definitely not sqlite, just some text padding it out",
        )
        .unwrap();
        let err = Db::open(&path).err().expect("should fail");
        assert!(matches!(err, DbError::Open { .. }), "got {err:?}");
        assert!(err.to_string().contains("not-a-db.db"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_database_persists_between_opens() {
        let dir = std::env::temp_dir().join(format!("zerohour-test-{}", new_id()));
        let path = dir.join("nested").join("z.db"); // nested folder gets created
        {
            let db = Db::open(&path).unwrap();
            db.insert_item(&Item::new(Kind::Task, "persist me", now()))
                .unwrap();
        } // `db` dropped here -> connection closed
        let db = Db::open(&path).unwrap();
        assert_eq!(db.list_items().unwrap()[0].title, "persist me");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
