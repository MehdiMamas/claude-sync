//! `service install|remove|status`: keep the daemon running without a desktop.
//!
//! With systemd running: a user unit `claude-sync.service` (started at boot when lingering
//! is on, restarted on failure). Without it (WSL without systemd, containers, minimal
//! hosts): crontab lines, `@reboot` plus a 5-minute check. A second `daemon` exits at once
//! (it cannot take `daemon.lock`), so the check never starts a duplicate.
use std::path::{Path, PathBuf};
use std::process::Command;

pub const UNIT: &str = "claude-sync.service";
/// Ends every crontab line this program owns.
pub const CRON_MARK: &str = "# claude-sync";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Mode {
    Systemd,
    Cron,
}

/// systemd is the init system and answers (the directory exists only then).
pub fn systemd_running() -> bool {
    Path::new("/run/systemd/system").is_dir()
}

pub fn detect() -> Mode {
    if systemd_running() {
        Mode::Systemd
    } else {
        Mode::Cron
    }
}

/// A word safe in a unit's `ExecStart=`.
fn quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

pub fn unit_text(exe: &Path) -> String {
    format!(
        "[Unit]\nDescription=claude-sync background sync\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nExecStart={} daemon\nRestart=on-failure\nRestartSec=30\n\n[Install]\nWantedBy=default.target\n",
        quote(&exe.to_string_lossy())
    )
}

/// A word safe in a crontab line (`%` starts stdin there).
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''").replace('%', "\\%"))
}

pub fn cron_lines(exe: &Path) -> Vec<String> {
    let cmd = format!(
        "{} daemon >/dev/null 2>&1 {CRON_MARK}",
        sh_quote(&exe.to_string_lossy())
    );
    vec![format!("@reboot {cmd}"), format!("*/5 * * * * {cmd}")]
}

/// `existing` without our lines.
pub fn strip_cron(existing: &str) -> String {
    existing
        .lines()
        .filter(|l| !l.trim_end().ends_with(CRON_MARK))
        .map(|l| format!("{l}\n"))
        .collect()
}

/// `existing` with our lines in place of any older ones.
pub fn merge_cron(existing: &str, exe: &Path) -> String {
    let mut out = strip_cron(existing);
    for l in cron_lines(exe) {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

fn unit_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".config")))
        .ok_or("cannot find the home directory")?;
    Ok(base.join("systemd/user").join(UNIT))
}

fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run {cmd}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "{cmd} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn crontab_read() -> String {
    // No crontab yet makes `crontab -l` fail: that is an empty one.
    run("crontab", &["-l"]).unwrap_or_default()
}

fn crontab_write(text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "cannot run crontab: {e} (install cron, or run `claude-sync-cli daemon` yourself)"
            )
        })?;
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    status
        .success()
        .then_some(())
        .ok_or("crontab refused the new table".into())
}

fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))
}

pub fn install() -> Result<String, String> {
    if cfg!(not(unix)) {
        return Err(
            "`service` is for Linux and macOS servers; on a desktop the app starts at login."
                .into(),
        );
    }
    let exe = exe()?;
    match detect() {
        Mode::Systemd => {
            let path = unit_path()?;
            std::fs::create_dir_all(path.parent().ok_or("bad unit path")?)
                .and_then(|_| std::fs::write(&path, unit_text(&exe)))
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            run("systemctl", &["--user", "daemon-reload"])?;
            run("systemctl", &["--user", "enable", "--now", UNIT])?;
            let mut msg = format!("Installed {} and started it.", path.display());
            // Without lingering a user service stops at logout and starts only at login.
            let user = std::env::var("USER").unwrap_or_default();
            if !user.is_empty() && run("loginctl", &["enable-linger", &user]).is_err() {
                msg.push_str("\nIt runs while you are logged in. To keep it running after logout and start it at boot, run once: sudo loginctl enable-linger $USER");
            }
            Ok(msg)
        }
        Mode::Cron => {
            crontab_write(&merge_cron(&crontab_read(), &exe))?;
            // Cron starts it within 5 minutes; start it now too.
            let started = Command::new(&exe)
                .arg("daemon")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok();
            Ok(format!(
                "systemd is not running here, so the crontab got two lines: at boot and every 5 minutes (a second daemon exits at once).{}",
                if started { "\nThe daemon is started." } else { "" }
            ))
        }
    }
}

