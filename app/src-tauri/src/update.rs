//! App updates (v2-FlowUpdate, v2-StateUpdate, Parity 51): `latest.json` of the newest GitHub
//! release is read at start and every 6 hours; a newer bundle downloads in the background and
//! `tauri-plugin-updater` checks its minisign signature (the pubkey in `tauri.conf.json`); a bad
//! signature is thrown away. A verified bundle installs when the app is idle, on "Restart to
//! update" or on Quit. The engine only records the state (`Command::SetUpdate`).
use claude_sync_actions::{Command, Event, Handle};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

const LATEST: &str =
    "https://github.com/MehdiMamas/claude-sync/releases/latest/download/latest.json";
const EVERY: Duration = Duration::from_secs(6 * 3600);
const TICK: Duration = Duration::from_secs(60);

/// The verified download waiting to be installed.
#[derive(Default)]
pub struct Pending(Mutex<Option<(Update, Vec<u8>)>>);

/// An install must not interrupt a sync or a window in use.
pub fn quiet(syncing: bool, window_open: bool) -> bool {
    !syncing && !window_open
}

async fn record(h: &Handle, state: String) {
    let _ = h.send(Command::SetUpdate { state }).await;
}

/// Check now; a newer version downloads and is verified before this returns. Returns the state
/// the window shows: empty (up to date), `ready:<v>` or `failed:<v>`. A check that cannot reach
/// GitHub is an error and leaves the state alone.
pub async fn check_now(app: &AppHandle, h: &Handle) -> Result<String, String> {
    let url = LATEST.parse().map_err(|e| format!("bad update URL: {e}"))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        record(h, String::new()).await;
        return Ok(String::new());
    };
    let v = update.version.clone();
    if app
        .state::<Pending>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .is_some_and(|(u, _)| u.version == v)
    {
        return Ok(format!("ready:{v}"));
    }
    record(h, format!("available:{v}")).await;
    // the plugin verifies the signature while it downloads
    let state = match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => {
            *app.state::<Pending>().0.lock().map_err(|e| e.to_string())? = Some((update, bytes));
            format!("ready:{v}")
        }
        Err(e) => {
            tracing::warn!("update {v} was thrown away: {e}");
            format!("failed:{v}")
        }
    };
    record(h, state.clone()).await;
    Ok(state)
}

/// Install the waiting bundle (the app restarts after it on `restart`).
fn install(app: &AppHandle) -> bool {
    let taken = app
        .state::<Pending>()
        .0
        .lock()
        .ok()
        .and_then(|mut p| p.take());
    let Some((update, bytes)) = taken else {
        return false;
    };
    match update.install(bytes) {
        Ok(()) => true,
        Err(e) => {
            tracing::error!("update install failed: {e}");
            false
        }
    }
}

/// "Restart to update".
pub fn restart_to_update(app: &AppHandle) -> Result<(), String> {
    if install(app) {
        app.restart();
    }
    Err("no update is ready".into())
}

/// Tray "Quit": install a waiting update first.
pub fn quit(app: &AppHandle) {
    install(app);
    app.exit(0);
}

/// Start, then every 6 hours: check; every minute: install a waiting update when idle.
pub fn spawn(app: AppHandle, h: Handle) {
    tauri::async_runtime::spawn(async move {
        let mut since = EVERY;
        loop {
            if since >= EVERY {
                since = Duration::ZERO;
                if let Err(e) = check_now(&app, &h).await {
                    tracing::warn!("update check failed: {e}");
                }
            }
            let waiting = app.state::<Pending>().0.lock().is_ok_and(|p| p.is_some());
            let open = app.get_webview_window(crate::window::LABEL).is_some();
            if waiting && quiet(h.snapshot().syncing, open) {
                if let Err(e) = restart_to_update(&app) {
                    tracing::warn!("{e}");
                }
            }
            tokio::time::sleep(TICK).await;
            since += TICK;
        }
    });
}

/// What `send` answers for `CheckUpdate`: the same check, as an event.
pub async fn answer(app: &AppHandle, h: &Handle) -> Result<Event, String> {
    check_now(app, h).await.map(Event::Text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_only_when_idle() {
        assert!(quiet(false, false));
        assert!(!quiet(true, false));
        assert!(!quiet(false, true));
    }
}
