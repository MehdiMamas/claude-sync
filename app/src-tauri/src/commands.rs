//! Tauri commands: the UI's whole API. Errors are user-facing strings.
use crate::auth::{self, DeviceCodeView};
use crate::background;
use crate::config;
use crate::ctx::{lock, Ctx, Status};
use crate::feedback::{self, FeedbackInput, FeedbackResult, Screenshot};
use crate::guard::{self, Error};
use crate::jobs as app_jobs;
use crate::notify;
use crate::updates::{self, UpdateInfo};
use claude_sync_core::apply::Manifest;
use claude_sync_core::conflicts::{Conflict, Source};
use claude_sync_core::contents::{self, GroupContents};
use claude_sync_core::engine::{
    ClaudeMdReview, ClaudeMdShare, ClaudeMdShared, CommitInfo, Copied, CopyPreview, CopyRequest,
    GroupStatus, RevertPlan, SyncReport,
};
use claude_sync_core::github::Invitation;
use claude_sync_core::jobs::JobsOverview;
use claude_sync_core::notifications::{self, Notification};
use claude_sync_core::watch::{EditAction, LocalEdit};
use claude_sync_core::{group, GroupId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_opener::OpenerExt;

type Res<T> = Result<T, Error>;
type CtxState<'a> = State<'a, Arc<Ctx>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    os: String,
    relay_configured: bool,
    client_id_missing: bool,
    state_dir: String,
}

#[tauri::command]
pub fn app_info(ctx: CtxState) -> AppInfo {
    AppInfo {
        version: config::VERSION.into(),
        os: config::os_label(),
        relay_configured: config::relay_configured(),
        client_id_missing: config::GITHUB_CLIENT_ID.is_empty(),
        state_dir: ctx.paths.state_dir.display().to_string(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    signed_in: bool,
    login: Option<String>,
    /// Signed out because GitHub stopped accepting this login's sign-in.
    expired_login: Option<String>,
    client_id_missing: bool,
}

#[tauri::command]
pub fn auth_status(ctx: CtxState) -> AuthStatus {
    let login = ctx.login();
    AuthStatus {
        signed_in: login.is_some(),
        login,
        expired_login: ctx.expired_login(),
        client_id_missing: config::GITHUB_CLIENT_ID.is_empty(),
    }
}

#[tauri::command]
pub async fn start_sign_in(app: AppHandle) -> Res<DeviceCodeView> {
    auth::start(&app).await
}

#[tauri::command]
pub fn cancel_sign_in(ctx: CtxState) {
    auth::cancel(&ctx);
}

#[tauri::command]
pub fn sign_out(app: AppHandle) -> Res<()> {
    auth::sign_out(&app)
}

#[tauri::command]
pub fn status(ctx: CtxState) -> Status {
    ctx.status()
}

#[tauri::command]
pub async fn groups(ctx: CtxState<'_>) -> Res<Vec<GroupStatus>> {
    let e = ctx.engine()?;
    e.groups().await.map_err(guard::logged("groups"))
}

#[tauri::command]
pub async fn create_group(
    app: AppHandle,
    ctx: CtxState<'_>,
    name: String,
    repo: Option<String>,
    claude_md: Option<ClaudeMdShare>,
) -> Res<GroupStatus> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a group name is needed".into());
    }
    let repo = repo
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| group::DEFAULT_REPO.into());
    let e = ctx.engine()?;
    let g = e
        .create_group(name, &repo, claude_md.as_ref())
        .await
        .map_err(guard::logged("create group"))?;
    background::sync_soon(&app, true);
    Ok(g)
}

#[tauri::command]
pub async fn invitations(app: AppHandle) -> Res<Vec<Invitation>> {
    background::check_invitations(&app).await
}

#[tauri::command]
pub async fn accept_invitation(app: AppHandle, ctx: CtxState<'_>, id: u64) -> Res<GroupStatus> {
    let e = ctx.engine()?;
    let g = e
        .accept_invitation(id)
        .await
        .map_err(guard::logged("accept invitation"))?;
    lock(&ctx.known_invitations).retain(|i| *i != id);
    if g.plugins_changed {
        background::plugins_changed(&app, Some(&g.id));
    }
    background::sync_soon(&app, true);
    Ok(g)
}

