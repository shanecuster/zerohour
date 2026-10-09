//! Where the sync password is kept. Never in the SQLite file: that file gets backed up, copied
//! between machines, and opened with `sqlite3`.
//!
//! [`SecretStore`] is a small trait so each platform can plug in its own storage:
//! - **OS keyring** ([`KeyringStore`], only with the `keyring` cargo feature): Secret Service on
//!   Linux (GNOME Keyring / KWallet), Keychain on macOS, Credential Manager on Windows.
//!   + Encrypted on disk, unlocked when you log in.
//!   - Needs a running keyring service; headless or SSH-only machines often have none.
//!   - Once unlocked, any program running as you can still ask for the password.
//! - **A file with `0600` permissions** ([`FileStore`]): `~/.config/zerohour/sync-password`.
//!   + Works everywhere, no extra services.
//!   - Plain text: any program running as you can read it, and it ends up in your backups.
//!   - `0600` (owner read/write only) keeps *other users* out. On Windows there are no Unix
//!     permission bits, so it relies on your user profile folder's permissions instead.
//! - **Android** (phase 3) will implement the trait with the Android Keystore.
//!
//! Neither option protects against malware running as you. [`default_store`] picks the keyring
//! when it works and falls back to the file automatically; the settings screen shows which one
//! is in use (via [`SecretStore::describe`]).

use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// Something that can store one password per account.
///
/// `Send` means a store can be moved to another thread (sync runs in the background).
pub trait SecretStore: Send {
    /// The saved password for `account`, or `None` if there isn't one.
    fn get(&self, account: &str) -> Result<Option<String>, SecretError>;
    /// Saves (or replaces) the password for `account`.
    fn set(&self, account: &str, password: &str) -> Result<(), SecretError>;
    /// Removes the password for `account` (no error if there wasn't one).
    fn delete(&self, account: &str) -> Result<(), SecretError>;
    /// A short description of where passwords go, shown in the settings screen.
    fn describe(&self) -> String;
}

/// A password storage failure, with a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("password storage: {0}")]
pub struct SecretError(pub String);

/// The account name passwords are stored under: `user @ server-url/database`.
///
/// Including the server means a password saved for one server is never sent to another one
/// after you change the URL.
pub fn account_name(username: &str, remote_key: &str) -> String {
    format!("{} @ {remote_key}", username.trim())
}

// ---- file store ------------------------------------------------------------------------

/// Stores one password in a text file readable only by you (`0600`).
///
/// File format: the account name on the first line, the password on the second. If the
/// account doesn't match (you changed server or user), the saved password is ignored.
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    /// A store using the given file.
    pub fn new(path: PathBuf) -> FileStore {
        FileStore { path }
    }

    /// The default file: `~/.config/zerohour/sync-password` on Linux (the platform's config
    /// folder elsewhere). `None` if the OS doesn't tell us a home folder.
    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", crate::APP_NAME)
            .map(|dirs| dirs.config_dir().join("sync-password"))
    }
}

impl SecretStore for FileStore {
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SecretError(format!("{}: {e}", self.path.display()))),
        };
        // `split_once('\n')` cuts the text at the first newline into (before, after).
        Ok(match text.split_once('\n') {
            Some((saved_account, password)) if saved_account == account => {
                Some(password.trim_end_matches('\n').to_string())
            }
            _ => None,
        })
    }

    fn set(&self, account: &str, password: &str) -> Result<(), SecretError> {
        if password.contains('\n') || account.contains('\n') {
            return Err(SecretError("line breaks aren't allowed".into()));
        }
        let fail = |e: std::io::Error| SecretError(format!("{}: {e}", self.path.display()));
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(fail)?;
        }
        // Write a temporary file and rename it over the real one: a rename is atomic, so a
        // crash can never leave a half-written password file behind.
        let tmp = self.path.with_extension("tmp");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        // On Unix, create the file with mode 0600 right away. Setting permissions *after*
        // creating it would leave a short moment where other users could open it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).map_err(fail)?;
        // If the temp file already existed with looser permissions, `mode` didn't apply.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(fail)?;
        }
        file.write_all(format!("{account}\n{password}\n").as_bytes())
            .map_err(fail)?;
        file.sync_all().map_err(fail)?;
        fs::rename(&tmp, &self.path).map_err(fail)
    }

    fn delete(&self, _account: &str) -> Result<(), SecretError> {
        match fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(SecretError(format!("{}: {e}", self.path.display())))
            }
            _ => Ok(()),
        }
    }

    fn describe(&self) -> String {
        format!("file {} (only you can read it)", self.path.display())
    }
}

