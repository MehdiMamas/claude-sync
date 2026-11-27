#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo)
)]
//! claude-sync desktop shell (v2-Plan step 7): tray, one window, one instance. Every command
//! word is answered by `claude-sync-cli` before Tauri starts; the app itself only draws what the
//! actor (`claude-sync-actions`) publishes and sends it the commands it is given.
mod capture;
#[cfg(target_os = "macos")]
mod mac_notify;
mod tray;
mod update;
mod window;

/// The window route a clicked notification opens.
const ROUTE: &str = "notifications";

use claude_sync_actions::{Command, Event, Handle, Secrets, Snapshot};
use claude_sync_cli::daemon::{single_instance, Instance};
use claude_sync_cli::Ctx;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

const BACKGROUND: &str = "--background";
const REFRESH: &str = "--refresh";

fn ctx() -> Ctx {
    Ctx {
        name: "claude-sync",
        remove_jobs: true,
        refresh: std::env::current_exe().ok(),
        // the app's sign-in is in the keychain, for the window and for `claude-sync status` alike
        secrets: Some(Secrets(Arc::new(claude_sync_github::secrets::Keychain))),
        ..Ctx::cli()
    }
}

/// The app starts only for no words, `--background` (autostart) and `--refresh`; any other word
/// is a command (`status`, `run-job`, `remove-jobs`, ...) and goes to the CLI.
fn is_app_launch(args: &[String]) -> bool {
    args.iter()
        .all(|a| a == BACKGROUND || a == REFRESH || a.starts_with("-psn_"))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The window's one way in: run a command, get its event. Everything else it learns from the
/// `snapshot` event.
#[tauri::command]
async fn send(app: AppHandle, h: State<'_, Handle>, cmd: Command) -> Result<Event, String> {
    // the updater plugin is the shell's, so "Check now" runs here
    if matches!(cmd, Command::CheckUpdate) {
        return update::answer(&app, &h).await;
    }
    h.send(cmd).await.map_err(|e| e.0)
}

/// "Restart to update": installs the verified download and restarts.
#[tauri::command]
fn install_update(app: AppHandle) -> Result<(), String> {
    update::restart_to_update(&app)
}

#[tauri::command]
fn snapshot(h: State<'_, Handle>) -> Snapshot {
    h.snapshot()
}

/// The two system pages the notification help opens (Parity 48); no other scheme is opened.
const SETTINGS_URIS: [&str; 2] = [
    "x-apple.systempreferences:com.apple.Notifications-Settings.extension",
    "ms-settings:notifications",
];

fn opens_as_url(t: &str) -> bool {
    t.starts_with("https://") || t.starts_with("http://") || SETTINGS_URIS.contains(&t)
}

/// Open a web address, a notification settings page or a file in the system handler (Parity 59).
/// Only `http(s)` addresses, exactly those two settings URIs and paths that exist.
/// `app/ui/src/fake.ts` `openTarget` applies the same list.
#[tauri::command]
fn open_target(app: AppHandle, target: String) -> Result<(), String> {
    let o = app.opener();
    if opens_as_url(&target) {
        o.open_url(target, None::<&str>).map_err(|e| e.to_string())
    } else if std::path::Path::new(&target).exists() {
        o.open_path(target, None::<&str>).map_err(|e| e.to_string())
    } else {
        Err("that file does not exist".into())
    }
}

/// macOS goes through `UNUserNotificationCenter`: the plugin's macOS path never asks for
/// permission, so nothing would show (v1 found this); other systems use the plugin.
fn desktop(app: &AppHandle, title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    if mac_notify::show(title, body).is_ok() {
        return;
    }
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!("notification failed: {e}");
    }
}

/// Whether the system lets the app show notifications: `granted`, `denied`, `not_asked` or
/// `unknown` (not macOS, or not running from an app bundle). The window shows help on `denied`.
#[tauri::command]
async fn notification_permission() -> &'static str {
    #[cfg(target_os = "macos")]
    return tauri::async_runtime::spawn_blocking(mac_notify::permission)
        .await
        .unwrap_or("unknown");
    #[cfg(not(target_os = "macos"))]
    "unknown"
}

