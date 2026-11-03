//! The background loop (`daemon`) and the one sync pass it and `sync` share.
use crate::auth::{self, Signed, Standing};
use crate::config;
use crate::jobs::Hooks;
use claude_sync_core::engine::SyncReport;
use claude_sync_core::github::with_caller;
use claude_sync_core::lock::SyncLock;
use claude_sync_core::personal::NewDevice;
use claude_sync_core::state::AppState;
use claude_sync_core::wake::next_wake;
use claude_sync_core::{Engine, Paths};
use std::fs::{File, OpenOptions, TryLockError};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// What one pass did.
#[derive(Default)]
pub struct Outcome {
    /// Why nothing ran (paused, rate limited, another syncer is busy).
    pub skipped: Option<String>,
    pub report: Option<SyncReport>,
    /// The sync itself failed.
    pub error: Option<String>,
    /// Personal memory: a repo from another device waits for `personal enable`.
    pub personal_offer: bool,
}

/// One pass: sync the groups (`full` = everything, else only what changed), then personal
/// memory, then the job and plugin hooks. Takes the shared sync lock, waiting up to
/// `wait` for another process.
pub async fn tick(
    engine: &Engine,
    paths: &Paths,
    full: bool,
    wait: Duration,
    personal_checked: &mut bool,
    hooks: &Arc<dyn Hooks>,
) -> Outcome {
    let mut out = Outcome::default();
    let state = match AppState::load(paths) {
        Ok(s) => s,
        Err(e) => {
            out.error = Some(e.to_string());
            return out;
        }
    };
    if state.paused && !full {
        out.skipped = Some("Syncing is paused".into());
        return out;
    }
    if let Some(reset) = engine.gh.limited_until() {
        out.skipped = Some(format!(
            "GitHub limit reached, resumes at {}",
            claude_sync_core::error::reset_text(reset)
        ));
        return out;
    }
    let _lock = match SyncLock::acquire(paths, wait) {
        Ok(l) => l,
        Err(_) => {
            out.skipped = Some("Another claude-sync is syncing right now".into());
            return out;
        }
    };
    let synced = with_caller("tick", async {
        if full {
            engine.sync_all().await
        } else {
            engine.sync_if_changed().await
        }
    })
    .await;
    match synced {
        Ok(r) => out.report = Some(r),
        Err(e) => out.error = Some(e.to_string()),
    }
    with_caller("personal", async {
        if !*personal_checked {
            match engine.personal_check_new_device().await {
                Ok(found) => {
                    *personal_checked = true;
                    out.personal_offer = found == NewDevice::Offer;
                }
                // Offline or rate limited: asked again after the next pass.
                Err(e) => tracing::info!("personal memory check: {e}"),
            }
        }
        if let Err(e) = engine.personal_sync(!full).await {
            tracing::info!("personal memory sync: {e}");
        }
    })
    .await;
    let (h, p) = (hooks.clone(), paths.clone());
    if let Err(e) = tokio::task::spawn_blocking(move || h.after_sync(&p)).await {
        tracing::error!("after-sync hooks failed: {e}");
    }
    out
}

/// Held for as long as this daemon runs, so a second `daemon` (cron's 5-minute check)
/// exits at once.
pub fn single_instance(paths: &Paths) -> Option<File> {
    std::fs::create_dir_all(&paths.state_dir).ok()?;
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.state_dir.join("daemon.lock"))
        .ok()?;
    match f.try_lock() {
        Ok(()) => Some(f),
        Err(TryLockError::WouldBlock) | Err(TryLockError::Error(_)) => None,
    }
}

