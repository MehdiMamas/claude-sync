//! Group scheduled jobs (§17) and group plugin installs (§18) in the app: the headless
//! `run-job` / `remove-jobs` subcommands the OS scheduler and the uninstaller start, the
//! reconcile after every sync, join and leave, and the commands of the group screen.
use crate::ctx::{Ctx, Prefs};
use crate::guard::{self, Error};
use crate::notify::{self, Kind};
use claude_sync_core::jobs::{
    self, os, run, AutoAccepted, Jobs, JobsOverview, JobsState, ReconcileReport,
};
use claude_sync_core::plugin_install::{self, PluginCli, RealCli};
use claude_sync_core::state::AppState;
use claude_sync_core::{claude_cli, notifications, GroupId, Paths};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// The uninstaller runs `claude-sync remove-jobs` (NSIS pre-uninstall hook).
pub const REMOVE_JOBS_ARG: &str = "remove-jobs";
/// `run-job <owner/repo> <id> --now`: started by "Run now" (runs a job that is turned off).
pub const NOW_ARG: &str = "--now";

/// One change to `jobs.json` and the OS tasks at a time (sync, join, the group screen).
static JOBS_LOCK: Mutex<()> = Mutex::new(());

fn home() -> Result<PathBuf, Error> {
    std::env::home_dir().ok_or_else(|| "cannot find the home directory".to_string())
}

/// Run `f` with the jobs of this device. Debug builds use their own task folder
/// (`\claude-sync-dev\`), so a dev run never touches the installed app's tasks.
fn with_jobs<T>(
    paths: &Paths,
    what: &str,
    f: impl FnOnce(&Jobs) -> claude_sync_core::Result<T>,
) -> Result<T, Error> {
    let home = home()?;
    let scheduler = os::system(&home, cfg!(debug_assertions));
    let program = std::env::current_exe().map_err(|e| format!("cannot find the app: {e}"))?;
    let jobs = Jobs {
        paths,
        home,
        scheduler: &*scheduler,
        program,
        resolve_claude: claude_cli::resolve,
    };
    let _one = JOBS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    guard::call(what, || f(&jobs))
}

// ---- headless subcommands ----

/// `claude-sync run-job <owner/repo> <id> [--now]`: run one job and exit (no tray, no
/// single-instance check). Returns the process exit code.
pub fn run_job_main(paths: &Paths, args: &[String]) -> i32 {
    let group = args.first().and_then(|g| GroupId::parse(g));
    let (Some(group), Some(id)) = (group, args.get(1)) else {
        eprintln!("usage: claude-sync run-job <owner/repo> <job id> [--now]");
        return 2;
    };
    let manual = args.iter().any(|a| a == NOW_ARG);
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("run-job: no runtime: {e}");
            return 1;
        }
    };
    let online = || rt.block_on(run::is_online());
    tracing::info!(group = %group, job = %id, manual, "job starting");
    // Not under JOBS_LOCK: the run takes as long as the job; the runner has its own lock.
    let home = match home() {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("run-job: {e}");
            return 1;
        }
    };
    let scheduler = os::system(&home, cfg!(debug_assertions));
    let jobs = Jobs {
        paths,
        home,
        scheduler: &*scheduler,
        program: std::env::current_exe().unwrap_or_default(),
        resolve_claude: claude_cli::resolve,
    };
    let title = jobs
        .find(&group, id)
        .map(|j| j.title)
        .unwrap_or_else(|_| id.clone());
    match guard::call("run job", || {
        jobs.run(&group, id, manual, &online, &run::RealExec)
    }) {
        Ok(Some(last)) => {
            tracing::info!(job = %id, status = %last.status, code = ?last.exit_code, "job finished");
            if let Some(report) = &last.report {
                notify_report(paths, &group, &title, &last.status, report);
            }
            if last.status == "ok" {
                0
            } else {
                1
            }
        }
        Ok(None) => 0,
        Err(_) => 1,
    }
}

/// `claude-sync remove-jobs`: remove every task this device registered (uninstall).
pub fn remove_jobs_main(paths: &Paths) -> i32 {
    match with_jobs(paths, "remove jobs", |j| j.remove_all()) {
        Ok(r) => {
            tracing::info!(removed = ?r.unregistered, errors = ?r.errors, "jobs removed");
            i32::from(!r.errors.is_empty())
        }
        Err(_) => 1,
    }
}