pub fn remove() -> Result<String, String> {
    let mut done = Vec::new();
    if systemd_running() {
        let path = unit_path()?;
        if path.exists() {
            // Not running or not enabled is fine: we are removing it anyway.
            let _ = run("systemctl", &["--user", "disable", "--now", UNIT]);
            std::fs::remove_file(&path)
                .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
            let _ = run("systemctl", &["--user", "daemon-reload"]);
            done.push(format!("removed {}", path.display()));
        }
    }
    let table = crontab_read();
    if table.lines().any(|l| l.trim_end().ends_with(CRON_MARK)) {
        crontab_write(&strip_cron(&table))?;
        done.push("removed the crontab lines".into());
    }
    Ok(if done.is_empty() {
        "Nothing was installed.".into()
    } else {
        format!("The service is removed ({}). A daemon that is running now stops at its next restart or with `pkill -f 'claude-sync-cli daemon'`.", done.join(", "))
    })
}

/// A sentence saying what is installed and whether it runs.
pub fn status() -> Result<String, String> {
    let mut lines = Vec::new();
    if systemd_running() {
        let path = unit_path()?;
        if path.exists() {
            let active = run("systemctl", &["--user", "is-active", UNIT])
                .unwrap_or_else(|_| "inactive\n".into());
            lines.push(format!(
                "systemd user unit {}: {}",
                path.display(),
                active.trim()
            ));
        } else {
            lines.push("systemd user unit: not installed".into());
        }
    }
    let n = crontab_read()
        .lines()
        .filter(|l| l.trim_end().ends_with(CRON_MARK))
        .count();
    if n > 0 {
        lines.push(format!("crontab: {n} lines"));
    } else if !systemd_running() {
        lines.push("crontab: not installed".into());
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_runs_the_daemon_and_restarts_it() {
        let t = unit_text(Path::new("/home/me/.local/bin/claude-sync-cli"));
        assert!(t.contains("ExecStart=\"/home/me/.local/bin/claude-sync-cli\" daemon\n"));
        assert!(t.contains("Restart=on-failure"));
        assert!(t.contains("WantedBy=default.target"));
        assert!(t.starts_with("[Unit]\n"));
    }

    #[test]
    fn the_unit_quotes_awkward_paths() {
        let t = unit_text(Path::new("/home/a b/50%/cli"));
        assert!(t.contains("ExecStart=\"/home/a b/50%%/cli\" daemon"));
    }

    #[test]
    fn cron_gets_a_boot_line_and_a_five_minute_line() {
        let lines = cron_lines(Path::new("/home/me/bin/claude-sync-cli"));
        assert_eq!(
            lines,
            vec![
                "@reboot '/home/me/bin/claude-sync-cli' daemon >/dev/null 2>&1 # claude-sync",
                "*/5 * * * * '/home/me/bin/claude-sync-cli' daemon >/dev/null 2>&1 # claude-sync",
            ]
        );
        assert!(cron_lines(Path::new("/it's/cli"))[0].contains(r"'/it'\''s/cli'"));
    }

    #[test]
    fn merging_keeps_other_lines_and_never_doubles_ours() {
        let exe = Path::new("/x/claude-sync-cli");
        let mine = "0 3 * * * backup.sh\n";
        let once = merge_cron(mine, exe);
        assert!(once.starts_with(mine));
        assert_eq!(once.lines().count(), 3);
        let twice = merge_cron(&once, Path::new("/y/claude-sync-cli"));
        assert_eq!(twice.lines().count(), 3);
        assert!(twice.contains("'/y/claude-sync-cli'") && !twice.contains("'/x/"));
        assert_eq!(strip_cron(&twice), mine);
        assert_eq!(strip_cron(""), "");
    }
}
