//! Background work: the sync loop, the local-edit watcher, invitations, updates and the
//! feedback outbox. Nothing here may panic the loop: every core call goes through `guard`.
use crate::config;
use crate::ctx::{lock, now_unix, Ctx, Status};
use crate::guard::{self, Error};
use crate::{feedback, notify, tray, updates};
use claude_sync_core::engine::SyncReport;
use claude_sync_core::watch::WatchHandle;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub fn start(app: &AppHandle) {
    let a = app.clone();
    tauri::async_runtime::spawn(async move { sync_loop(a).await });
    let a = app.clone();
    tauri::async_runtime::spawn(async move { invitations_loop(a).await });
    let a = app.clone();
    tauri::async_runtime::spawn(async move { update_loop(a).await });
    start_watcher(app);
}

async fn sync_loop(app: AppHandle) {
    let ctx = app.state::<Arc<Ctx>>().inner().clone();
    loop {
        // A panic here is already caught by `guard`; this is a last line of defence.
        let a = app.clone();
        if let Err(e) = tauri::async_runtime::spawn(async move { tick(&a).await }).await {
            tracing::error!("sync tick failed: {e}");
        }
        let wait = Duration::from_secs(ctx.poll_seconds());
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = ctx.wake.notified() => {}
        }
    }
}

async fn tick(app: &AppHandle) {
    let _ = run_sync(app, false).await;
    check_local_edits(app);
    feedback::retry_outbox(app).await;
}

/// Run a sync pass in the background (tray "Sync now", after sign-in, ...).
pub fn sync_soon(app: &AppHandle, full: bool) {
    let a = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_sync(&a, full).await;
        check_local_edits(&a);
    });
}

/// One sync pass. `full` = `sync_all` (user asked), else `sync_if_changed` (loop).
pub async fn run_sync(app: &AppHandle, full: bool) -> Result<SyncReport, Error> {
    let ctx = app.state::<Arc<Ctx>>().inner().clone();
    if ctx.auth().is_none() {
        refresh_status(app);
        return Err("Not signed in".into());
    }
    if ctx.paused() && !full {
        refresh_status(app);
        return Err("Syncing is paused".into());
    }
    let engine = ctx.engine()?;
    let _running = ctx.sync_lock.lock().await;
    let last = ctx.status().last_sync_unix;
    publish(
        app,
        Status {
            state: "syncing",
            text: "Syncing…".into(),
            last_sync_unix: last,
            error: None,
        },
    );
    let result = if full {
        guard::call_async("sync", engine.sync_all()).await
    } else {
        guard::call_async("sync", engine.sync_if_changed()).await
    };
    match &result {
        Ok(report) => {
            refresh_watcher(app);
            let _ = app.emit("sync-report", report);
            let errors: Vec<_> = report
                .groups
                .iter()
                .filter_map(|g| g.error.as_ref().map(|e| format!("{}: {e}", g.id)))
                .collect();
            for g in &report.groups {
                for c in &g.conflicts {
                    notify::show(
                        app,
                        "Conflict in a shared file",
                        &format!(
                            "{} ({}): the group's version won; yours was saved.",
                            c.path, g.id
                        ),
                    );
                }
                if g.lost_access {
                    notify::show(app, "Group access ended", &format!("You no longer have access to {}. Its files were removed from this machine (a backup was kept).", g.id));
                }
            }
            if report.groups.iter().any(|g| g.apply.plugins_changed) {
                plugins_changed(app);
            }
            if errors.is_empty() {
                publish(
                    app,
                    Status {
                        state: "ok",
                        text: "Up to date".into(),
                        last_sync_unix: Some(now_unix()),
                        error: None,
                    },
                );
            } else {
                let text = format!("{} group(s) had an error", errors.len());
                publish(
                    app,
                    Status {
                        state: "error",
                        text,
                        last_sync_unix: Some(now_unix()),
                        error: Some(errors.join("\n")),
                    },
                );
            }
        }
        Err(e) => publish(
            app,
            Status {
                state: "error",
                text: "Sync failed".into(),
                last_sync_unix: last,
                error: Some(e.clone()),
            },
        ),
    }
    result
}

