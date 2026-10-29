//! Self-update through the relay (§13, issue #27). The check runs from Rust because the
//! relay needs the user's GitHub token as a header. A found update downloads in the
//! background and installs on Quit or when the user clicks "Restart to update".
use crate::config;
use crate::ctx::{lock, Ctx};
use crate::guard::Error;
use crate::notify;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The window must have been hidden this long before an update installs by itself.
const QUIET_AFTER: Duration = Duration::from_secs(10 * 60);

/// When the window was last shown (unix seconds; 0 = never).
static LAST_SHOWN: AtomicU64 = AtomicU64::new(0);

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Record that the window is (or was just) shown.
pub fn mark_shown() {
    LAST_SHOWN.store(unix_now(), Ordering::Relaxed);
}

fn last_shown(now: Instant) -> Option<Instant> {
    match LAST_SHOWN.load(Ordering::Relaxed) {
        0 => None,
        t => now.checked_sub(Duration::from_secs(unix_now().saturating_sub(t))),
    }
}

/// A moment when a restart will not bother anyone: no sync running, the window hidden,
/// and last shown at least 10 minutes ago (or never).
pub fn quiet_now(
    sync_running: bool,
    window_visible: bool,
    last_shown: Option<Instant>,
    now: Instant,
) -> bool {
    !sync_running
        && !window_visible
        && last_shown.is_none_or(|t| now.saturating_duration_since(t) >= QUIET_AFTER)
}

/// True when a downloaded update is waiting.
pub fn ready(app: &AppHandle) -> bool {
    lock(&app.state::<UpdateSlot>().0)
        .as_ref()
        .is_some_and(|p| p.bytes.is_some())
}

/// Install the downloaded update if the app is quiet right now.
pub async fn install_if_quiet(app: &AppHandle) {
    if !ready(app) {
        return;
    }
    let ctx = app.state::<Arc<Ctx>>();
    let visible = app
        .get_webview_window(crate::window::LABEL)
        .is_some_and(|w| w.is_visible().unwrap_or(true));
    if visible {
        mark_shown();
    }
    let now = Instant::now();
    if quiet_now(
        ctx.sync_lock.try_lock().is_err(),
        visible,
        last_shown(now),
        now,
    ) {
        if let Err(e) = install_now(app).await {
            tracing::warn!("update install failed: {e}");
        }
    }
}

#[derive(Default)]
pub struct UpdateSlot(Mutex<Option<Pending>>);

struct Pending {
    update: Update,
    bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    pub available: bool,
    pub version: Option<String>,
    pub notes: Option<String>,
    /// Downloaded and ready to install.
    pub downloaded: bool,
}

fn endpoint() -> String {
    config::relay("/api/update?target={{target}}&arch={{arch}}&current_version={{current_version}}")
}

fn info(p: Option<&Pending>) -> UpdateInfo {
    UpdateInfo {
        current: config::VERSION.into(),
        available: p.is_some(),
        version: p.map(|p| p.update.version.clone()),
        notes: p.and_then(|p| p.update.body.clone()),
        downloaded: p.is_some_and(|p| p.bytes.is_some()),
    }
}

async fn check(app: &AppHandle) -> Result<Option<Update>, Error> {
    if !config::relay_configured() {
        return Err(
            "this build has no relay URL (CLAUDE_SYNC_RELAY_URL), so updates are off".into(),
        );
    }
    let auth = app
        .state::<Arc<Ctx>>()
        .auth()
        .ok_or("sign in to check for updates")?;
    let token = auth.session.token().await.map_err(|e| e.to_string())?;
    let url = endpoint()
        .parse()
        .map_err(|e| format!("bad update URL: {e}"))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .and_then(|b| b.header("Authorization", format!("Bearer {token}")))
        .map(|b| b.timeout(Duration::from_secs(60)))
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())?;
    updater.check().await.map_err(|e| e.to_string())
}

/// Check now; if there is a new version, download it in the background.
pub async fn check_and_download(app: &AppHandle) -> Result<UpdateInfo, Error> {
    let Some(update) = check(app).await? else {
        tracing::info!("update check: up to date ({})", config::VERSION);
        return Ok(info(None));
    };
    tracing::info!("update check: {} available, downloading", update.version);
    let slot = app.state::<UpdateSlot>();
    {
        let mut cur = lock(&slot.0);
        if let Some(p) = cur.as_ref() {
            if p.update.version == update.version {
                return Ok(info(Some(p)));
            }
        }
        *cur = Some(Pending {
            update: update.clone(),
            bytes: None,
        });
    }
    let a = app.clone();
    tauri::async_runtime::spawn(async move {
        match update.download(|_, _| {}, || {}).await {
            Ok(bytes) => {
                let slot = a.state::<UpdateSlot>();
                let info = {
                    let mut cur = lock(&slot.0);
                    match cur.as_mut() {
                        Some(p) if p.update.version == update.version => p.bytes = Some(bytes),
                        _ => return,
                    }
                    info(cur.as_ref())
                };
                tracing::info!("update {} downloaded", update.version);
                let _ = a.emit("update-available", &info);
                notify::show(
                    &a,
                    notify::Kind::Update,
                    None,
                    "claude-sync update ready",
                    &format!(
                        "claude-sync {} is ready. It installs at the next quiet moment.",
                        update.version
                    ),
                );
            }
            Err(e) => tracing::warn!("update download failed: {e}"),
        }
    });
    let info = info(lock(&slot.0).as_ref());
    let _ = app.emit("update-available", &info);
    Ok(info)
}

pub fn current(app: &AppHandle) -> UpdateInfo {
    info(lock(&app.state::<UpdateSlot>().0).as_ref())
}

/// "Restart to update".
pub async fn install_now(app: &AppHandle) -> Result<(), Error> {
    let pending = lock(&app.state::<UpdateSlot>().0)
        .take()
        .ok_or("no update is ready")?;
    let bytes = match pending.bytes {
        Some(b) => b,
        None => pending
            .update
            .download(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string())?,
    };
    pending.update.install(bytes).map_err(|e| e.to_string())?;
    app.restart();
}

/// Install a downloaded update, if any, without restarting.
pub fn install_pending(app: &AppHandle) {
    let pending = lock(&app.state::<UpdateSlot>().0).take();
    if let Some(Pending {
        update,
        bytes: Some(bytes),
    }) = pending
    {
        tracing::info!("installing {} on quit", update.version);
        if let Err(e) = update.install(bytes) {
            tracing::error!("update install failed: {e}");
        }
    }
}

/// Tray "Quit": install a downloaded update first, then exit.
pub fn quit(app: &AppHandle) {
    install_pending(app);
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn quiet_now_cases() {
        let now = Instant::now() + 60 * MIN;
        assert!(!quiet_now(true, false, None, now));
        assert!(!quiet_now(false, true, None, now));
        assert!(!quiet_now(false, false, Some(now - 5 * MIN), now));
        assert!(quiet_now(false, false, Some(now - 11 * MIN), now));
        assert!(quiet_now(false, false, None, now));
    }
}
