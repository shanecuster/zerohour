//! Optional sync between devices through a CouchDB server. Only compiled with the `sync` cargo
//! feature; a normal build has none of this code (and no network code at all).
//!
//! **The big picture.** Every item and every completion is one CouchDB document (`doc.rs`).
//! A sync is:
//! 1. **Check** the database exists and we may use it (`GET /{db}`). We never create or delete
//!    databases: you create it once on the server, and zerohour only touches that one.
//! 2. **Pull** everything that changed on the server since our last checkpoint (`_changes`),
//!    merging each document into SQLite with the conflict rules in `merge.rs`.
//! 3. **Push** everything that changed here since it was last synced (`_bulk_docs`).
//!    If the server says "conflict" (another device wrote in between), pull and push again.
//!
//! The bookkeeping (each document's `_rev`, the `_changes` checkpoint) lives in SQLite, see
//! `store.rs`. The password lives outside SQLite, see `secret.rs`.
//!
//! Module map:
//! - [`transport`] — the HTTP layer (a trait, so tests can use a fake server).
//! - `couch`       — the three CouchDB calls we make.
//! - [`doc`]       — items/completions ⇄ CouchDB documents.
//! - `merge`       — conflict rules (newest wins; deletes and completions are sticky).
//! - `store`       — sync bookkeeping in SQLite.
//! - `engine`      — one full sync: check, pull, push.
//! - [`secret`]    — where the password is kept (keyring or a 0600 file).
//!
//! Nothing here imports ratatui or crossterm, so the Android app can reuse all of it.

mod couch;
pub mod doc;
mod engine;
#[cfg(test)]
mod fake;
mod merge;
pub mod secret;
mod store;
pub mod transport;

use std::path::Path;

use chrono::{DateTime, Utc};

pub use engine::{SyncReport, sync_with};
pub use secret::{SecretError, SecretStore, account_name, default_store};
pub use transport::{Transport, UreqTransport};

use crate::db::{Db, DbError, ts_from_db};

/// Setting keys used by sync, stored in the normal `settings` table.
pub mod keys {
    /// "true" when sync is switched on. Missing = off (the default).
    pub const ENABLED: &str = "sync.enabled";
    /// Server URL, e.g. `https://couchdb.example.ts.net`.
    pub const URL: &str = "sync.url";
    /// Database name on the server.
    pub const DATABASE: &str = "sync.database";
    /// CouchDB username.
    pub const USERNAME: &str = "sync.username";
    /// The `_changes` checkpoint: the `last_seq` of the last page we applied.
    pub const SINCE: &str = "sync.since";
    /// Which server + database the checkpoint and revs belong to (see `SyncSettings::remote_key`).
    pub const REMOTE: &str = "sync.remote";
    /// When the last successful sync finished (RFC 3339 UTC).
    pub const LAST_OK: &str = "sync.last_ok";
}

/// How far (in seconds) this device's clock may differ from the server's before we warn.
pub const CLOCK_SKEW_WARNING_SECS: i64 = 60;

/// The user's sync settings (everything except the password).
///
/// All user-supplied at runtime; there are no built-in servers or defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSettings {
    /// Sync on/off. Off by default.
    pub enabled: bool,
    /// Server URL, e.g. `https://couchdb.example.ts.net` (may include a path prefix).
    pub url: String,
    /// The database name on the server.
    pub database: String,
    /// The CouchDB user to log in as.
    pub username: String,
}

impl SyncSettings {
    /// Reads the settings from the database (missing ones are empty / off).
    pub fn load(db: &Db) -> Result<SyncSettings, DbError> {
        let text =
            |key| -> Result<String, DbError> { Ok(db.get_setting(key)?.unwrap_or_default()) };
        Ok(SyncSettings {
            enabled: db.get_setting(keys::ENABLED)?.as_deref() == Some("true"),
            url: text(keys::URL)?,
            database: text(keys::DATABASE)?,
            username: text(keys::USERNAME)?,
        })
    }

    /// Saves the settings to the database (trimmed of stray spaces).
    pub fn save(&self, db: &Db) -> Result<(), DbError> {
        db.set_setting(keys::ENABLED, if self.enabled { "true" } else { "false" })?;
        db.set_setting(keys::URL, &self.normalized_url())?;
        db.set_setting(keys::DATABASE, self.database.trim())?;
        db.set_setting(keys::USERNAME, self.username.trim())?;
        Ok(())
    }

