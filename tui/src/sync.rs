//! The TUI side of sync: the status shown in the header, and the background worker that does
//! the slow parts (network, keyring) so the screen never freezes.
//!
//! Only compiled with the `sync` cargo feature. All the actual sync logic is in
//! `zerohour_core::sync`; this file just runs it on another thread and reports back.
//!
//! **How the background work flows:**
//! 1. The app calls [`SyncUi::start`] with a [`Job`]. That spawns a thread (`std::thread`) and
//!    keeps the receiving end of a *channel* (`mpsc`: a queue one thread writes into and another
//!    reads from).
//! 2. The thread does the work and sends [`WorkerMsg`]s into the channel, then ends.
//! 3. About every 200 ms while a job runs, the main loop calls [`SyncUi::poll`], which reads
//!    whatever arrived without waiting. The UI thread never blocks on the network.
//!
//! Every failure (offline, wrong password, no keyring...) becomes a message on screen. Nothing
//! here can crash the app or stop you from using it.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use chrono::{DateTime, Utc};
use zerohour_core::db::Db;
use zerohour_core::sync::{
    self, ConnectionInfo, SecretStore, SyncError, SyncReport, SyncSettings, default_store,
    load_password,
};

/// What the header's sync indicator shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncState {
    /// Sync is set up but has never succeeded on this device.
    Never,
    /// A sync is running right now.
    Syncing,
    /// The last sync succeeded at this time.
    Synced(DateTime<Utc>),
    /// The server couldn't be reached (not shown as an error: it's normal on a train).
    Offline,
    /// The last sync failed; the message says why.
    Error(String),
}

/// Something for the background worker to do.
#[derive(Debug, Clone)]
pub enum Job {
    /// Find out where passwords are stored and whether one is saved for these settings.
    Probe,
    /// Check the server, login and database. Uses `password` if given, else the saved one.
    Test { password: Option<String> },
    /// After the settings were saved: store the newly typed password (if any), then sync if
    /// sync is switched on. One job, so the sync can't start before the password is stored.
    Save {
        password: Option<String>,
        then_sync: bool,
    },
}

/// What the background worker reports back.
#[derive(Debug)]
pub enum WorkerMsg {
    /// Where passwords are stored (e.g. "OS keyring"), and whether one is saved.
    Storage {
        description: String,
        has_password: bool,
    },
    /// The password was saved (or removed).
    PasswordSaved,
    /// Result of "test connection".
    Tested(Result<ConnectionInfo, String>),
    /// Result of a sync.
    Synced(Result<SyncReport, SyncError>),
    /// Something went wrong with the password store.
    Failed(String),
}

/// Sync state for the TUI: settings, status, and the running background job (if any).
pub struct SyncUi {
    /// The saved sync settings.
    pub settings: SyncSettings,
    /// The database file the worker opens its own connection to. `None` in demo mode, where
    /// sync is never allowed (sample data must not reach your server).
    pub db_path: Option<PathBuf>,
    /// What the header shows.
    pub state: SyncState,
    /// Where the password is stored, once the worker has checked (`None` = not known yet).
    pub storage: Option<String>,
    /// Whether a password is saved for the current settings (`None` = not known yet).
    pub has_password: Option<bool>,
    /// Result of the last "test connection": (worked?, message).
    pub test_result: Option<(bool, String)>,
    /// Seconds this clock differs from the server's, if more than a minute.
    pub clock_skew_secs: Option<i64>,
    /// Number of documents synced, shown in settings.
    pub synced_docs: u64,
    /// The channel from the running job, while one runs.
    receiver: Option<Receiver<WorkerMsg>>,
}

impl SyncUi {
    /// Loads the saved settings and last sync time.
    pub fn new(db: &Db, db_path: Option<PathBuf>) -> anyhow::Result<SyncUi> {
        let state = match sync::last_synced(db)? {
            Some(time) => SyncState::Synced(time),
            None => SyncState::Never,
        };
        Ok(SyncUi {
            settings: SyncSettings::load(db)?,
            db_path,
            state,
            storage: None,
            has_password: None,
            test_result: None,
            clock_skew_secs: None,
            synced_docs: db.synced_doc_count()?,
            receiver: None,
        })
    }

    /// True while a background job runs.
    pub fn busy(&self) -> bool {
        self.receiver.is_some()
    }

    /// True if the header should show the sync indicator at all.
    pub fn visible(&self) -> bool {
        self.settings.enabled && self.db_path.is_some()
    }

