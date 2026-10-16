//! Self-update through the relay (§13, issue #27). The check runs from Rust because the
//! relay needs the user's GitHub token as a header. A found update downloads in the
//! background and installs on Quit or when the user clicks "Restart to update".
use crate::config;
use crate::ctx::{lock, Ctx};
use crate::guard::Error;
use crate::notify;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

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
        return Ok(info(None));
    };
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
                let _ = a.emit("update-available", &info);
                notify::show(
                    &a,
                    notify::Kind::Update,
                    None,
                    "claude-sync update ready",
                    &format!(
                        "Version {} installs when you quit, or restart now from Settings.",
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

/// Tray "Quit": install a downloaded update first, then exit.
pub fn quit(app: &AppHandle) {
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
    app.exit(0);
}