#[tauri::command]
pub async fn discover(ctx: CtxState<'_>) -> Res<Vec<GroupId>> {
    let e = ctx.engine()?;
    e.discover().await.map_err(guard::logged("discover groups"))
}

#[tauri::command]
pub async fn join(app: AppHandle, ctx: CtxState<'_>, group: GroupId) -> Res<GroupStatus> {
    let e = ctx.engine()?;
    let g = e.join(&group).await.map_err(guard::logged("join group"))?;
    if g.plugins_changed {
        background::plugins_changed(&app, Some(&g.id));
    }
    background::sync_soon(&app, true);
    Ok(g)
}

fn valid_login(login: &str) -> Res<&str> {
    let l = login.trim().trim_start_matches('@');
    let ok = !l.is_empty()
        && l.len() <= 39
        && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !l.starts_with('-')
        && !l.ends_with('-');
    ok.then_some(l)
        .ok_or_else(|| format!("\"{}\" is not a GitHub login", login.trim()))
}

#[tauri::command]
pub async fn add_member(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
    login: String,
) -> Res<()> {
    let login = valid_login(&login)?;
    let e = ctx.engine()?;
    e.add_member(&group, login)
        .await
        .map_err(guard::logged("add member"))?;
    background::sync_soon(&app, true);
    Ok(())
}

#[tauri::command]
pub async fn remove_member(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
    login: String,
) -> Res<()> {
    let login = valid_login(&login)?;
    let e = ctx.engine()?;
    e.remove_member(&group, login)
        .await
        .map_err(guard::logged("remove member"))?;
    background::sync_soon(&app, true);
    Ok(())
}

#[tauri::command]
pub async fn leave_group(app: AppHandle, ctx: CtxState<'_>, group: GroupId) -> Res<()> {
    let e = ctx.engine()?;
    let report = e
        .leave(&group)
        .await
        .map_err(guard::logged("leave group"))?;
    if report.plugins_changed {
        background::plugins_changed(&app, Some(&group));
    }
    app_jobs::after_leave(&app, &group);
    Ok(())
}

/// Put the joined groups in a new order (first wins collisions, §7) and apply them again.
#[tauri::command]
pub async fn reorder_groups(
    app: AppHandle,
    ctx: CtxState<'_>,
    order: Vec<GroupId>,
) -> Res<SyncReport> {
    let e = ctx.engine()?;
    let _running = ctx.sync_lock.lock().await;
    let report = guard::call_async("reorder groups", e.reorder_groups(order)).await?;
    if report.groups.iter().any(|g| g.apply.plugins_changed) {
        background::plugins_changed(&app, None);
    }
    let _ = app.emit("sync-report", &report);
    Ok(report)
}

#[tauri::command]
pub fn local_edits(ctx: CtxState) -> Res<Vec<LocalEdit>> {
    let e = ctx.engine()?;
    let edits = e.local_edits().map_err(guard::logged("local edits"))?;
    *lock(&ctx.last_edits) = edits.clone();
    Ok(edits)
}

#[tauri::command]
pub async fn resolve_edit(
    app: AppHandle,
    ctx: CtxState<'_>,
    edit: LocalEdit,
    action: EditAction,
) -> Res<()> {
    let e = ctx.engine()?;
    e.resolve_edit(&edit, action)
        .await
        .map_err(guard::logged("resolve edit"))?;
    let remaining = {
        let mut last = lock(&ctx.last_edits);
        last.retain(|x| *x != edit);
        last.clone()
    };
    let _ = app.emit("local-edits", &remaining);
    Ok(())
}

/// What `group` gives this device: plugins, skills, agents, commands, output styles,
/// settings keys and its CLAUDE.md. Local files only.
#[tauri::command]
pub fn group_contents(ctx: CtxState, group: GroupId) -> Res<GroupContents> {
    contents::group_contents(&ctx.paths, &group).map_err(guard::logged("group contents"))
}

/// The user's own skill folders in `~/.claude/skills`, for the "Share one of your skills" picker.
/// Skills a group put there are left out: they are shared already. So are folders without a
/// `SKILL.md` of their own, such as `synced`, where Claude Code keeps the skills of the
/// claude.ai account in one subfolder per organization.
#[tauri::command]
pub fn local_skills(ctx: CtxState) -> Res<Vec<String>> {
    let manifest = Manifest::load(&ctx.paths).map_err(guard::logged("read the manifest"))?;
    let rd = match std::fs::read_dir(ctx.paths.claude_home.join("skills")) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("could not list the skills: {e}")),
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.path().join("SKILL.md").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && !group_managed(&manifest, n))
        .collect();
    names.sort();
    Ok(names)
}

