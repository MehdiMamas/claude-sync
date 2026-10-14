//! Tauri commands: the UI's whole API. Errors are user-facing strings.
use crate::auth::{self, DeviceCodeView};
use crate::background;
use crate::config;
use crate::ctx::{lock, Ctx, Status};
use crate::feedback::{self, FeedbackInput, FeedbackResult, Screenshot};
use crate::guard::{self, Error};
use crate::updates::{self, UpdateInfo};
use claude_sync_core::apply::Manifest;
use claude_sync_core::engine::{CommitInfo, GroupStatus, SyncReport};
use claude_sync_core::github::Invitation;
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
    client_id_missing: bool,
}

#[tauri::command]
pub fn auth_status(ctx: CtxState) -> AuthStatus {
    let login = ctx.login();
    AuthStatus {
        signed_in: login.is_some(),
        login,
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
        .create_group(name, &repo)
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
        background::plugins_changed(&app);
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
        background::plugins_changed(&app);
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
        background::plugins_changed(&app);
    }
    Ok(())
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

/// The user's own skill folders in `~/.claude/skills`, for the "Share a skill" picker.
/// Skills a group put there are left out: they are shared already.
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

#[tauri::command]
pub async fn sync_now(app: AppHandle) -> Res<SyncReport> {
    let r = background::run_sync(&app, true).await;
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    device_name: Option<String>,
    poll_seconds: Option<u64>,
    autostart: Option<bool>,
    paused: Option<bool>,
}

fn settings_of(app: &AppHandle, ctx: &Ctx) -> Settings {
    Settings {
        device_name: ctx.device_name(),
        poll_seconds: ctx.poll_seconds(),
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        paused: ctx.paused(),
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
    Ok(settings_of(&app, &ctx))
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
