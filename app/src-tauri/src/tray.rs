//! Tray icon and menu (§11): status, Sync now, Open, Notifications (N), Pause/Resume,
//! Send feedback, Quit.
use crate::ctx::Ctx;
use crate::{background, notify, updates, window};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

pub struct TrayItems {
    menu: Menu<Wry>,
    status: MenuItem<Wry>,
    pause: MenuItem<Wry>,
    /// "Notifications (N)": in the menu only while N > 0.
    notifications: MenuItem<Wry>,
}

/// Where "Notifications (N)" goes: right after "Open claude-sync".
const NOTIFICATIONS_POSITION: usize = 4;

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let ctx = app.state::<Arc<Ctx>>();
    let status = MenuItem::with_id(app, "status", "claude-sync: starting", false, None::<&str>)?;
    let sync = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open claude-sync", true, None::<&str>)?;
    let notifications =
        MenuItem::with_id(app, "notifications", "Notifications", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", pause_label(ctx.paused()), true, None::<&str>)?;
    let groups_off = MenuItem::with_id(
        app,
        "groups_off",
        "Turn groups off / on",
        true,
        None::<&str>,
    )?;
    let feedback = MenuItem::with_id(app, "feedback", "Send feedback…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(app);
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &sep()?,
            &sync,
            &open,
            &pause,
            &groups_off,
            &sep()?,
            &feedback,
            &quit,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("claude-sync")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "sync" => background::sync_soon(app, true),
            "open" => window::open(app, None),
            "notifications" => window::open(app, Some(notify::ROUTE)),
            "pause" => {
                let ctx = app.state::<Arc<Ctx>>();
                let paused = !ctx.paused();
                if let Err(e) = ctx.update_state(|s| s.paused = paused) {
                    tracing::warn!("pause not saved: {e}");
                }
                if let Err(e) =
                    claude_sync_core::choices::set_app(&ctx.paths, |a| a.paused = paused)
                {
                    tracing::warn!("pause choice not saved: {e}");
                }
                background::refresh_status(app);
                ctx.wake.notify_one();
            }
            "groups_off" => toggle_groups(app),
            "feedback" => window::open(app, Some("feedback")),
            "quit" => updates::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window::open(tray.app_handle(), None);
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(TrayItems {
        menu,
        status,
        pause,
        notifications,
    });
    Ok(())
}

/// Turn every joined group off on this device, or back on when all of them are already off.
fn toggle_groups(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let ctx = app.state::<Arc<Ctx>>();
        let result = async {
            let e = ctx.engine().map_err(|e| e.to_string())?;
            let groups = e.groups().await.map_err(|e| e.to_string())?;
            let enable = !groups.is_empty() && groups.iter().all(|g| g.disabled);
            for g in &groups {
                let report = e
                    .set_group_enabled(&g.id, enable)
                    .await
                    .map_err(|e| e.to_string())?;
                if report.plugins_changed {
                    background::plugins_changed(&app, Some(&g.id));
                }
            }
            Ok::<_, String>(())
        }
        .await;
        if let Err(e) = result {
            tracing::warn!("turn groups off/on failed: {e}");
        }
        background::refresh_status(&app);
    });
}

fn pause_label(paused: bool) -> &'static str {
    if paused {
        "Resume syncing"
    } else {
        "Pause syncing"
    }
}

/// Bring the tray in line with the current status.
pub fn update(app: &AppHandle, status_text: &str, paused: bool) {
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.status.set_text(status_text);
        let _ = items.pause.set_text(pause_label(paused));
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(status_text));
    }
}

/// Show "Notifications (N)" while N notifications are unread; take it out at 0.
pub fn set_unread(app: &AppHandle, unread: usize) {
    let Some(items) = app.try_state::<TrayItems>() else {
        return;
    };
    let shown = items.menu.get(items.notifications.id()).is_some();
    if unread == 0 {
        if shown {
            let _ = items.menu.remove(&items.notifications);
        }
        return;
    }
    let _ = items
        .notifications
        .set_text(format!("Notifications ({unread})"));
    if !shown {
        let _ = items
            .menu
            .insert(&items.notifications, NOTIFICATIONS_POSITION);
    }
}