fn poll_interval(paths: &Paths) -> Duration {
    let s = AppState::load(paths)
        .ok()
        .and_then(|s| s.poll_seconds)
        .unwrap_or(config::DEFAULT_POLL_SECONDS)
        .max(config::MIN_POLL_SECONDS);
    Duration::from_secs(s)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Sync every poll interval until `shutdown` completes. The token file is read again at
/// every pass, so `login` and `logout` take effect without a restart.
pub async fn run(paths: &Paths, hooks: Arc<dyn Hooks>, shutdown: impl Future<Output = ()>) {
    tokio::pin!(shutdown);
    let mut cached: Option<(Signed, Arc<Engine>)> = None;
    let mut was_limited = false;
    let mut personal_checked = false;
    tracing::info!("daemon started, version {}", config::VERSION);
    loop {
        let wait = match auth::standing(paths) {
            Ok(Standing::In(signed)) => {
                // A token another process refreshed replaces ours (refresh tokens are
                // used once).
                let same = cached.as_ref().is_some_and(|(c, _)| {
                    c.login == signed.login
                        && c.session.tokens().access_token == signed.session.tokens().access_token
                        && !c.session.is_expired()
                });
                if !same {
                    match auth::engine(paths, &signed) {
                        Ok(e) => {
                            cached = Some((signed, Arc::new(e)));
                            personal_checked = false;
                        }
                        Err(e) => tracing::error!("cannot start the engine: {e}"),
                    }
                }
                match &cached {
                    Some((_, engine)) => {
                        let o = tick(
                            engine,
                            paths,
                            was_limited,
                            Duration::ZERO,
                            &mut personal_checked,
                            &hooks,
                        )
                        .await;
                        log_outcome(&o);
                        let limited = engine.gh.limited_until();
                        was_limited = limited.is_some();
                        next_wake(now_unix(), poll_interval(paths), limited)
                    }
                    None => poll_interval(paths),
                }
            }
            Ok(Standing::Expired(l)) => {
                cached = None;
                tracing::warn!("sign-in of {l} expired; run `claude-sync-cli login`");
                poll_interval(paths)
            }
            Ok(Standing::Out) => {
                cached = None;
                tracing::info!("not signed in; waiting for `claude-sync-cli login`");
                poll_interval(paths)
            }
            Err(e) => {
                tracing::error!("{e}");
                poll_interval(paths)
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = &mut shutdown => {
                tracing::info!("daemon stopping");
                return;
            }
        }
    }
}

fn log_outcome(o: &Outcome) {
    if let Some(s) = &o.skipped {
        tracing::info!("sync skipped: {s}");
    }
    if let Some(e) = &o.error {
        tracing::warn!("sync failed: {e}");
    }
    if let Some(r) = &o.report {
        for g in &r.groups {
            if let Some(e) = &g.error {
                tracing::warn!(group = %g.id, "sync error: {e}");
            }
        }
    }
    if o.personal_offer {
        tracing::info!(
            "personal memory from another device is waiting: run `claude-sync-cli personal enable`"
        );
    }
}

/// Resolves on Ctrl-C or, on Unix, SIGTERM (what systemd sends).
pub async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_sync_core::github::GitHub;
    use claude_sync_core::group::{self, GroupFile};
    use claude_sync_core::remote::fake::{FakeGitHub, FakeRepo};
    use claude_sync_core::GroupId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Counting(AtomicUsize);
    impl Hooks for Counting {
        fn after_sync(&self, _: &Paths) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct World {
        _dir: tempfile::TempDir,
        paths: Paths,
        repo: FakeRepo,
        id: GroupId,
        engine: Engine,
    }

    /// `bo` on a temp "machine", joined to ana's group in the fake GitHub.
    async fn world() -> World {
        let gh = FakeGitHub::new();
        let id = GroupId::new("ana", group::DEFAULT_REPO);
        let json = String::from_utf8(GroupFile::new("Friends", "ana").to_bytes()).unwrap();
        let repo = FakeRepo::with_files(
            id.clone(),
            &[
                (group::FILE, &json),
                (group::CLAUDE_MD, "# Friends\n"),
                ("agents/a.md", "base\n"),
            ],
        );
        repo.add_collaborator("bo");
        gh.add(&repo);
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            claude_home: dir.path().join("claude"),
            state_dir: dir.path().join("state"),
        };
        // Never called: every repo comes from the fake world.
        let engine = Engine::with_remotes(
            paths.clone(),
            GitHub::with_api(String::new(), "http://127.0.0.1:9"),
            "bo".into(),
            "pc".into(),
            Arc::new(gh.account("bo")),
        );
        engine.join(&id).await.unwrap();
        World {
            _dir: dir,
            paths,
            repo,
            id,
            engine,
        }
    }

    #[tokio::test]
    async fn one_tick_pulls_the_groups_change_and_runs_the_hooks() {
        let w = world().await;
        let agent = w.paths.claude_home.join("agents/a.md");
        assert_eq!(std::fs::read_to_string(&agent).unwrap(), "base\n");
        w.repo.set_file("agents/a.md", "changed\n");
        let hooks = Arc::new(Counting::default());
        let dynh: Arc<dyn Hooks> = hooks.clone();
        let mut checked = false;
        let o = tick(
            &w.engine,
            &w.paths,
            true,
            Duration::ZERO,
            &mut checked,
            &dynh,
        )
        .await;
        assert!(o.skipped.is_none() && o.error.is_none(), "{:?}", o.error);
        let report = o.report.unwrap();
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].id, w.id);
        assert!(report.groups[0].pulled);
        assert_eq!(std::fs::read_to_string(&agent).unwrap(), "changed\n");
        assert_eq!(hooks.0.load(Ordering::SeqCst), 1);
        assert!(checked, "personal memory was looked at once");
    }

    #[tokio::test]
    async fn a_pass_skips_while_paused_or_while_another_process_syncs() {
        let w = world().await;
        let hooks = Arc::new(Counting::default());
        let dynh: Arc<dyn Hooks> = hooks.clone();
        let mut checked = false;

        // Another process holds the sync lock: nothing is synced, nothing is hooked.
        let held = SyncLock::acquire(&w.paths, Duration::ZERO).unwrap();
        let o = tick(
            &w.engine,
            &w.paths,
            true,
            Duration::ZERO,
            &mut checked,
            &dynh,
        )
        .await;
        assert!(o.skipped.unwrap().contains("Another claude-sync"));
        assert!(o.report.is_none());
        drop(held);

        // Paused: the loop's passes skip, a person's `sync` does not.
        let mut st = AppState::load(&w.paths).unwrap();
        st.paused = true;
        st.save(&w.paths).unwrap();
        let o = tick(
            &w.engine,
            &w.paths,
            false,
            Duration::ZERO,
            &mut checked,
            &dynh,
        )
        .await;
        assert_eq!(o.skipped.as_deref(), Some("Syncing is paused"));
        let o = tick(
            &w.engine,
            &w.paths,
            true,
            Duration::ZERO,
            &mut checked,
            &dynh,
        )
        .await;
        assert!(o.report.is_some());
        assert_eq!(hooks.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn only_one_daemon_runs() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths {
            claude_home: d.path().join("c"),
            state_dir: d.path().join("s"),
        };
        let first = single_instance(&p).unwrap();
        assert!(single_instance(&p).is_none());
        drop(first);
        assert!(single_instance(&p).is_some());
    }
}