/// Clicking a notification opens the window on the Notifications screen (macOS: the delegate in
/// `mac_notify`; Windows: the plugin reports the click while the toast is on screen).
fn on_click(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    mac_notify::init(app);
    let a = app.clone();
    let registered = app.notification().on_action(move |action| {
        if action.action_id() == "tap" {
            let b = a.clone();
            let _ = a.run_on_main_thread(move || window::open(&b, Some(ROUTE)));
        }
    });
    if let Err(e) = registered {
        tracing::warn!("notification clicks are not handled: {e}");
    }
}

/// Tell the window and the tray on every change, and show new notifications on the desktop.
fn pump(app: AppHandle, h: Handle) {
    tauri::async_runtime::spawn(async move {
        let mut rx = h.watch();
        let mut sent: Option<Snapshot> = None;
        let mut seen = h.snapshot().notifications.iter().map(|n| n.id).max();
        let mut clock = tokio::time::interval(Duration::from_secs(30));
        loop {
            let s = rx.borrow_and_update().clone();
            tray::apply(&app, &s, now());
            if sent.as_ref() != Some(&s) {
                let _ = app.emit("snapshot", &s);
                for n in s.notifications.iter().filter(|n| Some(n.id) > seen) {
                    if !n.read && s.desktop_notifications {
                        desktop(&app, "claude-sync", &n.text);
                    }
                }
                seen = seen.max(s.notifications.iter().map(|n| n.id).max());
                sent = Some(s);
            }
            tokio::select! {
                r = rx.changed() => if r.is_err() { break },
                _ = clock.tick() => {}
            }
        }
    });
}

/// First run: start at login, except in debug builds or with `CLAUDE_SYNC_NO_AUTOSTART`
/// (Parity 72). The choice is saved, so it is made once.
fn first_run_autostart(app: &AppHandle, h: &Handle) {
    if h.snapshot().start_at_login.is_some()
        || cfg!(debug_assertions)
        || std::env::var_os("CLAUDE_SYNC_NO_AUTOSTART").is_some()
    {
        return;
    }
    let on = app.autolaunch().enable().is_ok();
    let h = h.clone();
    tauri::async_runtime::spawn(async move {
        let _ = h.send(Command::StartAtLogin { on }).await;
    });
}