fn group_managed(manifest: &Manifest, skill: &str) -> bool {
    let prefix = format!("skills/{skill}/");
    manifest
        .files
        .range(prefix.clone()..)
        .next()
        .is_some_and(|(path, _)| path.starts_with(&prefix))
}

#[tauri::command]
pub async fn share_skill(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
    skill: String,
) -> Res<()> {
    if skill.is_empty() || skill.contains(['/', '\\']) || skill.starts_with('.') {
        return Err("not a skill folder name".into());
    }
    let e = ctx.engine()?;
    e.share_skill(&group, &skill)
        .await
        .map_err(guard::logged("share skill"))?;
    background::sync_soon(&app, true);
    Ok(())
}

/// The user's own CLAUDE.md text as a group would get it (no managed block or group imports,
/// secrets redacted), for the "Share my CLAUDE.md" preview. Empty when there is none.
#[tauri::command]
pub fn claude_md_preview(ctx: CtxState) -> Res<String> {
    let e = ctx.engine()?;
    e.claude_md_preview()
        .map_err(guard::logged("CLAUDE.md preview"))
}

#[tauri::command]
pub async fn share_claude_md(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
    share: ClaudeMdShare,
) -> Res<ClaudeMdShared> {
    let e = ctx.engine()?;
    let done = e
        .share_claude_md(&group, &share)
        .await
        .map_err(guard::logged("share CLAUDE.md"))?;
    background::sync_soon(&app, true);
    Ok(done)
}

#[tauri::command]
pub async fn recent_commits(
    ctx: CtxState<'_>,
    group: GroupId,
    limit: Option<usize>,
) -> Res<Vec<CommitInfo>> {
    let e = ctx.engine()?;
    e.recent_commits(&group, limit.unwrap_or(20).min(100))
        .await
        .map_err(guard::logged("recent changes"))
}

/// Changes others made to the group's CLAUDE.md that wait for this user (§7a).
#[tauri::command]
pub fn claude_md_review(ctx: CtxState, group: GroupId) -> Res<ClaudeMdReview> {
    let e = ctx.engine()?;
    e.claude_md_review(&group)
        .map_err(guard::logged("CLAUDE.md review"))
}

/// Accept or decline pending CLAUDE.md changes (`ids`, or all of them).
#[tauri::command]
pub async fn decide_claude_md(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
    ids: Option<Vec<String>>,
    accept: bool,
) -> Res<ClaudeMdReview> {
    let e = ctx.engine()?;
    let r = e
        .decide_claude_md(&group, ids.as_deref(), accept)
        .await
        .map_err(guard::logged("CLAUDE.md review"))?;
    background::refresh_status(&app);
    Ok(r)
}

/// This device's choice on "Review changes others make to CLAUDE.md" (`None`: follow the group).
#[tauri::command]
pub async fn set_claude_md_review(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
    on: Option<bool>,
) -> Res<ClaudeMdReview> {
    let e = ctx.engine()?;
    let r = e
        .set_claude_md_review(&group, on)
        .await
        .map_err(guard::logged("CLAUDE.md review"))?;
    background::refresh_status(&app);
    Ok(r)
}

/// The group setting "Auto-accept CLAUDE.md changes" (needs push access).
#[tauri::command]
pub async fn set_group_auto_accept_claude_md(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
    on: bool,
) -> Res<ClaudeMdReview> {
    let e = ctx.engine()?;
    let r = e
        .set_group_auto_accept_claude_md(&group, on)
        .await
        .map_err(guard::logged("CLAUDE.md review"))?;
    background::refresh_status(&app);
    Ok(r)
}

/// Make the copy the group's CLAUDE.md again, forgetting declined changes.
#[tauri::command]
pub async fn use_group_claude_md(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
) -> Res<ClaudeMdReview> {
    let e = ctx.engine()?;
    let r = e
        .use_group_claude_md(&group)
        .await
        .map_err(guard::logged("CLAUDE.md review"))?;
    background::refresh_status(&app);
    background::check_local_edits(&app);
    Ok(r)
}

