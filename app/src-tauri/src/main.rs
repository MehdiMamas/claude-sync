#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! claude-sync desktop app (docs/architecture.md §11): tray, background sync, UI.

mod auth;
mod background;
mod commands;
mod config;
mod ctx;
mod feedback;
mod guard;
mod notify;
mod personal;
mod tray;
mod updates;
mod window;

use claude_sync_core::Paths;
use ctx::Ctx;
use std::sync::Arc;
use tauri::{RunEvent, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// Log to `<state>/logs` (§11). If that fails the app still runs, logging to stderr.
fn init_logging(paths: &Paths) {
    match claude_sync_core::logging::init(paths) {
        // The guard flushes the log on drop; it must live as long as the process.
        Ok(log_guard) => std::mem::forget(log_guard),
        Err(e) => {
            let _ = tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .try_init();
            tracing::warn!("could not open the log file, logging to stderr: {e}");
        }
    }
}

/// Turn start-at-login on once, on the first run (§11). Debug builds skip it so a dev
/// run does not register target/debug in the login items; so does
/// `CLAUDE_SYNC_NO_AUTOSTART=1`.
fn first_run_autostart(app: &tauri::AppHandle, ctx: &Ctx) {
    let mut prefs = ctx.prefs();
    if prefs.autostart_initialized
        || cfg!(debug_assertions)
        || std::env::var_os("CLAUDE_SYNC_NO_AUTOSTART").is_some()
    {
        return;
    }
    match app.autolaunch().enable() {
        Ok(()) => tracing::info!("start at login turned on (first run)"),
        Err(e) => tracing::warn!("could not turn on start at login: {e}"),
    }
    prefs.autostart_initialized = true;
    if let Err(e) = ctx.save_prefs(&prefs) {
        tracing::warn!("could not save app prefs: {e}");
    }
}

fn main() {
    guard::install_panic_hook();
    let paths = match Paths::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("claude-sync: {e}");
            std::process::exit(1);
        }
    };
    init_logging(&paths);
    let ctx = Arc::new(Ctx::load(paths));
    tracing::info!(version = config::VERSION, state = %ctx.paths.state_dir.display(), "claude-sync starting");
    let background_start = std::env::args().any(|a| a == config::BACKGROUND_ARG);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            window::open(app, None)
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![config::BACKGROUND_ARG]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(ctx.clone())
        .manage(updates::UpdateSlot::default())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::auth_status,
            commands::start_sign_in,
            commands::cancel_sign_in,
            commands::sign_out,
            commands::status,
            commands::groups,
            commands::create_group,
            commands::invitations,
            commands::accept_invitation,
            commands::discover,
            commands::join,
            commands::add_member,
            commands::remove_member,
            commands::leave_group,
            commands::local_edits,
            commands::resolve_edit,
            commands::group_contents,
            commands::local_skills,
            commands::share_skill,
            commands::recent_commits,
            commands::sync_now,
            commands::get_settings,
            commands::set_settings,
            commands::feedback_logs,
            commands::feedback_capture_screen,
            commands::feedback_attach_file,
            commands::feedback_paste_image,
            commands::feedback_submit,
            commands::check_update,
            commands::update_status,
            commands::install_update,
            commands::open_url,
            commands::open_path,
            personal::personal_status,
            personal::personal_enable,
            personal::personal_disable,
            personal::personal_set_kinds,
            personal::personal_set_new_devices_join,
            personal::personal_remove_device,
            personal::personal_dismiss_offer,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            first_run_autostart(&handle, &ctx);
            #[cfg(debug_assertions)]
            if let Some(token) = auth::dev_token() {
                auth::sign_in_with_dev_token(&handle, token);
            }
            tray::build(&handle)?;
            background::refresh_status(&handle);
            background::start(&handle);
            if !background_start || ctx.auth().is_none() {
                window::open(&handle, None);
            } else {
                window::closed(&handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the app");

    app.run(|app, event| match event {
        // Closing the window must not quit: the app lives in the tray. Quit calls exit(0).
        RunEvent::ExitRequested {
            api, code: None, ..
        } => api.prevent_exit(),
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Destroyed,
            ..
        } if label == window::LABEL => window::closed(app),
        _ => {}
    });
}
