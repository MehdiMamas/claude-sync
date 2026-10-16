//! Notifications: the one place the app tells the user something happened.
//!
//! Every notification goes through [`show`]:
//!
//! ```ignore
//! notify::show(app, Kind::Group, Some(&group_id), "Group access ended", &body);
//! notify::show(app, Kind::Other, None, "Feedback sent", &body);
//! ```
//!
//! It records the notification in the history (`claude_sync_core::notifications`,
//! `~/.claude-sync/notifications.json`) first, then shows the desktop notification unless
//! the user turned them off in Settings, then emits `notifications-changed` (payload
//! [`Changed`]) so the window and the tray update. The history is kept either way, so a
//! missed notification can be read in the app's Notifications screen.
//!
//! Clicking a desktop notification opens the window on that screen. The plugin reports
//! the click on Windows while the toast is on screen (not from the Action Center) and on
//! macOS; see [`init`].
use crate::ctx::Ctx;
use crate::{tray, window};
use claude_sync_core::notifications;
pub use claude_sync_core::notifications::Kind;
use claude_sync_core::GroupId;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

/// The route of the Notifications screen (`index.html#/notifications`).
pub const ROUTE: &str = "notifications";

/// Payload of the `notifications-changed` event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changed {
    pub unread: usize,
}

/// Record, show and announce one notification. Never fails: problems are logged.
pub fn show(app: &AppHandle, kind: Kind, group: Option<&GroupId>, title: &str, body: &str) {
    let ctx = app.state::<Arc<Ctx>>();
    if let Err(e) = notifications::record(&ctx.paths, kind, group.cloned(), title, body) {
        tracing::warn!("notification not saved to the history: {e}");
    }
    if ctx.prefs().desktop_notifications {
        if let Err(e) = app.notification().builder().title(title).body(body).show() {
            tracing::warn!("notification failed: {e}");
        }
    }
    changed(app);
}

/// The history changed (new entry, marked read, cleared): update the tray and the window.
pub fn changed(app: &AppHandle) {
    let unread = notifications::unread_count(&app.state::<Arc<Ctx>>().paths);
    tray::set_unread(app, unread);
    let _ = app.emit("notifications-changed", Changed { unread });
}

/// Set the tray item from the saved history and open the window on the Notifications
/// screen when a desktop notification is clicked.
pub fn init(app: &AppHandle) {
    changed(app);
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