/// Duplicate and contradicting rules across the user's groups and own CLAUDE.md, and
/// settings two groups set differently (§7b).
#[tauri::command]
pub fn conflicts(ctx: CtxState) -> Res<Vec<Conflict>> {
    let e = ctx.engine()?;
    e.conflicts().map_err(guard::logged("conflicts"))
}

/// How `text` would clash once shared with or copied to `group`; `from` is the group it is
/// copied from, none for the user's own text.
#[tauri::command]
pub fn claude_md_check(
    ctx: CtxState,
    group: GroupId,
    text: String,
    from: Option<GroupId>,
) -> Res<Vec<Conflict>> {
    let e = ctx.engine()?;
    e.claude_md_check(&group, &text, from.as_ref())
        .map_err(guard::logged("CLAUDE.md check"))
}

/// Keep the other side of a clash: take `line` out of what Claude reads from `source` on
/// this device only.
#[tauri::command]
pub async fn drop_claude_md_line(
    app: AppHandle,
    ctx: CtxState<'_>,
    source: Source,
    line: String,
) -> Res<()> {
    let e = ctx.engine()?;
    e.drop_claude_md_line(&source, &line)
        .await
        .map_err(guard::logged("conflicts"))?;
    background::refresh_status(&app);
    Ok(())
}

/// Replace `line` of `source` with `text`. A group's line is committed for everyone.
#[tauri::command]
pub async fn edit_claude_md_line(
    app: AppHandle,
    ctx: CtxState<'_>,
    source: Source,
    line: String,
    text: String,
) -> Res<()> {
    let e = ctx.engine()?;
    e.edit_claude_md_line(&source, &line, &text)
        .await
        .map_err(guard::logged("conflicts"))?;
    background::sync_soon(&app, true);
    Ok(())
}

/// Settle a setting two groups set differently: `keep`'s value applies on this device.
#[tauri::command]
pub async fn keep_setting(
    app: AppHandle,
    ctx: CtxState<'_>,
    keep: GroupId,
    drop: GroupId,
    pointer: String,
) -> Res<()> {
    let e = ctx.engine()?;
    e.keep_setting(&keep, &drop, &pointer)
        .await
        .map_err(guard::logged("conflicts"))?;
    background::refresh_status(&app);
    Ok(())
}

/// What copying from `from` to `to` would bring (§7b).
#[tauri::command]
pub async fn copy_preview(ctx: CtxState<'_>, from: GroupId, to: GroupId) -> Res<CopyPreview> {
    let e = ctx.engine()?;
    e.copy_preview(&from, &to)
        .await
        .map_err(guard::logged("copy to another group"))
}

#[tauri::command]
pub async fn copy_to_group(
    app: AppHandle,
    ctx: CtxState<'_>,
    from: GroupId,
    to: GroupId,
    request: CopyRequest,
) -> Res<Copied> {
    let e = ctx.engine()?;
    let done = e
        .copy_to_group(&from, &to, &request)
        .await
        .map_err(guard::logged("copy to another group"))?;
    background::sync_soon(&app, true);
    Ok(done)
}

#[tauri::command]
pub async fn sync_now(app: AppHandle) -> Res<SyncReport> {
    let r = background::run_sync(&app, true).await;
    crate::personal::after_sync(&app, true).await;
    background::check_local_edits(&app);
    r
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    device_name: String,
    poll_seconds: u64,
    autostart: bool,
    paused: bool,
    desktop_notifications: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    device_name: Option<String>,
    poll_seconds: Option<u64>,
    autostart: Option<bool>,
    paused: Option<bool>,
    desktop_notifications: Option<bool>,
}

fn settings_of(app: &AppHandle, ctx: &Ctx) -> Settings {
    Settings {
        device_name: ctx.device_name(),
        poll_seconds: ctx.poll_seconds(),
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        paused: ctx.paused(),
        desktop_notifications: ctx.prefs().desktop_notifications,
    }
}

#[tauri::command]
pub fn get_settings(app: AppHandle, ctx: CtxState) -> Settings {
    settings_of(&app, &ctx)
}

