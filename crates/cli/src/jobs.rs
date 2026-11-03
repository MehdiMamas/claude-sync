//! Group jobs (§17) and plugin installs (§18) on this machine: the OS tasks run
//! `claude-sync-cli run-job <group> <id>`, and a sync is followed by a reconcile.
use claude_sync_core::jobs::{os, run, Jobs};
use claude_sync_core::plugin_install::{self, PluginCli, RealCli};
use claude_sync_core::state::AppState;
use claude_sync_core::{claude_cli, GroupId, Paths};
use std::sync::Mutex;

/// One change to `jobs.json` and the OS tasks at a time.
static JOBS_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with the jobs of this device.
pub fn with_jobs<T>(
    paths: &Paths,
    f: impl FnOnce(&Jobs) -> claude_sync_core::Result<T>,
) -> Result<T, String> {
    let home = std::env::home_dir().ok_or("cannot find the home directory")?;
    let scheduler = os::system(&home, cfg!(debug_assertions));
    let program = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let jobs = Jobs {
        paths,
        home,
        scheduler: &*scheduler,
        program,
        resolve_claude: claude_cli::resolve,
    };
    let _one = JOBS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    f(&jobs).map_err(|e| e.to_string())
}

/// What follows a sync: tasks and plugins of the joined groups. Blocking.
pub trait Hooks: Send + Sync {
    fn after_sync(&self, paths: &Paths);
}

/// Registers the OS tasks and installs group plugins.
pub struct RealHooks;

impl Hooks for RealHooks {
    fn after_sync(&self, paths: &Paths) {
        let groups = match AppState::load(paths) {
            Ok(s) => s.groups,
            Err(e) => {
                tracing::warn!("after sync: could not read the state: {e}");
                return;
            }
        };
        match with_jobs(paths, |j| j.reconcile(false)) {
            Ok(r) => {
                for a in &r.auto_accepted {
                    tracing::info!(group = %a.group, job = %a.title, "job accepted without asking (group setting)");
                }
                for e in r.errors {
                    tracing::warn!("job registration: {e}");
                }
            }
            Err(e) => tracing::warn!("reconcile jobs: {e}"),
        }
        ensure_plugins(paths, &groups);
    }
}

fn real_cli(paths: &Paths) -> Option<Box<dyn PluginCli>> {
    let home = std::env::home_dir()?;
    let claude = claude_cli::resolve(&home)?;
    Some(Box::new(RealCli::new(claude, paths, &home)))
}

fn ensure_plugins(paths: &Paths, groups: &[GroupId]) {
    for g in groups {
        let cli = || real_cli(paths);
        match plugin_install::ensure(paths, g, &cli, false) {
            Ok(o) => {
                for w in o.warnings {
                    tracing::warn!(group = %g, "plugins: {w}");
                }
            }
            Err(e) => tracing::warn!(group = %g, "install group plugins: {e}"),
        }
    }
}

/// `claude-sync-cli run-job <owner/repo> <id> [--now]`: run one job and exit. Returns the
/// exit code.
pub fn run_job_main(paths: &Paths, args: &[String]) -> i32 {
    let group = args.first().and_then(|g| GroupId::parse(g));
    let (Some(group), Some(id)) = (group, args.get(1)) else {
        eprintln!("usage: claude-sync-cli run-job <owner/repo> <job id> [--now]");
        return 2;
    };
    let manual = args.iter().any(|a| a == "--now");
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
    let Some(home) = std::env::home_dir() else {
        tracing::error!("run-job: cannot find the home directory");
        return 1;
    };
    let scheduler = os::system(&home, cfg!(debug_assertions));
    let jobs = Jobs {
        paths,
        home,
        scheduler: &*scheduler,
        program: std::env::current_exe().unwrap_or_default(),
        resolve_claude: claude_cli::resolve,
    };
    match jobs.run(&group, id, manual, &online, &run::RealExec) {
        Ok(Some(last)) => {
            tracing::info!(job = %id, status = %last.status, code = ?last.exit_code, "job finished");
            if let Some(report) = &last.report {
                // No desktop here: the history keeps the note and the app shows it.
                let note = format!("Report: {}", report.display());
                if let Err(e) = claude_sync_core::notifications::record(
                    paths,
                    claude_sync_core::notifications::Kind::Job,
                    Some(group.clone()),
                    &format!("{id}: report ready"),
                    &note,
                ) {
                    tracing::warn!("notification not saved to the history: {e}");
                }
                println!("{note}");
            }
            i32::from(last.status != "ok")
        }
        Ok(None) => 0,
        Err(e) => {
            tracing::error!("run-job: {e}");
            eprintln!("claude-sync-cli: {e}");
            1
        }
    }
}
