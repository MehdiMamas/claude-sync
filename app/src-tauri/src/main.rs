#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! claude-sync desktop app (docs/architecture.md §11): tray, background sync, UI.

mod auth;
mod background;
mod commands;
mod config;
mod ctx;
mod feedback;
mod guard;
mod jobs;
mod notify;
mod personal;
mod tray;
mod updates;
mod window;

use claude_sync_core::notifications::Kind;
use claude_sync_core::Paths;
use ctx::Ctx;
use std::sync::Arc;
use tauri::{RunEvent, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// Log to `<state>/logs` (§11). If that fails the app still runs, logging to stderr.
/// The returned guard flushes the log when dropped.
fn init_logging(paths: &Paths) -> Option<Box<dyn std::any::Any + Send>> {
    match claude_sync_core::logging::init(paths) {
        Ok(log_guard) => Some(log_guard),
        Err(e) => {
            let _ = tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .try_init();
            tracing::warn!("could not open the log file, logging to stderr: {e}");
            None
        }
    }
}

/// The headless subcommands (§17): started by the OS scheduler, "Run now" and the
/// uninstaller. They run before Tauri starts, so there is no tray, no window and no
/// single-instance check. `None`: start the app.
fn headless(paths: &Paths) -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some(claude_sync_core::jobs::RUN_JOB_ARG) => Some(jobs::run_job_main(paths, &args[1..])),
        Some(jobs::REMOVE_JOBS_ARG) => Some(jobs::remove_jobs_main(paths)),
        _ => None,
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

/// Move a v0 clone out of `~/.claude-sync` (§10) before the app reads its state.
/// Skipped when `CLAUDE_SYNC_HOME` points somewhere else. A failure is only logged:
/// the app then keeps its state in `~/.claude-sync/app`.
fn migrate_v0() -> Option<claude_sync_core::migrate::Migrated> {
    let set = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    if set("CLAUDE_SYNC_HOME").is_some() {
        return None;
    }
    let home = std::env::home_dir()?;
    let claude_home = set("CLAUDE_HOME").map_or_else(|| home.join(".claude"), Into::into);
    match claude_sync_core::migrate::migrate_v0(&home, &claude_home) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("claude-sync: could not move the v0 setup: {e}");
            None
        }
    }
}

fn main() {
    guard::install_panic_hook();
    let migrated = migrate_v0();
    let paths = match Paths::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("claude-sync: {e}");
            std::process::exit(1);
        }
    };
    let log_guard = init_logging(&paths);
    if let Some(code) = headless(&paths) {
        drop(log_guard);
        std::process::exit(code);
    }
    // The guard must live as long as the app.
    std::mem::forget(log_guard);
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
            commands::transfer_group,
            commands::leave_group,
            commands::reorder_groups,
            commands::local_edits,
            commands::resolve_edit,
            commands::group_contents,
            commands::local_skills,
            commands::share_skill,
            commands::claude_md_preview,
            commands::share_claude_md,
            commands::recent_commits,
            commands::revert_preview,
            commands::revert_commit,
            commands::claude_md_review,
            commands::decide_claude_md,
            commands::set_claude_md_review,
            commands::set_group_auto_accept_claude_md,
            commands::set_group_proposal_mode,
            commands::list_proposals,
            commands::approve_proposal,
            commands::decline_proposal,
            commands::use_group_claude_md,
            commands::conflicts,
            commands::claude_md_check,
            commands::drop_claude_md_line,
            commands::edit_claude_md_line,
            commands::keep_setting,
            commands::copy_preview,
            commands::copy_to_group,
            commands::sync_now,
            commands::get_settings,
            commands::set_settings,
            commands::notifications,
            commands::mark_notification_read,
            commands::mark_all_notifications_read,
            commands::clear_notifications,
            commands::test_notification,
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
            commands::jobs,
            commands::accept_job,
            commands::set_job_enabled,
            commands::set_jobs_auto_accept,
            commands::run_job_now,
            commands::open_job_file,
            commands::plugin_warnings,
            commands::retry_plugins,
            commands::helper_status,
            commands::helper_save_key,
            commands::helper_remove_key,
            commands::helper_test_key,
            commands::helper_explain,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            auth::init(&handle);
            first_run_autostart(&handle, &ctx);
            #[cfg(debug_assertions)]
            if let Some(token) = auth::dev_token() {
                auth::sign_in_with_dev_token(&handle, token);
            }
            tray::build(&handle)?;
            notify::init(&handle);
            if let Some(m) = &migrated {
                notify::show(&handle, Kind::Other, None, "Old setup moved", &m.message());
            }
            background::refresh_status(&handle);
            background::start(&handle);
            jobs::reconcile_soon(&handle, true);
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
