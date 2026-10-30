//! The command line (docs/architecture.md §11a): `claude-sync status` and
//! `claude-sync personal enable|disable|status`. Handled before Tauri starts, like
//! `run-job`, and calling the same core functions as the Settings screen.
use crate::ctx::{now_unix, Ctx};
use crate::guard;
use claude_sync_core::engine::GroupStatus;
use claude_sync_core::personal::PersonalStatus;
use claude_sync_core::Paths;
use serde_json::json;

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
            let login = ctx.login();
            let expired = ctx.expired_login();
            let (groups, groups_error) = match ctx.engine() {
                Ok(e) => match e.groups().await {
                    Ok(g) => (g, None),
                    Err(err) => (Vec::new(), Some(err.to_string())),
                },
                Err(_) => (Vec::new(), None),
            };
            let personal = ctx.engine().ok().map(|e| e.personal_status());
            let requests = claude_sync_core::github::read_request_counts(&paths.requests_file());
            if json {
                return serde_json::to_string_pretty(&json!({
                    "requests": requests,
                    "login": login,
                    "expiredLogin": expired,
                    "groups": groups,
                    "groupsError": groups_error,
                    "personal": personal,
                }))
                .map_err(|e| e.to_string());
            }
            let mut out = status_text(
                login.as_deref(),
                expired.as_deref(),
                &groups,
                personal.as_ref(),
                now,
            );
            if let Some(e) = groups_error {
                out.push_str(&format!("\nCould not read the groups: {e}"));
            }
            out.push_str(&requests_text(requests.as_ref()));
            Ok(out)
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

/// "5 min ago", "never".
pub fn ago(now: u64, then: Option<u64>) -> String {
    let Some(t) = then else {
        return "never".into();
    };
    let s = now.saturating_sub(t);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

pub fn personal_text(p: &PersonalStatus, now: u64) -> String {
    if !p.enabled {
        return "Personal memory: off".into();
    }
    let k = &p.kinds;
    let kinds: Vec<&str> = [
        (k.claude_md, "CLAUDE.md"),
        (k.memory, "memory"),
        (k.state, "state"),
        (k.settings, "settings"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    let mut out = format!(
        "Personal memory: on ({})\n  kinds: {}\n  last sync: {}",
        p.repo,
        if kinds.is_empty() {
            "none".into()
        } else {
            kinds.join(", ")
        },
        ago(now, p.last_sync_unix)
    );
    if let Some(e) = &p.last_error {
        out.push_str(&format!("\n  last error: {e}"));
    }
    out
}

/// The top 10 GitHub requests of the last 10 minutes, as the running app counted them.
pub fn requests_text(requests: Option<&claude_sync_core::github::RequestSnapshot>) -> String {
    let Some(r) = requests else {
        return String::new();
    };
    let mut out = String::from(
        "
GitHub requests, last 10 min (count, endpoint, result, caller):",
    );
    if r.last10min.is_empty() {
        out.push_str(
            "
  none",
        );
    }
    for line in claude_sync_core::github::top_lines(&r.last10min, 10) {
        out.push_str(&format!(
            "
  {line}"
        ));
    }
    out
}

pub fn status_text(
    login: Option<&str>,
    expired: Option<&str>,
    groups: &[GroupStatus],
    personal: Option<&PersonalStatus>,
    now: u64,
) -> String {
    let mut out = match (login, expired) {
        (Some(l), _) => format!("Signed in as {l}"),
        (None, Some(l)) => format!("Sign-in expired for {l}: open the app to sign in again"),
        (None, None) => return "Not signed in".into(),
    };
    if groups.is_empty() {
        out.push_str("\nGroups: none");
    } else {
        out.push_str("\nGroups:");
    }
    for g in groups {
        let state = if g.disabled {
            "off on this device"
        } else {
            "on"
        };
        out.push_str(&format!(
            "\n  {} ({state}), last sync {}",
            g.id,
            ago(now, g.last_sync_unix)
        ));
        if let Some(e) = &g.last_error {
            out.push_str(&format!("\n    last error: {e}"));
        }
    }
    if let Some(p) = personal {
        out.push('\n');
        out.push_str(&personal_text(p, now));
    }
    out
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
    use claude_sync_core::personal::Kinds;

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

    #[test]
    fn status_shows_the_top_requests() {
        use claude_sync_core::github::{RequestEntry, RequestSnapshot};
        let row = |count, endpoint: &str| RequestEntry {
            caller: "tick".into(),
            endpoint: endpoint.into(),
            result: "200".into(),
            count,
        };
        let snap = RequestSnapshot {
            since: 0,
            last10min: (1..=12).map(|n| row(n, &format!("GET e{n}"))).collect(),
            total: vec![],
        };
        let text = requests_text(Some(&snap));
        assert!(text.contains("GET e1  200  (tick)"));
        assert_eq!(text.lines().count(), 1 + 1 + 10, "heading and 10 rows");
        assert_eq!(requests_text(None), "");
        // The same file the app writes is what `status --json` embeds.
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["last10min"][0]["endpoint"], "GET e1");
    }

    fn personal(enabled: bool) -> PersonalStatus {
        PersonalStatus {
            enabled,
            decided: true,
            offer: false,
            kinds: Kinds {
                settings: false,
                ..Kinds::default()
            },
            repo: "me/claude-sync-personal".into(),
            repo_url: String::new(),
            device_id: String::new(),
            devices: vec![],
            new_devices_join: None,
            last_sync_unix: Some(1000),
            last_error: Some("offline".into()),
            rate_limited_until: None,
            conflicts: vec![],
            skipped: vec![],
        }
    }

    #[test]
    fn personal_text_on_and_off() {
        assert_eq!(
            personal_text(&personal(false), 2000),
            "Personal memory: off"
        );
        let t = personal_text(&personal(true), 1000 + 300);
        assert!(t.contains("on (me/claude-sync-personal)"));
        assert!(t.contains("kinds: CLAUDE.md, memory, state\n"));
        assert!(t.contains("last sync: 5 min ago"));
        assert!(t.contains("last error: offline"));
    }

    #[test]
    fn status_text_signed_out_and_in() {
        assert_eq!(status_text(None, None, &[], None, 0), "Not signed in");
        assert!(status_text(None, Some("me"), &[], None, 0).contains("expired for me"));
        let t = status_text(Some("me"), None, &[], Some(&personal(false)), 0);
        assert_eq!(t, "Signed in as me\nGroups: none\nPersonal memory: off");
    }

    #[test]
    fn ago_text() {
        assert_eq!(ago(10, None), "never");
        assert_eq!(ago(100, Some(90)), "just now");
        assert_eq!(ago(10_000, Some(10_000 - 7200)), "2 h ago");
        assert_eq!(ago(300_000, Some(0)), "3 d ago");
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