    /// The URL without surrounding spaces or a trailing `/`, so paths can be appended.
    pub fn normalized_url(&self) -> String {
        self.url.trim().trim_end_matches('/').to_string()
    }

    /// Identifies "this server + this database". If it changes, the saved revs and checkpoint
    /// belong to a different database and must be forgotten.
    pub fn remote_key(&self) -> String {
        format!("{}/{}", self.normalized_url(), self.database.trim())
    }

    /// The name the password is stored under (see [`secret::account_name`]).
    pub fn account(&self) -> String {
        account_name(&self.username, &self.remote_key())
    }

    /// True for `http://` URLs: the password and data then travel unencrypted, unless the
    /// network itself is encrypted (Tailscale, WireGuard, a VPN). The UI shows a warning.
    pub fn is_plain_http(&self) -> bool {
        self.normalized_url()
            .to_ascii_lowercase()
            .starts_with("http://")
    }

    /// Checks the settings make sense before any network call, with a message saying what to
    /// fix. Does not check that the server is reachable (that's "test connection").
    pub fn validate(&self) -> Result<(), SyncError> {
        let url = self.normalized_url();
        let config = |message: &str| Err(SyncError::Config(message.to_string()));
        if url.is_empty() {
            return config("enter the server URL");
        }
        let lower = url.to_ascii_lowercase();
        if !lower.starts_with("https://") && !lower.starts_with("http://") {
            return config("the server URL must start with https:// or http://");
        }
        // `ureq::http::Uri` is a URL parser; we only use it to check the shape.
        let uri: ureq::http::Uri = match url.parse() {
            Ok(uri) => uri,
            Err(_) => return config("the server URL isn't valid"),
        };
        match uri.authority() {
            None => return config("the server URL has no host name"),
            // `user:password@host` URLs would put the password in places like error messages.
            Some(authority) if authority.as_str().contains('@') => {
                return config("put the username in its own field, not in the URL");
            }
            Some(_) => {}
        }
        if url.contains('?') || url.contains('#') {
            return config("the server URL can't contain ? or #");
        }
        if !valid_database_name(self.database.trim()) {
            return config(
                "database names start with a lowercase letter and use only a-z, 0-9 and _$()+-/",
            );
        }
        if self.username.trim().is_empty() {
            return config("enter the username");
        }
        Ok(())
    }
}

/// CouchDB's rule for database names: a lowercase letter first, then lowercase letters,
/// digits, or any of `_ $ ( ) + - /`.
fn valid_database_name(name: &str) -> bool {
    let mut chars = name.chars();
    // `matches!` checks a value against a pattern; `'a'..='z'` is an inclusive range.
    matches!(chars.next(), Some('a'..='z'))
        && chars
            .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '$' | '(' | ')' | '+' | '-' | '/'))
}

/// Everything that can make a sync (or connection test) fail, worded for the user.
///
/// None of these messages ever include the password.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The settings are incomplete or malformed.
    #[error("{0}")]
    Config(String),
    /// No password saved for this server and user.
    #[error("no password saved; enter it in settings (c)")]
    NoPassword,
    /// The server can't be reached right now.
    #[error("offline: {0}")]
    Offline(String),
    /// The secure (HTTPS) connection failed, usually an untrusted certificate.
    #[error("secure connection failed: {0}")]
    Tls(String),
    /// HTTP 401.
    #[error("wrong username or password")]
    Unauthorized,
    /// HTTP 403.
    #[error("this user isn't allowed to use that database")]
    Forbidden,
    /// HTTP 404 on the database.
    #[error("database not found; create it on the server first (zerohour never creates databases)")]
    DbNotFound,
    /// Any other HTTP error status.
    #[error("server error {status}: {reason}")]
    Http { status: u16, reason: String },
    /// The server answered with something we couldn't understand.
    #[error("unexpected answer from the server: {0}")]
    BadResponse(String),
    /// The server refused some documents (e.g. a validation function on the database).
    #[error("the server refused {count} document(s): {reason}")]
    Rejected { count: usize, reason: String },
    /// Reading or writing the password failed.
    #[error(transparent)]
    Secret(#[from] SecretError),
    /// The local database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl SyncError {
    /// True when the problem is just "no connection right now", which the UI shows as
    /// *offline* rather than as an error.
    pub fn is_offline(&self) -> bool {
        matches!(self, SyncError::Offline(_))
    }
}