#[tauri::command]
pub fn set_settings(app: AppHandle, ctx: CtxState, settings: SettingsPatch) -> Res<Settings> {
    if let Some(on) = settings.autostart {
        let r = if on {
            app.autolaunch().enable()
        } else {
            app.autolaunch().disable()
        };
        r.map_err(|e| format!("could not change start at login: {e}"))?;
    }
    let device = settings.device_name.map(|d| d.trim().to_string());
    if let Some(d) = &device {
        if d.len() > 64 || d.contains(['/', '\\', ':', '@']) {
            return Err("device names can't contain / \\ : @ and are at most 64 characters".into());
        }
    }
    let poll = settings
        .poll_seconds
        .map(|p| p.clamp(config::MIN_POLL_SECONDS, 3600));
    let renamed = device.is_some();
    let saved = ctx.update_state(|s| {
        if let Some(d) = device {
            s.device_name = (!d.is_empty()).then_some(d);
        }
        if let Some(p) = poll {
            s.poll_seconds = Some(p);
        }
        if let Some(p) = settings.paused {
            s.paused = p;
        }
    });
    if renamed {
        ctx.reset_engine();
    }
    background::refresh_status(&app);
    ctx.wake.notify_one();
    saved?;
    if let Some(on) = settings.desktop_notifications {
        let mut prefs = ctx.prefs();
        prefs.desktop_notifications = on;
        ctx.save_prefs(&prefs)
            .map_err(|e| format!("could not save the setting: {e}"))?;
    }
    Ok(settings_of(&app, &ctx))
}

/// The notification history, newest first.
#[tauri::command]
pub fn notifications(ctx: CtxState) -> Vec<Notification> {
    notifications::list(&ctx.paths)
}

#[tauri::command]
pub fn mark_notification_read(app: AppHandle, ctx: CtxState, id: u64) -> Res<()> {
    notifications::mark_read(&ctx.paths, id).map_err(guard::logged("mark notification read"))?;
    notify::changed(&app);
    Ok(())
}

#[tauri::command]
pub fn mark_all_notifications_read(app: AppHandle, ctx: CtxState) -> Res<()> {
    notifications::mark_all_read(&ctx.paths).map_err(guard::logged("mark notifications read"))?;
    notify::changed(&app);
    Ok(())
}

#[tauri::command]
pub fn clear_notifications(app: AppHandle, ctx: CtxState) -> Res<()> {
    notifications::clear(&ctx.paths).map_err(guard::logged("clear notifications"))?;
    notify::changed(&app);
    Ok(())
}

/// Settings' "Send a test notification".
#[tauri::command]
pub fn test_notification(app: AppHandle) {
    notify::show(
        &app,
        notify::Kind::Other,
        None,
        "Test notification",
        "Notifications from claude-sync show up like this.",
    );
}

#[tauri::command]
pub fn feedback_logs(ctx: CtxState) -> Res<String> {
    feedback::logs(&ctx)
}

#[tauri::command]
pub async fn feedback_capture_screen(app: AppHandle) -> Res<Screenshot> {
    feedback::capture_screen(&app).await
}

