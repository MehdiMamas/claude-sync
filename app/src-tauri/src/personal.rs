//! Personal memory (docs/architecture.md §16): the commands behind Settings → Personal memory,
//! and the pass that runs after each group sync. Groups never see any of it.
use crate::ctx::{lock, Ctx};
use crate::guard::{self, Error};
use crate::notify;
use claude_sync_core::personal::{Kinds, NewDevice, PersonalStatus, JOINED_NOTICE, OFFER_NOTICE};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

type Res<T> = Result<T, Error>;
type CtxState<'a> = State<'a, Arc<Ctx>>;

/// The account whose new-device check already ran in this session.
static CHECKED: Mutex<Option<String>> = Mutex::new(None);

/// After a group sync: once per sign-in, see whether this account's other devices share
/// personal memory; then sync it. `full` = the user asked (sync even if nothing changed).
pub async fn after_sync(app: &AppHandle, full: bool) {
    let ctx = app.state::<Arc<Ctx>>().inner().clone();
    let (Some(login), Ok(engine)) = (ctx.login(), ctx.engine()) else {
        return;
    };
    let check = lock(&CHECKED).as_deref() != Some(login.as_str());
    if check {
        match guard::call_async("personal memory check", engine.personal_check_new_device()).await {
            Ok(found) => {
                *lock(&CHECKED) = Some(login);
                match found {
                    NewDevice::Joined => {
                        notify::show(
                            app,
                            notify::Kind::Personal,
                            None,
                            "Personal memory is on",
                            JOINED_NOTICE,
                        );
                    }
                    // The window shows the offer from the status (`offer`).
                    NewDevice::Offer => notify::show(
                        app,
                        notify::Kind::Personal,
                        None,
                        "Personal memory",
                        OFFER_NOTICE,
                    ),
                    NewDevice::Nothing => {}
                }
                if found != NewDevice::Nothing {
                    emit(app, &engine.personal_status());
                }
            }
            // Offline or rate limited: try again after the next sync.
            Err(e) => tracing::info!("personal memory check: {e}"),
        }
    }
    let Ok(Some(report)) =
        guard::call_async("personal memory sync", engine.personal_sync(!full)).await
    else {
        return;
    };
    apply_app_choices(app);
    for c in &report.conflicts {
        notify::show(
            app,
            notify::Kind::Personal,
            None,
            "Conflict in your personal memory",
            &format!(
                "{}: the version from your other device won; yours was saved.",
                c.path
            ),
        );
    }
    emit(app, &engine.personal_status());
}

/// Pause, the check interval and start at login chosen on another device take effect here.
fn apply_app_choices(app: &AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let ctx = app.state::<std::sync::Arc<crate::ctx::Ctx>>();
    let Ok(c) = claude_sync_core::choices::load(&ctx.paths) else {
        return;
    };
    let a = c.app;
    if a.ts == 0 && !a.paused && a.interval_secs.is_none() && a.start_at_login.is_none() {
        return;
    }
    let fresh = claude_sync_core::state::AppState::load(&ctx.paths).ok();
    if let Err(e) = ctx.update_state(|s| {
        // The engine may have joined or left groups meanwhile; keep what it saved.
        if let Some(f) = fresh {
            s.groups = f.groups;
            s.disabled = f.disabled;
        }
        s.paused = a.paused;
        if a.interval_secs.is_some() {
            s.poll_seconds = a.interval_secs;
        }
    }) {
        tracing::warn!("could not apply the settings choice: {e}");
    }
    if let Some(on) = a.start_at_login {
        let al = app.autolaunch();
        if al.is_enabled().unwrap_or(on) != on {
            let r = if on { al.enable() } else { al.disable() };
            if let Err(e) = r {
                tracing::warn!("could not change start at login: {e}");
            }
        }
    }
    crate::background::refresh_status(app);
    ctx.wake.notify_one();
}

fn emit(app: &AppHandle, status: &PersonalStatus) {
    let _ = app.emit("personal", status);
}

fn engine(ctx: &Ctx) -> Res<Arc<claude_sync_core::Engine>> {
    ctx.engine()
}

#[tauri::command]
pub fn personal_status(ctx: CtxState) -> Res<PersonalStatus> {
    Ok(engine(&ctx)?.personal_status())
}

#[tauri::command]
pub async fn personal_enable(app: AppHandle, ctx: CtxState<'_>) -> Res<PersonalStatus> {
    let e = engine(&ctx)?;
    let st = guard::call_async("turn on personal memory", e.personal_enable()).await?;
    emit(&app, &st);
    for c in &st.conflicts {
        tracing::info!(path = %c.path, saved = %c.saved_to, "personal memory conflict");
    }
    Ok(st)
}

#[tauri::command]
pub async fn personal_disable(app: AppHandle, ctx: CtxState<'_>) -> Res<PersonalStatus> {
    let e = engine(&ctx)?;
    let st = guard::call_async("turn off personal memory", e.personal_disable()).await?;
    emit(&app, &st);
    Ok(st)
}

#[tauri::command]
pub async fn personal_set_kinds(
    app: AppHandle,
    ctx: CtxState<'_>,
    kinds: Kinds,
) -> Res<PersonalStatus> {
    let e = engine(&ctx)?;
    let st = guard::call_async("personal memory kinds", e.personal_set_kinds(kinds)).await?;
    emit(&app, &st);
    ctx.wake.notify_one();
    Ok(st)
}

#[tauri::command]
pub async fn personal_set_new_devices_join(
    app: AppHandle,
    ctx: CtxState<'_>,
    join: bool,
) -> Res<PersonalStatus> {
    let e = engine(&ctx)?;
    let st = guard::call_async("new devices join", e.personal_set_new_devices_join(join)).await?;
    emit(&app, &st);
    Ok(st)
}

#[tauri::command]
pub async fn personal_remove_device(
    app: AppHandle,
    ctx: CtxState<'_>,
    id: String,
) -> Res<PersonalStatus> {
    let e = engine(&ctx)?;
    let st = guard::call_async("remove a device", e.personal_remove_device(&id)).await?;
    emit(&app, &st);
    Ok(st)
}

#[tauri::command]
pub async fn personal_dismiss_offer(ctx: CtxState<'_>) -> Res<()> {
    let e = engine(&ctx)?;
    guard::call_async("personal memory offer", e.personal_dismiss_offer()).await
}
