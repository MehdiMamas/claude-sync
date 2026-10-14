//! Tray icon and menu (§11): status, Sync now, Open, Pause/Resume, Send feedback, Quit.
use crate::ctx::Ctx;
use crate::{background, updates, window};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

pub struct TrayItems {
    status: MenuItem<Wry>,
    pause: MenuItem<Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let ctx = app.state::<Arc<Ctx>>();
    let status = MenuItem::with_id(app, "status", "claude-sync: starting", false, None::<&str>)?;
    let sync = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open claude-sync", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", pause_label(ctx.paused()), true, None::<&str>)?;
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
            "pause" => {
                let ctx = app.state::<Arc<Ctx>>();
                let paused = !ctx.paused();
                if let Err(e) = ctx.update_state(|s| s.paused = paused) {
                    tracing::warn!("pause not saved: {e}");
                }
                background::refresh_status(app);
                ctx.wake.notify_one();
            }
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
    app.manage(TrayItems { status, pause });
    Ok(())
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