/// Status when nothing is running: signed out, paused, or the last result.
pub fn refresh_status(app: &AppHandle) {
    let ctx = app.state::<Arc<Ctx>>();
    let cur = ctx.status();
    let s = if ctx.auth().is_none() {
        Status {
            state: "signedOut",
            text: "Not signed in".into(),
            last_sync_unix: None,
            error: None,
        }
    } else if ctx.paused() {
        Status {
            state: "paused",
            text: "Paused".into(),
            ..cur
        }
    } else if matches!(cur.state, "signedOut" | "paused" | "idle") {
        Status {
            state: "idle",
            text: "Waiting to sync".into(),
            ..cur
        }
    } else {
        cur
    };
    publish(app, s);
}

fn publish(app: &AppHandle, s: Status) {
    let ctx = app.state::<Arc<Ctx>>();
    let paused = ctx.paused();
    tray::update(app, &format!("claude-sync: {}", s.text), paused);
    if ctx.set_status(s.clone()) {
        let _ = app.emit("status", &s);
    }
}

/// The text of the reload hint, shown in the window and as a notification.
pub const PLUGINS_CHANGED: &str =
    "Plugins changed. Run /reload-plugins in open Claude Code sessions, or restart them.";

/// A sync, join or leave changed plugin files or a marketplace entry: open Claude Code
/// sessions only see it after `/reload-plugins` or a restart.
pub fn plugins_changed(app: &AppHandle) {
    let _ = app.emit("plugins-changed", ());
    notify::show(app, "Plugins changed", PLUGINS_CHANGED);
}

/// Look for managed files edited by hand and tell the UI (and the user, for new ones).
pub fn check_local_edits(app: &AppHandle) {
    let ctx = app.state::<Arc<Ctx>>();
    let Ok(engine) = ctx.engine() else { return };
    let Ok(edits) = guard::call("local_edits", || engine.local_edits()) else {
        return;
    };
    let new: Vec<_> = {
        let mut last = lock(&ctx.last_edits);
        if *last == edits {
            return;
        }
        let new = edits
            .iter()
            .filter(|e| !last.contains(e))
            .cloned()
            .collect();
        *last = edits.clone();
        new
    };
    let _ = app.emit("local-edits", &edits);
    if let Some(first) = new.first() {
        let body = if new.len() == 1 {
            format!(
                "{} was changed on this machine. Share it with {}?",
                first.path, first.group
            )
        } else {
            format!("{} shared files were changed on this machine.", new.len())
        };
        notify::show(app, "Local change to a shared file", &body);
    }
}

async fn invitations_loop(app: AppHandle) {
    tokio::time::sleep(Duration::from_secs(5)).await;
    loop {
        let _ = check_invitations(&app).await;
        tokio::time::sleep(Duration::from_secs(config::INVITATIONS_EVERY_SECONDS)).await;
    }
}

pub async fn check_invitations(
    app: &AppHandle,
) -> Result<Vec<claude_sync_core::github::Invitation>, Error> {
    let ctx = app.state::<Arc<Ctx>>().inner().clone();
    let engine = ctx.engine()?;
    let invites = guard::call_async("invitations", engine.invitations()).await?;
    let new: Vec<_> = {
        let mut known = lock(&ctx.known_invitations);
        let new = invites
            .iter()
            .filter(|i| !known.contains(&i.id))
            .cloned()
            .collect();
        *known = invites.iter().map(|i| i.id).collect();
        new
    };
    let _ = app.emit("invitations", &invites);
    for i in new {
        notify::show(
            app,
            "Invitation to a claude-sync group",
            &format!(
                "{} invited you to {}. Open claude-sync to accept.",
                i.inviter, i.repo
            ),
        );
    }
    Ok(invites)
}

async fn update_loop(app: AppHandle) {
    tokio::time::sleep(Duration::from_secs(20)).await;
    loop {
        if let Err(e) = updates::check_and_download(&app).await {
            tracing::info!("update check: {e}");
        }
        tokio::time::sleep(Duration::from_secs(config::UPDATES_EVERY_SECONDS)).await;
    }
}

/// Watch `~/.claude` and the groups for hand edits (2 s debounce, §7). Without a watcher
/// the sync loop still checks on every tick.
fn start_watcher(app: &AppHandle) {
    let ctx = app.state::<Arc<Ctx>>();
    let a = app.clone();
    match claude_sync_core::watch::spawn_watcher(&ctx.paths, move || check_local_edits(&a)) {
        Ok(h) => {
            app.manage(h);
        }
        Err(e) => tracing::warn!("file watcher not started: {e}"),
    }
}

/// Folders created by a sync are watched from now on.
fn refresh_watcher(app: &AppHandle) {
    if let Some(h) = app.try_state::<WatchHandle>() {
        h.refresh();
    }
}