// ---- keyring store ---------------------------------------------------------------------

/// Stores passwords in the OS keyring. Only built with the `keyring` cargo feature.
#[cfg(feature = "keyring")]
#[derive(Debug, Clone, Default)]
pub struct KeyringStore;

#[cfg(feature = "keyring")]
impl KeyringStore {
    /// The keyring entry for an account, under the service name "zerohour".
    fn entry(account: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(crate::APP_NAME, account).map_err(|e| SecretError(e.to_string()))
    }

    /// True if the keyring can actually be used right now (a service is running and answers).
    ///
    /// Asking for a password that doesn't exist is a harmless way to find out: "no such
    /// entry" means the keyring works; any other error means it doesn't.
    pub fn is_available() -> bool {
        match KeyringStore::entry("availability-check") {
            Ok(entry) => matches!(entry.get_password(), Ok(_) | Err(keyring::Error::NoEntry)),
            Err(_) => false,
        }
    }
}

#[cfg(feature = "keyring")]
impl SecretStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        match KeyringStore::entry(account)?.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError(e.to_string())),
        }
    }

    fn set(&self, account: &str, password: &str) -> Result<(), SecretError> {
        KeyringStore::entry(account)?
            .set_password(password)
            .map_err(|e| SecretError(e.to_string()))
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        match KeyringStore::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError(e.to_string())),
        }
    }

    fn describe(&self) -> String {
        "OS keyring".to_string()
    }
}

/// The best password store on this machine: the OS keyring if it's built in and working,
/// otherwise the `0600` file. `None` only if there's no keyring *and* no home folder.
///
/// This may talk to the keyring service (and the OS may ask you to unlock it), so call it from
/// a background thread, not while drawing the screen.
pub fn default_store() -> Option<Box<dyn SecretStore>> {
    #[cfg(feature = "keyring")]
    if KeyringStore::is_available() {
        return Some(Box::new(KeyringStore));
    }
    FileStore::default_path().map(|path| Box::new(FileStore::new(path)) as Box<dyn SecretStore>)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (FileStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("zerohour-secret-{}", crate::model::new_id()));
        (
            FileStore::new(dir.join("nested").join("sync-password")),
            dir,
        )
    }

    #[test]
    fn file_store_round_trip_and_account_check() {
        let (store, dir) = temp_store();
        assert_eq!(store.get("me @ server").unwrap(), None, "no file yet");
        store.set("me @ server", "placeholder-secret").unwrap();
        assert_eq!(
            store.get("me @ server").unwrap().as_deref(),
            Some("placeholder-secret")
        );
        // A different account (e.g. after changing server) never gets this password.
        assert_eq!(store.get("me @ other-server").unwrap(), None);
        store.set("me @ server", "changed").unwrap();
        assert_eq!(
            store.get("me @ server").unwrap().as_deref(),
            Some("changed")
        );
        store.delete("me @ server").unwrap();
        assert_eq!(store.get("me @ server").unwrap(), None);
        store.delete("me @ server").unwrap(); // deleting twice is fine
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn file_store_is_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let (store, dir) = temp_store();
        store.set("me @ server", "placeholder-secret").unwrap();
        let mode = fs::metadata(&store.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn line_breaks_are_rejected() {
        let (store, _dir) = temp_store();
        assert!(store.set("me", "two\nlines").is_err());
    }
}
