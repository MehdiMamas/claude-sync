//! The command line (docs/architecture.md §11a): `claude-sync status` and
//! `claude-sync personal enable|disable|status`. Handled before Tauri starts, like
//! `run-job`, and calling the same core functions as the Settings screen.
use crate::ctx::{now_unix, Ctx};
use crate::guard;
use claude_sync_core::report::personal_text;
use claude_sync_core::Paths;

/// Sent to the running app after a change so its screens reload.
pub const REFRESH_ARG: &str = "--refresh";

/// Subcommands this module handles.
pub const COMMANDS: [&str; 2] = ["status", "personal"];

const USAGE: &str = "usage: claude-sync status [--json]\n       claude-sync personal enable|disable|status [--json]";

#[derive(Debug, PartialEq, Eq)]
pub enum Cmd {
    Status,
    PersonalStatus,
    PersonalEnable,
    PersonalDisable,
}

/// The command and `--json`, or the usage text.
pub fn parse(args: &[String]) -> Result<(Cmd, bool), &'static str> {
    let json = args.iter().any(|a| a == "--json");
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json")
        .collect();
    let cmd = match words.as_slice() {
        ["status"] => Cmd::Status,
        ["personal", "status"] => Cmd::PersonalStatus,
        ["personal", "enable"] => Cmd::PersonalEnable,
        ["personal", "disable"] => Cmd::PersonalDisable,
        _ => return Err(USAGE),
    };
    Ok((cmd, json))
}

/// Run a command. Exit code: 0 done, 1 failed, 2 wrong usage.
pub fn run(paths: &Paths, args: &[String]) -> i32 {
    let (cmd, json) = match parse(args) {
        Ok(v) => v,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("claude-sync: {e}");
            return 1;
        }
    };
    match rt.block_on(execute(paths, cmd, json)) {
        Ok(out) => {
            println!("{out}");
            0
        }
        Err(e) => {
            eprintln!("claude-sync: {e}");
            1
        }
    }
}

async fn execute(paths: &Paths, cmd: Cmd, json: bool) -> Result<String, String> {
    let ctx = Ctx::load(paths.clone());
    let now = now_unix();
    match cmd {
        Cmd::Status => {
            let engine = ctx.engine().ok();
            let report = claude_sync_core::report::StatusReport::gather(
                paths,
                ctx.login(),
                ctx.expired_login(),
                engine.as_deref(),
            )
            .await;
            if json {
                return serde_json::to_string_pretty(&report.json()).map_err(|e| e.to_string());
            }
            Ok(report.text(now))
        }
        Cmd::PersonalStatus | Cmd::PersonalEnable | Cmd::PersonalDisable => {
            let engine = ctx.engine()?;
            let st = match cmd {
                Cmd::PersonalEnable => {
                    guard::call_async("turn on personal memory", engine.personal_enable()).await?
                }
                Cmd::PersonalDisable => {
                    guard::call_async("turn off personal memory", engine.personal_disable()).await?
                }
                _ => engine.personal_status(),
            };
            if cmd != Cmd::PersonalStatus {
                refresh_running_app();
            }
            if json {
                return serde_json::to_string_pretty(&st).map_err(|e| e.to_string());
            }
            Ok(personal_text(&st, now))
        }
    }
}

/// In the running app, on `--refresh`: send the personal status to the open screens.
pub fn refresh(app: &tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    let ctx = app.state::<std::sync::Arc<Ctx>>();
    if let Ok(engine) = ctx.engine() {
        let _ = app.emit("personal", engine.personal_status());
    }
}

/// Ask the running app (if any) to reload what it shows. Launching the binary with
/// `--refresh` reaches the single-instance handler; with no app running it would start
/// one, so check first.
fn refresh_running_app() {
    if !app_running() {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg(REFRESH_ARG).spawn();
    }
}

/// Another process running this binary as the app (not a CLI call or a job).
#[cfg(unix)]
fn app_running() -> bool {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-axo", "pid=,args="])
        .output()
    else {
        return false;
    };
    let me = std::process::id().to_string();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| is_app_line(l, &me))
}

/// A `ps` line `<pid> <exe path> [args]`: the app has no subcommand.
#[cfg(unix)]
fn is_app_line(line: &str, my_pid: &str) -> bool {
    let line = line.trim_start();
    let Some((pid, rest)) = line.split_once(' ') else {
        return false;
    };
    // The file name ends the program path: `/claude-sync` followed by a space or the end
    // (not `/claude-sync.app/...`).
    let Some(after) = rest
        .match_indices("/claude-sync")
        .map(|(i, m)| &rest[i + m.len()..])
        .find(|a| a.is_empty() || a.starts_with(' '))
    else {
        return false;
    };
    let sub = after.split_whitespace().next();
    pid != my_pid && matches!(sub, None | Some("--background"))
}

#[cfg(windows)]
fn app_running() -> bool {
    use std::os::windows::process::CommandExt;
    let Ok(out) = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq claude-sync.exe", "/FO", "CSV", "/NH"])
        .creation_flags(0x0800_0000)
        .output()
    else {
        return false;
    };
    let me = std::process::id().to_string();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains("claude-sync.exe"))
        .any(|l| !l.contains(&format!("\"{me}\"")))
}

/// On macOS and Linux: `~/.local/bin/claude-sync` points at this binary, so the command
/// works in a terminal. Only when that folder exists and nothing uses the name.
#[cfg(unix)]
pub fn install_shim() {
    if cfg!(debug_assertions) {
        return;
    }
    let (Some(home), Ok(exe)) = (std::env::home_dir(), std::env::current_exe()) else {
        return;
    };
    let dir = home.join(".local/bin");
    let link = dir.join("claude-sync");
    if dir.is_dir() && std::fs::symlink_metadata(&link).is_err() {
        match std::os::unix::fs::symlink(&exe, &link) {
            Ok(()) => tracing::info!(link = %link.display(), "command line shim added"),
            Err(e) => tracing::warn!("could not add the command line shim: {e}"),
        }
    }
}

#[cfg(not(unix))]
pub fn install_shim() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse(&v(&["status"])), Ok((Cmd::Status, false)));
        assert_eq!(
            parse(&v(&["personal", "enable", "--json"])),
            Ok((Cmd::PersonalEnable, true))
        );
        assert_eq!(
            parse(&v(&["--json", "personal", "disable"])),
            Ok((Cmd::PersonalDisable, true))
        );
        assert_eq!(
            parse(&v(&["personal", "status"])),
            Ok((Cmd::PersonalStatus, false))
        );
        assert!(parse(&v(&["personal"])).is_err());
        assert!(parse(&v(&["personal", "x"])).is_err());
        assert!(parse(&v(&["status", "extra"])).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn finds_the_app_among_processes() {
        let app = "/Applications/claude-sync.app/Contents/MacOS/claude-sync";
        assert!(is_app_line(&format!(" 12 {app}"), "99"));
        assert!(is_app_line(&format!("12 {app} --background"), "99"));
        assert!(!is_app_line(&format!("12 {app}"), "12"));
        assert!(!is_app_line(&format!("12 {app} run-job a/b x"), "99"));
        assert!(!is_app_line(&format!("12 {app} personal status"), "99"));
        assert!(!is_app_line("12 /usr/bin/claude-sync-other", "99"));
    }
}