impl From<rusqlite::Error> for SyncError {
    /// Lets `?` turn raw SQLite errors (from transactions in `engine.rs`) into sync errors.
    fn from(error: rusqlite::Error) -> SyncError {
        SyncError::Db(DbError::from(error))
    }
}

/// The result of "test connection".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// Documents currently in the database.
    pub doc_count: u64,
    /// Seconds this device's clock is ahead (+) or behind (-) the server, if more than
    /// [`CLOCK_SKEW_WARNING_SECS`].
    pub clock_skew_secs: Option<i64>,
}

/// Checks that the server answers, the login works, and the database exists, using any
/// transport (the tests use a fake one).
pub fn check_connection(
    transport: &dyn Transport,
    settings: &SyncSettings,
    now: DateTime<Utc>,
) -> Result<ConnectionInfo, SyncError> {
    settings.validate()?;
    let info = couch::CouchClient::new(transport, settings.database.trim()).db_info()?;
    Ok(ConnectionInfo {
        doc_count: info.doc_count,
        clock_skew_secs: clock_skew(info.server_date.as_deref(), now),
    })
}

/// "Test connection" against the real server. Blocks for up to the request timeout, so call
/// it from a background thread.
pub fn test_connection(
    settings: &SyncSettings,
    password: &str,
    now: DateTime<Utc>,
) -> Result<ConnectionInfo, SyncError> {
    check_connection(&UreqTransport::new(settings, password), settings, now)
}

/// One full sync of the database file at `db_path` with the real server.
///
/// Opens its own connection to the file, so it can run on a background thread while the app
/// keeps using its connection. SQLite handles the two connections safely.
pub fn sync_database(
    db_path: &Path,
    settings: &SyncSettings,
    password: &str,
    now: DateTime<Utc>,
) -> Result<SyncReport, SyncError> {
    let mut db = Db::open(db_path)?;
    sync_with(
        &mut db,
        &UreqTransport::new(settings, password),
        settings,
        now,
    )
}

/// Reads the saved password for these settings from `store`.
pub fn load_password(
    store: &dyn SecretStore,
    settings: &SyncSettings,
) -> Result<String, SyncError> {
    store
        .get(&settings.account())?
        .filter(|p| !p.is_empty())
        .ok_or(SyncError::NoPassword)
}

/// When the last successful sync finished, if ever.
pub fn last_synced(db: &Db) -> Result<Option<DateTime<Utc>>, DbError> {
    db.get_setting(keys::LAST_OK)?
        .as_deref()
        .map(ts_from_db)
        .transpose()
}

