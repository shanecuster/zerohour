//! Desktop notifications through the `notify-send` command (from libnotify).
//!
//! `notify-send` talks to whatever notification daemon the desktop runs (mako, dunst, a shell
//! like Noctalia...) over the D-Bus session bus. Calling the command, instead of speaking D-Bus
//! ourselves, needs no extra Rust library and works with every daemon.
//!
//! When there is no session bus or no daemon (logged out, at a text console, the desktop still
//! starting), `notify-send` exits with an error. That is reported as a failed send, so
//! `zerohour remind` doesn't record the reminder and tries again on its next run.
//!
//! Notifications stay until dismissed (no expire time is passed): what the daemon does with
//! them is the daemon's setting.

use std::process::{Command, Stdio};

use zerohour_core::APP_NAME;

use crate::remind::Notifier;

/// The real notifier: one `notify-send` call per notification.
pub struct NotifySend;

impl Notifier for NotifySend {
    /// Runs `notify-send --app-name=zerohour -- <title> <body>` and waits for it to finish
    /// (it returns as soon as the daemon has accepted the notification).
    fn send(&mut self, title: &str, body: &str) -> Result<(), String> {
        // `Command` builds a process to run. Each `.arg` is passed as one argument as-is (no
        // shell is involved), so quotes or `$` in a task title can't do anything surprising.
        // The `--` ends the options, so a title that starts with "-" isn't read as a flag.
        let output = Command::new("notify-send")
            .arg(format!("--app-name={APP_NAME}"))
            .arg("--")
            .arg(title)
            .arg(body)
            .stdin(Stdio::null())
            .output()
            // The command couldn't even start: most likely libnotify isn't installed.
            .map_err(|e| format!("couldn't run notify-send ({e}); is libnotify installed?"))?;
        if output.status.success() {
            return Ok(());
        }
        // notify-send explains itself on stderr, e.g. "Failed to show notification: Could
        // not connect: No such file or directory". Keep its first line for the log.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().next().unwrap_or("").trim();
        Err(if reason.is_empty() {
            format!("notify-send failed ({})", output.status)
        } else {
            reason.to_string()
        })
    }
}