    /// Starts `job` in the background with `settings` (which may be unsaved, for "test
    /// connection" from the form). Returns `false` if another job is still running.
    pub fn start(&mut self, job: Job, settings: SyncSettings) -> bool {
        if self.busy() {
            return false;
        }
        if matches!(
            job,
            Job::Save {
                then_sync: true,
                ..
            }
        ) {
            self.state = SyncState::Syncing;
        }
        let db_path = self.db_path.clone();
        let (sender, receiver) = mpsc::channel();
        // `move` gives the closure ownership of `sender`, `job`, etc., so the new thread can
        // keep using them after this function returns.
        thread::spawn(move || run_job(job, &settings, db_path, &sender));
        self.receiver = Some(receiver);
        true
    }

    /// Takes every message the worker has sent so far, without waiting. Returns them so the
    /// app can react (e.g. reload the lists after a sync). Updates the status as it goes.
    pub fn poll(&mut self) -> Vec<WorkerMsg> {
        let Some(receiver) = &self.receiver else {
            return Vec::new();
        };
        let mut messages = Vec::new();
        loop {
            match receiver.try_recv() {
                Ok(message) => messages.push(message),
                Err(TryRecvError::Empty) => break,
                // The worker finished and dropped its end of the channel.
                Err(TryRecvError::Disconnected) => {
                    self.receiver = None;
                    break;
                }
            }
        }
        for message in &messages {
            self.apply(message);
        }
        messages
    }

    /// Updates the visible state from one worker message.
    fn apply(&mut self, message: &WorkerMsg) {
        match message {
            WorkerMsg::Storage {
                description,
                has_password,
            } => {
                self.storage = Some(description.clone());
                self.has_password = Some(*has_password);
            }
            WorkerMsg::PasswordSaved | WorkerMsg::Failed(_) => {}
            WorkerMsg::Tested(Ok(info)) => {
                self.clock_skew_secs = info.clock_skew_secs;
                self.test_result = Some((
                    true,
                    format!("Connected. The database has {} documents.", info.doc_count),
                ));
            }
            WorkerMsg::Tested(Err(message)) => self.test_result = Some((false, message.clone())),
            WorkerMsg::Synced(Ok(report)) => {
                self.clock_skew_secs = report.clock_skew_secs;
                self.state = SyncState::Synced(Utc::now());
            }
            WorkerMsg::Synced(Err(error)) if error.is_offline() => self.state = SyncState::Offline,
            WorkerMsg::Synced(Err(error)) => self.state = SyncState::Error(error.to_string()),
        }
    }

    /// The state to show after the settings changed (e.g. a new server: "never synced").
    pub fn settings_changed(&mut self, settings: SyncSettings, db: &Db) -> anyhow::Result<()> {
        self.settings = settings;
        self.has_password = None;
        self.test_result = None;
        self.synced_docs = db.synced_doc_count()?;
        Ok(())
    }
}

/// The body of the background thread: does one job and reports through `sender`.
///
/// Send errors are ignored (`let _ =`): they only happen if the app already quit and nobody
/// is listening anymore.
fn run_job(
    job: Job,
    settings: &SyncSettings,
    db_path: Option<PathBuf>,
    sender: &Sender<WorkerMsg>,
) {
    // Finding the store may talk to the OS keyring, which is why it happens here.
    let Some(store) = default_store() else {
        let _ = sender.send(WorkerMsg::Failed(
            "no keyring and no home folder: can't store the password".into(),
        ));
        return;
    };
    let report_storage = |sender: &Sender<WorkerMsg>| {
        let has_password = matches!(store.get(&settings.account()), Ok(Some(p)) if !p.is_empty());
        let _ = sender.send(WorkerMsg::Storage {
            description: store.describe(),
            has_password,
        });
    };

    match job {
        Job::Probe => report_storage(sender),
        Job::Save {
            password,
            then_sync,
        } => {
            if let Some(password) = password {
                match store.set(&settings.account(), &password) {
                    Ok(()) => {
                        let _ = sender.send(WorkerMsg::PasswordSaved);
                    }
                    Err(e) => {
                        let _ = sender.send(WorkerMsg::Failed(e.to_string()));
                    }
                }
            }
            report_storage(sender);
            if then_sync {
                run_sync(store.as_ref(), settings, db_path, sender);
            }
        }
        Job::Test { password } => {
            let result = match password.filter(|p| !p.is_empty()) {
                Some(password) => Ok(password),
                None => load_password(store.as_ref(), settings),
            }
            .and_then(|password| sync::test_connection(settings, &password, Utc::now()));
            let _ = sender.send(WorkerMsg::Tested(result.map_err(|e| e.to_string())));
        }
    }
}

/// Runs one full sync and reports the result.
fn run_sync(
    store: &dyn SecretStore,
    settings: &SyncSettings,
    db_path: Option<PathBuf>,
    sender: &Sender<WorkerMsg>,
) {
    let result = match db_path {
        None => Err(SyncError::Config("sync is off in demo mode".into())),
        Some(path) => load_password(store, settings)
            .and_then(|password| sync::sync_database(&path, settings, &password, Utc::now())),
    };
    let _ = sender.send(WorkerMsg::Synced(result));
}