/// "<title> finished: its report is ready." This process has no window, so it records
/// the notification in the history itself (the app picks it up on its next background pass, see
/// `background::tick`) and shows the desktop notification unless it is turned off in
/// Settings. Clicking it opens the report (Windows: while this process waits, up to 10
/// minutes); otherwise the text names the file.
fn notify_report(
    paths: &Paths,
    group: &GroupId,
    title: &str,
    status: &str,
    report: &std::path::Path,
) {
    let heading = format!("{title}: report ready");
    let ended = if status == "ok" {
        String::new()
    } else {
        format!("The run ended with \"{status}\". ")
    };
    let saved = format!("{ended}Report: {}", report.display());
    if let Err(e) = notifications::record(paths, Kind::Job, Some(group.clone()), &heading, &saved) {
        tracing::warn!("notification not saved to the history: {e}");
    }
    if !Prefs::load(paths).desktop_notifications {
        return;
    }
    let body = format!("{ended}Click to open {}", report.display());
    #[cfg(windows)]
    {
        use tauri_winrt_notification::Toast;
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let tx2 = tx.clone();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_default();
        // Same rule as tauri-plugin-notification: an unbundled build has no app id.
        let dev = exe_dir.ends_with("target/debug")
            || exe_dir.ends_with("target/release")
            || exe_dir.ends_with(r"target\debug")
            || exe_dir.ends_with(r"target\release");
        let app_id = if dev {
            Toast::POWERSHELL_APP_ID
        } else {
            crate::config::IDENTIFIER
        };
        let shown = Toast::new(app_id)
            .title(&heading)
            .text1(&body)
            .on_activated(move |_| {
                let _ = tx.send(true);
                Ok(())
            })
            .on_dismissed(move |_| {
                let _ = tx2.send(false);
                Ok(())
            })
            .show();
        match shown {
            Ok(()) => {
                if rx.recv_timeout(Duration::from_secs(10 * 60)) == Ok(true) {
                    if let Err(e) = tauri_plugin_opener::open_path(report, None::<&str>) {
                        tracing::warn!("could not open the report: {e}");
                    }
                }
            }
            Err(e) => tracing::warn!("job notification failed: {e}"),
        }
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "macos")]
        let _ = notify_rust::set_application(crate::config::IDENTIFIER);
        if let Err(e) = notify_rust::Notification::new()
            .summary(&heading)
            .body(&body)
            .show()
        {
            tracing::warn!("job notification failed: {e}");
        }
    }
}

// ---- reconcile and plugin installs, in the background ----

fn ctx(app: &AppHandle) -> Arc<Ctx> {
    app.state::<Arc<Ctx>>().inner().clone()
}

/// Make the OS tasks match the groups (after a sync, join or leave). `verify` also checks
/// that registered tasks still exist (at startup).
pub fn reconcile_soon(app: &AppHandle, verify: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Ok(r) = with_jobs(&ctx(&app).paths, "reconcile jobs", |j| j.reconcile(verify)) {
            handled(&app, r);
        }
    });
}

fn real_cli(paths: &Paths) -> Option<Box<dyn PluginCli>> {
    let home = std::env::home_dir()?;
    let claude = claude_cli::resolve(&home)?;
    Some(Box::new(RealCli::new(claude, paths, &home)))
}

/// Install or update the plugins `groups` turn on (§18), in the background. `force`: Retry.
pub fn ensure_plugins_soon(app: &AppHandle, groups: Vec<GroupId>, force: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        ensure_plugins(&app, &groups, force);
    });
}

fn ensure_plugins(app: &AppHandle, groups: &[GroupId], force: bool) -> Vec<String> {
    let ctx = ctx(app);
    let mut warnings = Vec::new();
    let mut changed = Vec::new();
    for g in groups {
        let cli = || real_cli(&ctx.paths);
        if let Ok(o) = guard::call("install group plugins", || {
            plugin_install::ensure(&ctx.paths, g, &cli, force)
        }) {
            if !o.installed.is_empty() || !o.updated.is_empty() {
                changed.push(g);
            }
            warnings.extend(o.warnings);
        }
    }
    if !changed.is_empty() {
        // Link the group when the plugins of only one changed.
        let group = (changed.len() == 1).then(|| changed[0]);
        crate::background::plugins_changed(app, group);
    }
    warnings
}

/// After a sync: tasks and plugins of every joined group.
pub fn after_sync(app: &AppHandle) {
    let groups = match AppState::load(&ctx(app).paths) {
        Ok(s) => s.groups,
        Err(e) => {
            tracing::warn!("after sync: could not read the state: {e}");
            return;
        }
    };
    reconcile_soon(app, false);
    ensure_plugins_soon(app, groups, false);
}

// ---- commands ----