/// How far this device's clock is from the server's `Date` header, in seconds (positive =
/// we're ahead), but only if it's more than [`CLOCK_SKEW_WARNING_SECS`]. `None` otherwise or
/// if the header is missing/unreadable.
///
/// Why it matters: conflicts are decided by `updated_at`, so a device whose clock runs fast
/// wins conflicts it shouldn't.
pub fn clock_skew(server_date: Option<&str>, now: DateTime<Utc>) -> Option<i64> {
    // HTTP dates look like "Thu, 08 Oct 2026 14:03:00 GMT", the RFC 2822 format.
    let server = DateTime::parse_from_rfc2822(server_date?).ok()?;
    let seconds = (now - server.with_timezone(&Utc)).num_seconds();
    (seconds.abs() > CLOCK_SKEW_WARNING_SECS).then_some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn settings() -> SyncSettings {
        SyncSettings {
            enabled: true,
            url: " https://couchdb.example.ts.net/ ".into(),
            database: "zerohour".into(),
            username: "tester".into(),
        }
    }

    #[test]
    fn good_settings_validate_and_normalize() {
        let s = settings();
        assert!(s.validate().is_ok());
        assert_eq!(s.normalized_url(), "https://couchdb.example.ts.net");
        assert_eq!(s.remote_key(), "https://couchdb.example.ts.net/zerohour");
        assert!(!s.is_plain_http());
        // A path prefix (CouchDB behind a reverse proxy) is fine.
        let proxied = SyncSettings {
            url: "https://couchdb.example.ts.net/couch".into(),
            ..settings()
        };
        assert!(proxied.validate().is_ok());
    }

    #[test]
    fn plain_http_is_allowed_but_flagged() {
        let s = SyncSettings {
            url: "http://couchdb.example.ts.net:5984".into(),
            ..settings()
        };
        assert!(s.validate().is_ok());
        assert!(s.is_plain_http());
    }

    #[test]
    fn bad_settings_say_what_to_fix() {
        let cases = [
            ("", "zerohour", "tester"),
            ("couchdb.example.ts.net", "zerohour", "tester"),
            ("ftp://couchdb.example.ts.net", "zerohour", "tester"),
            (
                "https://user:placeholder@couchdb.example.ts.net",
                "zerohour",
                "tester",
            ),
            ("https://couchdb.example.ts.net/?x=1", "zerohour", "tester"),
            ("https://couchdb.example.ts.net", "", "tester"),
            ("https://couchdb.example.ts.net", "Zerohour", "tester"),
            ("https://couchdb.example.ts.net", "1db", "tester"),
            ("https://couchdb.example.ts.net", "zero hour", "tester"),
            ("https://couchdb.example.ts.net", "zerohour", "  "),
        ];
        for (url, database, username) in cases {
            let s = SyncSettings {
                enabled: true,
                url: url.into(),
                database: database.into(),
                username: username.into(),
            };
            assert!(
                matches!(s.validate(), Err(SyncError::Config(_))),
                "{url} / {database} / {username}"
            );
        }
        // The error never repeats a password typed into the URL.
        let s = SyncSettings {
            url: "https://user:placeholder@couchdb.example.ts.net".into(),
            ..settings()
        };
        assert!(
            !s.validate()
                .unwrap_err()
                .to_string()
                .contains("placeholder")
        );
    }

    #[test]
    fn database_names_follow_couchdb_rules() {
        assert!(valid_database_name("zerohour"));
        assert!(valid_database_name("team/zero_hour-2$(x)+y"));
        assert!(!valid_database_name("_users"));
        assert!(!valid_database_name(""));
    }

    #[test]
    fn settings_round_trip_through_the_database() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(SyncSettings::load(&db).unwrap(), SyncSettings::default());
        assert!(!SyncSettings::load(&db).unwrap().enabled, "off by default");
        settings().save(&db).unwrap();
        let loaded = SyncSettings::load(&db).unwrap();
        assert!(loaded.enabled);
        assert_eq!(loaded.url, "https://couchdb.example.ts.net");
    }

    #[test]
    fn clock_skew_is_only_reported_beyond_a_minute() {
        let now = Utc.with_ymd_and_hms(2026, 10, 8, 14, 3, 0).unwrap();
        assert_eq!(clock_skew(Some("Thu, 08 Oct 2026 14:03:30 GMT"), now), None);
        assert_eq!(
            clock_skew(Some("Thu, 08 Oct 2026 13:58:00 GMT"), now),
            Some(300),
            "we're 5 minutes ahead"
        );
        assert_eq!(
            clock_skew(Some("Thu, 08 Oct 2026 14:05:00 GMT"), now),
            Some(-120)
        );
        assert_eq!(clock_skew(Some("garbage"), now), None);
        assert_eq!(clock_skew(None, now), None);
    }

    #[test]
    fn password_lookup_uses_the_account_for_this_server() {
        let dir = std::env::temp_dir().join(format!("zerohour-pw-{}", crate::model::new_id()));
        let store = secret::FileStore::new(dir.join("sync-password"));
        assert!(matches!(
            load_password(&store, &settings()),
            Err(SyncError::NoPassword)
        ));
        store
            .set(&settings().account(), "placeholder-secret")
            .unwrap();
        assert_eq!(
            load_password(&store, &settings()).unwrap(),
            "placeholder-secret"
        );
        let other_server = SyncSettings {
            url: "https://other.example.ts.net".into(),
            ..settings()
        };
        assert!(load_password(&store, &other_server).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
