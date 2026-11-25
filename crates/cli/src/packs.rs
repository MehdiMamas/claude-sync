//! `claude-sync-cli packs ...` (§23): opt-in packs from the command line, for WSL and other
//! machines with no window. The commands are in `claude_sync_core::packs::cli`; this supplies
//! what differs here: the engine that applies a change and the job scheduler.
use crate::auth;
use crate::jobs::with_jobs;
use claude_sync_core::lock::SyncLock;
use claude_sync_core::packs::cli::{self, Host};
use claude_sync_core::packs::ops::Env;
use claude_sync_core::{claude_cli, GroupId, Paths};
use std::path::PathBuf;
use std::time::Duration;

struct CliHost<'a> {
    paths: &'a Paths,
    rt: &'a tokio::runtime::Runtime,
}

impl Host for CliHost<'_> {
    fn claude(&self) -> Option<PathBuf> {
        claude_cli::resolve(&std::env::home_dir()?)
    }

    fn apply(&self, group: &GroupId, jobs: &[(String, String)]) -> claude_sync_core::Result<()> {
        let invalid = |m: String| claude_sync_core::Error::Invalid(m);
        // Not while the daemon is syncing.
        let _lock = SyncLock::acquire(self.paths, Duration::from_secs(60))
            .map_err(|_| invalid("another sync is running; try again in a minute".into()))?;
        let (_, engine) = auth::require(self.paths).map_err(invalid)?;
        self.rt.block_on(engine.reapply_group(group))?;
        with_jobs(self.paths, |j| {
            for (id, hash) in jobs {
                j.accept(group, id, hash)?;
            }
            j.reconcile(false)
        })
        .map(|_| ())
        .map_err(invalid)
    }
}

/// `packs [list | setup <id> | check <id> | on <id> | off <id>] [--group owner/repo] [--json]`.
/// Returns the exit code: 0 done, 1 failed, 2 wrong usage.
pub fn main(paths: &Paths, args: &[String]) -> i32 {
    let Some(home) = std::env::home_dir() else {
        eprintln!("claude-sync-cli: cannot find the home directory");
        return 1;
    };
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("claude-sync-cli: {e}");
            return 1;
        }
    };
    let host = CliHost { paths, rt: &rt };
    let env = Env::real(&home);
    cli::run(
        paths,
        &host,
        &env,
        args,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}