fn overview(paths: &Paths, group: &GroupId) -> Result<JobsOverview, Error> {
    with_jobs(paths, "jobs", |j| j.overview(group))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// Registration errors are shown on the job itself; this logs them. Jobs accepted without
/// asking (the group's "without asking" setting) get a notification each.
fn handled(app: &AppHandle, r: ReconcileReport) {
    for a in &r.auto_accepted {
        notify_auto(app, a);
    }
    for e in r.errors {
        tracing::warn!("job registration: {e}");
    }
}

fn notify_auto(app: &AppHandle, a: &AutoAccepted) {
    let (title, what) = if a.changed {
        (format!("Job changed in {}", a.group), "This new version")
    } else {
        (format!("New job from {}", a.group), "It")
    };
    let body = format!(
        "{}: {what} will run without asking, because this group's jobs are allowed. See the group's Scheduled jobs.",
        a.title
    );
    notify::show(app, Kind::Job, Some(&a.group), &title, &body);
}

pub async fn list(ctx: Arc<Ctx>, group: GroupId) -> Result<JobsOverview, Error> {
    blocking(move || overview(&ctx.paths, &group)).await
}

pub async fn accept(
    app: AppHandle,
    group: GroupId,
    id: String,
    hash: String,
) -> Result<JobsOverview, Error> {
    blocking(move || {
        let ctx = ctx(&app);
        let r = with_jobs(&ctx.paths, "accept job", |j| j.accept(&group, &id, &hash))?;
        handled(&app, r);
        overview(&ctx.paths, &group)
    })
    .await
}

pub async fn set_enabled(
    app: AppHandle,
    group: GroupId,
    id: String,
    enabled: bool,
) -> Result<JobsOverview, Error> {
    blocking(move || {
        let ctx = ctx(&app);
        let r = with_jobs(&ctx.paths, "turn job on or off", |j| {
            j.set_enabled(&group, &id, enabled)
        })?;
        handled(&app, r);
        overview(&ctx.paths, &group)
    })
    .await
}

/// "Run this group's jobs without asking" on or off.
pub async fn set_auto_accept(
    app: AppHandle,
    group: GroupId,
    on: bool,
) -> Result<JobsOverview, Error> {
    blocking(move || {
        let ctx = ctx(&app);
        let r = with_jobs(&ctx.paths, "allow jobs without asking", |j| {
            j.set_auto_accept(&group, on)
        })?;
        handled(&app, r);
        overview(&ctx.paths, &group)
    })
    .await
}

/// "Run now": start the runner as its own process, so the run outlives the window.
pub async fn run_now(ctx: Arc<Ctx>, group: GroupId, id: String) -> Result<JobsOverview, Error> {
    blocking(move || {
        let job = with_jobs(&ctx.paths, "run job", |j| j.find(&group, &id))?;
        let state = JobsState::load(&ctx.paths).map_err(|e| e.to_string())?;
        if !state.accepted(&job) {
            return Err("accept the job first".into());
        }
        if run::is_running(&ctx.paths, &job) {
            return Err("the job is running already".into());
        }
        let exe = std::env::current_exe().map_err(|e| format!("cannot find the app: {e}"))?;
        let mut cmd = std::process::Command::new(exe);
        cmd.args([jobs::RUN_JOB_ARG, &group.to_string(), &id, NOW_ARG])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            cmd.creation_flags(claude_cli::CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start the job: {e}"))?;
        // Reap it when it ends (no zombie on Unix).
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        // Give the runner a moment to take its lock, so the screen shows it running.
        std::thread::sleep(Duration::from_millis(500));
        overview(&ctx.paths, &group)
    })
    .await
}

/// Open the job's last log or its report. Paths come from the job, never from the UI.
pub fn open_file(
    app: &AppHandle,
    ctx: &Ctx,
    group: &GroupId,
    id: &str,
    kind: &str,
) -> Result<(), Error> {
    let job = with_jobs(&ctx.paths, "open job file", |j| j.find(group, id))?;
    let path = match kind {
        "log" => run::LastRun::load(&ctx.paths, &job.id)
            .and_then(|r| r.log)
            .ok_or_else(|| "the job has no log yet".to_string())?,
        "report" => job
            .report_path
            .clone()
            .ok_or_else(|| "the job names no report".to_string())?,
        _ => return Err("unknown file".into()),
    };
    if !path.is_file() {
        return Err(format!("{} does not exist", path.display()));
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

pub fn plugin_warnings(ctx: &Ctx, group: &GroupId) -> Result<Vec<String>, Error> {
    plugin_install::warnings(&ctx.paths, group).map_err(guard::logged("plugin warnings"))
}

pub async fn retry_plugins(app: AppHandle, group: GroupId) -> Result<Vec<String>, Error> {
    blocking(move || Ok(ensure_plugins(&app, &[group], true))).await
}

/// A left group: its tasks go (reconcile), its plugin record too (the plugins stay).
pub fn after_leave(app: &AppHandle, group: &GroupId) {
    let ctx = ctx(app);
    if let Err(e) = plugin_install::forget(&ctx.paths, group) {
        tracing::warn!("forget group plugins: {e}");
    }
    reconcile_soon(app, false);
}
