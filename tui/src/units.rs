//! The systemd user units that run `zerohour remind` every minute, printed by
//! `zerohour remind --print-units` (both) or `--print-unit service|timer` (one, ready to
//! redirect into a file).
//!
//! zerohour never installs them itself: you save them under `~/.config/systemd/user/` and
//! run `systemctl --user enable --now zerohour-remind.timer`. Nothing on your system changes
//! until you do.
//!
//! - **The service** is `Type=oneshot`: systemd starts `zerohour remind`, which shows what is
//!   due and exits.
//! - **The timer** starts the service at second 0 of every minute. `AccuracySec=1s` stops
//!   systemd from shifting it by up to a minute to save power (its default), and
//!   `Persistent=true` runs a missed start right after the computer is back on.
//!
//! The service needs the D-Bus session address, which the systemd user manager normally
//! already has (see the README if notifications never appear).

use std::path::Path;

/// The two unit files' names. The timer starts the service with the same name.
pub const SERVICE_NAME: &str = "zerohour-remind.service";
/// See [`SERVICE_NAME`].
pub const TIMER_NAME: &str = "zerohour-remind.timer";

/// The `.service` file. `exe` is the `zerohour` binary to run (normally the one printing
/// this); `db` is passed as `--db` when one was given, otherwise the default database is used.
pub fn service(exe: &Path, db: Option<&Path>) -> String {
    let mut command = format!("{} remind", systemd_quote(&exe.to_string_lossy()));
    if let Some(db) = db {
        command.push_str(&format!(" --db {}", systemd_quote(&db.to_string_lossy())));
    }
    // A build inside the project folder moves or disappears with `cargo clean`: say so in
    // the file itself, where it will be seen.
    let warning = if exe.components().any(|part| part.as_os_str() == "target") {
        "# NOTE: this points at a build inside a project's target/ folder. For everyday use,\n\
         # install zerohour (cargo install --path tui) and print the units again.\n"
    } else {
        ""
    };
    format!(
        "# {SERVICE_NAME}: shows the zerohour reminders that are due now, then exits.\n\
         # Started every minute by {TIMER_NAME}.\n\
         {warning}\
         [Unit]\n\
         Description=zerohour reminders (show what is due, then exit)\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={command}\n"
    )
}

/// The `.timer` file.
pub fn timer() -> String {
    format!(
        "# {TIMER_NAME}: runs {SERVICE_NAME} at the start of every minute.\n\
         # Enable with: systemctl --user enable --now {TIMER_NAME}\n\
         [Unit]\n\
         Description=Run zerohour remind every minute\n\
         \n\
         [Timer]\n\
         OnCalendar=*-*-* *:*:00\n\
         AccuracySec=1s\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
    )
}

/// Both files, each under a line saying where to save it, plus the commands to turn them on.
pub fn both(exe: &Path, db: Option<&Path>) -> String {
    format!(
        "# ---- save as ~/.config/systemd/user/{SERVICE_NAME} ----\n\
         {}\n\
         # ---- save as ~/.config/systemd/user/{TIMER_NAME} ----\n\
         {}\n\
         # ---- then ----\n\
         # systemctl --user daemon-reload\n\
         # systemctl --user enable --now {TIMER_NAME}\n\
         # (or save each file directly: zerohour remind --print-unit service > ...)\n",
        service(exe, db),
        timer()
    )
}

/// Makes a path safe inside `ExecStart=`: systemd reads `%` as the start of a placeholder
/// (`%h` = home folder), so a literal one is written `%%`; and a path with spaces, quotes or
/// backslashes goes in double quotes, with `"` and `\` escaped by a backslash.
fn systemd_quote(text: &str) -> String {
    let text = text.replace('%', "%%");
    let needs_quotes = text
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\'' || c == '\\');
    if !needs_quotes {
        return text;
    }
    // `flat_map` turns each character into zero or more characters and joins them up.
    let escaped: String = text
        .chars()
        .flat_map(|c| match c {
            '"' | '\\' => vec!['\\', c],
            _ => vec![c],
        })
        .collect();
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_runs_remind_with_the_given_binary() {
        let text = service(Path::new("/home/me/.cargo/bin/zerohour"), None);
        assert!(text.contains("\nType=oneshot\n"));
        assert!(text.contains("\nExecStart=/home/me/.cargo/bin/zerohour remind\n"));
        assert!(
            !text.contains("NOTE"),
            "an installed binary needs no warning"
        );
    }

    #[test]
    fn a_db_flag_is_passed_on_and_odd_paths_are_quoted() {
        let text = service(
            Path::new("/opt/my apps/zerohour"),
            Some(Path::new("/data/100%/z.db")),
        );
        assert!(
            text.contains("\nExecStart=\"/opt/my apps/zerohour\" remind --db /data/100%%/z.db\n"),
            "{text}"
        );
    }

    #[test]
    fn a_build_in_target_gets_a_warning() {
        let text = service(Path::new("/src/zerohour/target/debug/zerohour"), None);
        assert!(text.contains("# NOTE: this points at a build inside"));
    }

    #[test]
    fn the_timer_fires_every_minute_and_catches_up() {
        let text = timer();
        for line in [
            "OnCalendar=*-*-* *:*:00",
            "AccuracySec=1s",
            "Persistent=true",
            "WantedBy=timers.target",
        ] {
            assert!(text.lines().any(|l| l == line), "missing {line}");
        }
    }

    #[test]
    fn both_names_where_to_save_each_file() {
        let text = both(Path::new("/usr/bin/zerohour"), None);
        assert!(text.contains("~/.config/systemd/user/zerohour-remind.service"));
        assert!(text.contains("~/.config/systemd/user/zerohour-remind.timer"));
        assert!(text.contains("systemctl --user enable --now zerohour-remind.timer"));
    }
}