/// Debug builds only: `CLAUDE_SYNC_DEV_TOKEN` signs in with that token so the app can be tried
/// without an OAuth App; a token GitHub refuses is logged and ignored (as v1). Unlike v1 the
/// token is saved like any sign-in.
#[cfg(debug_assertions)]
fn dev_token_sign_in(h: &Handle) {
    let Some(t) = std::env::var("CLAUDE_SYNC_DEV_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && h.snapshot().login.is_none())
    else {
        return;
    };
    let h = h.clone();
    tauri::async_runtime::spawn(async move {
        let tokens = claude_sync_github::Tokens::plain(&t);
        match h
            .send(Command::SignInDone(claude_sync_actions::Outcome::Tokens(
                tokens,
            )))
            .await
        {
            Ok(_) => tracing::info!("signed in with CLAUDE_SYNC_DEV_TOKEN"),
            Err(e) => tracing::warn!("CLAUDE_SYNC_DEV_TOKEN was not accepted: {}", e.0),
        }
    });
}

/// macOS and Linux release builds: `~/.local/bin/claude-sync` points at this binary, only when
/// that folder exists and the name is free (Parity 65).
#[cfg(unix)]
fn link_cli() {
    if cfg!(debug_assertions) {
        return;
    }
    let (Some(home), Ok(exe)) = (std::env::home_dir(), std::env::current_exe()) else {
        return;
    };
    let dir = home.join(".local/bin");
    let link = dir.join("claude-sync");
    if dir.is_dir() && std::fs::symlink_metadata(&link).is_err() {
        let _ = std::os::unix::fs::symlink(exe, link);
    }
}

#[cfg(not(unix))]
fn link_cli() {}

/// Held for the life of the app: `daemon.lock` and `daemon.owner` say who syncs on this machine.
#[derive(Default)]
struct Owner(Mutex<Option<Instance>>);

fn start(app: &AppHandle, background: bool) -> Result<(), String> {
    let ctx = ctx();
    let home = std::env::home_dir().ok_or("cannot find the home directory")?;
    let state = claude_sync_fs::Home::from_env(&home).state;
    let env = claude_sync_cli::env(&ctx)?;
    let h = tauri::async_runtime::block_on(claude_sync_actions::spawn(env, Duration::from_secs(1)))
        .map_err(|e| e.0)?;
    // The hand-over from a v1 daemon ran when the core opened; a live v2 daemon that holds the
    // lock keeps it and the app goes on without (sync.lock keeps passes apart).
    match single_instance(&state, ctx.version) {
        Ok(Some(i)) => {
            let owner = app.state::<Owner>();
            let mut o = owner.0.lock().map_err(|e| e.to_string())?;
            *o = Some(i);
        }
        Ok(None) => tracing::info!("another daemon holds daemon.lock"),
        Err(e) => tracing::warn!("{e}"),
    }
    app.manage(h.clone());
    first_run_autostart(app, &h);
    #[cfg(debug_assertions)]
    dev_token_sign_in(&h);
    link_cli();
    let s = h.snapshot();
    tray::build(app, &s, now()).map_err(|e| e.to_string())?;
    on_click(app);
    update::spawn(app.clone(), h.clone());
    pump(app.clone(), h);
    if !background || s.login.is_none() {
        window::open(app, None);
    } else {
        window::closed(app);
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if !is_app_launch(&args[1..]) {
        std::process::exit(claude_sync_cli::run_status(args, &ctx()) as i32);
    }
    let background = args.iter().any(|a| a == BACKGROUND);
    let log = std::env::home_dir()
        .and_then(|h| claude_sync_fs::logging::init(&claude_sync_fs::Home::from_env(&h).state));
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // `--refresh`: another program changed the state; reload the screens, open nothing.
            if args.iter().any(|a| a == REFRESH) {
                if let Some(h) = app.try_state::<Handle>() {
                    let h = h.inner().clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = h.send(Command::Refresh).await;
                    });
                }
            } else {
                window::open(app, None);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![BACKGROUND]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Owner::default())
        .manage(update::Pending::default())
        .manage(tray::Shown::default())
        .invoke_handler(tauri::generate_handler![
            send,
            snapshot,
            install_update,
            open_target,
            notification_permission,
            capture::capture_screen
        ])
        .setup(move |app| start(app.handle(), background).map_err(Into::into))
        .build(tauri::generate_context!());
    let app = match app {
        Ok(a) => a,
        Err(e) => {
            eprintln!("claude-sync: {e}");
            std::process::exit(1);
        }
    };
    let mut log = log;
    app.run(move |app, event| match event {
        // Closing the window never quits: the app lives in the tray. Quit calls exit(0).
        RunEvent::ExitRequested {
            api, code: None, ..
        } => api.prevent_exit(),
        RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Destroyed,
            ..
        } if label == window::LABEL => window::closed(app),
        RunEvent::Exit => {
            // the log is flushed when the guard goes
            drop(log.take());
            // removes daemon.owner
            let owner = app.state::<Owner>();
            let taken = owner.0.lock().map(|mut o| o.take());
            drop(taken);
        }
        // Dock icon clicked (macOS) while no window shows.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => window::open(app, None),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_the_two_notification_settings_uris_open_besides_web_addresses() {
        for ok in [
            "https://github.com/a/b",
            "http://localhost:1",
            "ms-settings:notifications",
            "x-apple.systempreferences:com.apple.Notifications-Settings.extension",
        ] {
            assert!(opens_as_url(ok), "{ok}");
        }
        for bad in [
            "ms-settings:",
            "ms-settings:privacy",
            "ms-settings:notifications ",
            "x-apple.systempreferences:",
            "x-apple.systempreferences:com.apple.preference.security",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "/tmp/x",
        ] {
            assert!(!opens_as_url(bad), "{bad}");
        }
    }

    #[test]
    fn only_no_words_and_the_two_flags_start_the_app() {
        assert!(is_app_launch(&v(&[])));
        assert!(is_app_launch(&v(&["--background"])));
        assert!(is_app_launch(&v(&["--refresh"])));
        assert!(!is_app_launch(&v(&["status"])));
        assert!(!is_app_launch(&v(&["run-job", "acme/a", "x"])));
        assert!(!is_app_launch(&v(&["remove-jobs"])));
        assert!(!is_app_launch(&v(&["--json", "status"])));
    }
}
