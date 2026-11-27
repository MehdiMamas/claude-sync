//! `daemon`: sync in a loop. One daemon per machine: it holds `<state>/daemon.lock` (the OS
//! drops the lock when the process dies) and writes `<state>/daemon.owner`, so another
//! program (the app, the importer) can tell a live daemon from a stale file.
//!
//! The token file is read on every pass: a `login` or `logout` in another terminal takes
//! effect at the next pass without restarting the daemon.
use crate::token::FileSecrets;
use claude_sync_actions::handover::{own_owner, write_owner};
use claude_sync_actions::{Core, Env};
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub use claude_sync_actions::handover::{live_owner, owner_path, Owner};

pub fn lock_path(state: &Path) -> PathBuf {
    state.join("daemon.lock")
}

/// Holds the lock and the owner file; both go when it drops.
pub struct Instance {
    _lock: File,
    owner: PathBuf,
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.owner);
        // flock goes with the open file description: a child forked right now (another thread
        // spawning a process) would keep the lock after our fd closes, so release it by hand
        let _ = self._lock.unlock();
    }
}

/// `Ok(None)`: another daemon holds the lock.
pub fn single_instance(state: &Path, version: &str) -> Result<Option<Instance>, String> {
    std::fs::create_dir_all(state)
        .map_err(|e| format!("cannot create {}: {e}", state.display()))?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(state))
        .map_err(|e| format!("cannot open daemon.lock: {e}"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(e)) => return Err(format!("cannot lock daemon.lock: {e}")),
    }
    write_owner(state, &own_owner(version))?;
    let path = owner_path(state);
    Ok(Some(Instance {
        _lock: lock,
        owner: path,
    }))
}

/// The lock for this daemon; a v1 daemon holding it is handed over from first (v2-Cutover).
/// `Ok(None)`: a live v2 daemon has it.
pub fn take(
    state: &Path,
    version: &str,
    procs: &dyn claude_sync_actions::handover::Procs,
) -> Result<Option<Instance>, String> {
    use claude_sync_actions::handover::*;
    if let Some(i) = single_instance(state, version)? {
        return Ok(Some(i));
    }
    let exe = std::env::current_exe().ok();
    match free_daemon_lock(
        state,
        procs,
        (std::process::id(), exe.as_deref()),
        WAIT,
        POLL,
    ) {
        Handover::Free { .. } => single_instance(state, version),
        Handover::V2Holds => Ok(None),
        Handover::Stuck { pid } => Err(crate::run::v1_daemon_message(pid)),
    }
}

/// Sets `stop` on SIGTERM or Ctrl-C. The runtime lives as long as the returned value.
fn stop_on_signal(stop: Arc<AtomicBool>) -> Result<tokio::runtime::Runtime, String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            if let Ok(mut term) = signal(SignalKind::terminate()) {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        stop.store(true, Ordering::SeqCst);
    });
    Ok(rt)
}

/// Run until `stop`. `open` builds the core (called again when the token file changes);
/// `step` is the pause between ticks (the schedule inside the core decides when to sync).
pub fn run_loop(
    state: &Path,
    open: &dyn Fn() -> Result<Core, String>,
    stop: &AtomicBool,
    step: Duration,
    gh: &dyn Fn() -> Option<String>,
) {
    // `gh auth token` at most once a minute while the sign-in is expired (never after logout).
    let mut last_gh: Option<std::time::Instant> = None;
    let token = state.join("token");
    let mut seen: Option<Vec<u8>> = None;
    let mut core: Option<Core> = None;
    while !stop.load(Ordering::SeqCst) {
        let now = std::fs::read(&token).ok();
        if core.is_none() || now != seen {
            match open() {
                Ok(c) => core = Some(c),
                Err(e) => tracing::error!("{e}"),
            }
            seen = now;
        }
        if let Some(c) = core.as_mut() {
            if let Err(e) = c.tick() {
                tracing::error!("sync tick failed: {e}");
            }
            if c.token_refused() && last_gh.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                last_gh = Some(std::time::Instant::now());
                crate::token::recover(c, gh);
            }
            // A refresh (or a recovery) the core wrote itself is not a reason to reopen it.
            seen = std::fs::read(&token).ok();
        }
        let mut waited = Duration::ZERO;
        while waited < step && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100).min(step));
            waited += Duration::from_millis(100);
        }
    }
}

/// The `daemon` command. Returns the line to print.
pub fn run(state: &Path, version: &str, env: Env) -> Result<String, String> {
    let Some(_one) = take(state, version, &*env.procs)? else {
        return Ok("claude-sync-cli daemon is already running.".into());
    };
    let stop = Arc::new(AtomicBool::new(false));
    let _rt = stop_on_signal(stop.clone())?;
    let open = || {
        Core::open_locked(env.clone())
            .map(|mut c| {
                c.start_watch();
                c
            })
            .map_err(|e| e.to_string())
    };
    run_loop(
        state,
        &open,
        &stop,
        Duration::from_secs(1),
        &crate::token::gh_token,
    );
    Ok("claude-sync-cli daemon stopped.".into())
}

/// For tests and the app: an `Env` whose sign-in is the token file under `state`.
pub fn file_secrets(state: &Path) -> claude_sync_actions::Secrets {
    claude_sync_actions::Secrets(Arc::new(FileSecrets(state.join("token"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_daemon_per_state_folder_and_an_owner_file_while_it_lives() {
        let d = tempfile::tempdir().unwrap();
        let state = d.path().join("s");
        let first = single_instance(&state, "9.9.9").unwrap().unwrap();
        // a second one cannot take the lock
        assert!(single_instance(&state, "9.9.9").unwrap().is_none());
        let o = live_owner(&state).unwrap();
        assert_eq!(o.pid, std::process::id());
        assert_eq!(o.version, "9.9.9");
        assert!(o.started > 0);
        drop(first);
        assert!(!owner_path(&state).exists());
        assert!(live_owner(&state).is_none());
        // the lock is free again
        assert!(single_instance(&state, "9.9.9").unwrap().is_some());
    }

    #[test]
    fn an_owner_file_of_a_dead_or_reused_pid_is_not_live() {
        let d = tempfile::tempdir().unwrap();
        let mine = Owner {
            pid: std::process::id(),
            started: 1, // not when this process started
            version: "1".into(),
            exe: "x".into(),
        };
        std::fs::write(owner_path(d.path()), serde_json::to_vec(&mine).unwrap()).unwrap();
        assert!(live_owner(d.path()).is_none());
        std::fs::write(owner_path(d.path()), b"not json").unwrap();
        assert!(live_owner(d.path()).is_none());
    }
}