#[tauri::command]
pub async fn feedback_attach_file(data: String) -> Res<Screenshot> {
    tauri::async_runtime::spawn_blocking(move || feedback::attach_file(&data))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn feedback_paste_image() -> Res<Screenshot> {
    tauri::async_runtime::spawn_blocking(feedback::paste_clipboard)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn feedback_submit(app: AppHandle, report: FeedbackInput) -> Res<FeedbackResult> {
    feedback::submit(&app, report).await
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Res<UpdateInfo> {
    updates::check_and_download(&app).await
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> UpdateInfo {
    updates::current(&app)
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Res<()> {
    updates::install_now(&app).await
}

/// Open GitHub pages (device login, issues, repos) in the browser. Nothing else.
#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> Res<()> {
    if !url.starts_with("https://github.com/") {
        return Err("only github.com links can be opened".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Open a file the app saved (a conflict copy) or show it in its folder.
#[tauri::command]
pub fn open_path(app: AppHandle, ctx: CtxState, path: String, reveal: Option<bool>) -> Res<()> {
    let p = PathBuf::from(&path);
    let p = std::fs::canonicalize(&p).map_err(|_| format!("{path} no longer exists"))?;
    let root = std::fs::canonicalize(&ctx.paths.state_dir).map_err(|e| e.to_string())?;
    if !p.starts_with(&root) {
        return Err("only files saved by claude-sync can be opened".into());
    }
    if reveal.unwrap_or(false) {
        app.opener()
            .reveal_item_in_dir(&p)
            .map_err(|e| e.to_string())
    } else {
        app.opener()
            .open_path(p.to_string_lossy(), None::<&str>)
            .map_err(|e| e.to_string())
    }
}

/// The scheduled jobs of `group` (§17): pending ones, accepted ones, what was skipped.
#[tauri::command]
pub async fn jobs(ctx: CtxState<'_>, group: GroupId) -> Res<JobsOverview> {
    app_jobs::list(ctx.inner().clone(), group).await
}

/// Accept the version of a job the screen showed (`hash`), turn it on and register it.
#[tauri::command]
pub async fn accept_job(
    app: AppHandle,
    group: GroupId,
    id: String,
    hash: String,
) -> Res<JobsOverview> {
    app_jobs::accept(app, group, id, hash).await
}

#[tauri::command]
pub async fn set_job_enabled(
    app: AppHandle,
    group: GroupId,
    id: String,
    enabled: bool,
) -> Res<JobsOverview> {
    app_jobs::set_enabled(app, group, id, enabled).await
}

/// "Run this group's jobs without asking": on accepts its new and changed jobs from now on
/// (each one notified); off asks again for the next change.
#[tauri::command]
pub async fn set_jobs_auto_accept(app: AppHandle, group: GroupId, on: bool) -> Res<JobsOverview> {
    app_jobs::set_auto_accept(app, group, on).await
}

#[tauri::command]
pub async fn run_job_now(ctx: CtxState<'_>, group: GroupId, id: String) -> Res<JobsOverview> {
    app_jobs::run_now(ctx.inner().clone(), group, id).await
}

/// Open a job's last log (`kind` = "log") or its report ("report").
#[tauri::command]
pub fn open_job_file(
    app: AppHandle,
    ctx: CtxState,
    group: GroupId,
    id: String,
    kind: String,
) -> Res<()> {
    app_jobs::open_file(&app, &ctx, &group, &id, &kind)
}

/// Plugins of `group` that could not be installed or updated (§18).
#[tauri::command]
pub fn plugin_warnings(ctx: CtxState, group: GroupId) -> Res<Vec<String>> {
    app_jobs::plugin_warnings(&ctx, &group)
}

/// Retry: run the plugin installs of `group` again; returns the warnings left.
#[tauri::command]
pub async fn retry_plugins(app: AppHandle, group: GroupId) -> Res<Vec<String>> {
    app_jobs::retry_plugins(app, group).await
}

#[cfg(test)]
mod tests {
    use super::{group_managed, valid_login};
    use claude_sync_core::apply::{ManagedFile, Manifest};
    use claude_sync_core::GroupId;

    #[test]
    fn logins() {
        assert_eq!(valid_login(" @Mehdi-Mamas ").unwrap(), "Mehdi-Mamas");
        assert!(valid_login("a/b").is_err());
        assert!(valid_login("-x").is_err());
        assert!(valid_login("").is_err());
        assert!(valid_login(&"a".repeat(40)).is_err());
    }

    #[test]
    fn group_skills_are_not_offered_for_sharing() {
        let mut m = Manifest::default();
        let owner = ManagedFile {
            group: GroupId {
                owner: "o".into(),
                repo: "r".into(),
            },
            sha256: String::new(),
        };
        m.files.insert("skills/team/SKILL.md".into(), owner.clone());
        m.files.insert("skills/team-b/x.md".into(), owner);
        assert!(group_managed(&m, "team"));
        assert!(group_managed(&m, "team-b"));
        assert!(!group_managed(&m, "tea"));
        assert!(!group_managed(&m, "mine"));
    }
}

/// The files reverting a commit would change (host only; writes nothing).
#[tauri::command]
pub async fn revert_preview(ctx: CtxState<'_>, group: GroupId, sha: String) -> Res<RevertPlan> {
    let e = ctx.engine()?;
    e.revert_preview(&group, &sha)
        .await
        .map_err(guard::logged("revert preview"))
}

/// Revert a commit of the group with a new commit (host only), then sync.
#[tauri::command]
pub async fn revert_commit(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
    sha: String,
) -> Res<RevertPlan> {
    let e = ctx.engine()?;
    let done = e
        .revert_commit(&group, &sha)
        .await
        .map_err(guard::logged("revert commit"))?;
    background::sync_soon(&app, true);
    Ok(done)
}
