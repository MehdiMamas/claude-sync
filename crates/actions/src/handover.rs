//! The v1 daemon hand-over and `daemon.owner` (v2-Cutover). A v2 start that finds `daemon.lock`
//! held by a process that is not a live v2 daemon takes it for a v1 daemon: it waits for the
//! v1 pass to end (`sync.lock`), stops the process and polls the lock, all within one bound.
//! The process list and the kill go through [`Procs`], so tests never touch real processes.
use claude_sync_fs::pass::SyncLock;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};

/// Written by every v2 daemon after it takes `daemon.lock`, removed on exit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub pid: u32,
    /// OS start time of the process, seconds since the epoch.
    pub started: u64,
    pub version: String,
    pub exe: String,
}

pub fn owner_path(state: &Path) -> PathBuf {
    state.join("daemon.owner")
}

fn start_time(pid: u32) -> Option<u64> {
    let pid = Pid::from_u32(pid);
    let mut sys = System::new().ok()?;
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map(|p| p.start_time())
}

/// The owner record of this process.
pub fn own_owner(version: &str) -> Owner {
    let pid = std::process::id();
    Owner {
        pid,
        started: start_time(pid).unwrap_or_default(),
        version: version.into(),
        exe: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    }
}

pub fn write_owner(state: &Path, o: &Owner) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(o).map_err(|e| e.to_string())?;
    std::fs::write(owner_path(state), bytes).map_err(|e| format!("cannot write daemon.owner: {e}"))
}

/// The daemon that is alive: the owner file names a process that exists and started when the
/// file says (a reused pid starts later).
pub fn live_owner(state: &Path) -> Option<Owner> {
    let o: Owner = serde_json::from_slice(&std::fs::read(owner_path(state)).ok()?).ok()?;
    (start_time(o.pid)? == o.started).then_some(o)
}

/// One process, as much as the match needs.
#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub same_user: bool,
    pub cmd: Vec<String>,
    pub exe: Option<PathBuf>,
}

pub trait Procs: Send + Sync {
    fn list(&self) -> Vec<Proc>;
    /// SIGTERM on Unix, `TerminateProcess` on Windows.
    fn terminate(&self, pid: u32) -> bool;
    /// A systemd user unit `claude-sync.service` is installed.
    fn unit_installed(&self) -> bool;
    /// `systemctl --user restart claude-sync`.
    fn restart_unit(&self) -> bool;
}

/// The real thing.
pub struct SystemProcs;

impl Procs for SystemProcs {
    fn list(&self) -> Vec<Proc> {
        let Ok(mut sys) = System::new() else {
            return vec![];
        };
        let kind = ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always)
            .with_user(UpdateKind::Always);
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        let me = sys
            .process(Pid::from_u32(std::process::id()))
            .and_then(|p| p.user_id().cloned());
        sys.processes()
            .values()
            .map(|p| Proc {
                pid: p.pid().as_u32(),
                same_user: me.is_some() && p.user_id() == me.as_ref(),
                cmd: p
                    .cmd()
                    .iter()
                    .map(|s| s.to_string_lossy().into_owned())
                    .collect(),
                exe: p.exe().map(Path::to_path_buf),
            })
            .collect()
    }

    fn terminate(&self, pid: u32) -> bool {
        let pid = Pid::from_u32(pid);
        let Ok(mut sys) = System::new() else {
            return false;
        };
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        let Some(p) = sys.process(pid) else {
            return false;
        };
        #[cfg(unix)]
        {
            p.kill_with(Signal::Term).unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = Signal::Term;
            p.kill()
        }
    }

    fn unit_installed(&self) -> bool {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|h| h.join(".config")))
            .is_some_and(|b| b.join("systemd/user/claude-sync.service").is_file())
    }

    fn restart_unit(&self) -> bool {
        std::process::Command::new("systemctl")
            .args(["--user", "restart", "claude-sync"])
            .status()
            .is_ok_and(|s| s.success())
    }
}

/// Lists nothing and stops nothing: for tests and callers that must not touch processes.
pub struct NoProcs;

impl Procs for NoProcs {
    fn list(&self) -> Vec<Proc> {
        vec![]
    }
    fn terminate(&self, _: u32) -> bool {
        false
    }
    fn unit_installed(&self) -> bool {
        false
    }
    fn restart_unit(&self) -> bool {
        false
    }
}

pub fn system() -> Arc<dyn Procs> {
    Arc::new(SystemProcs)
}

pub const WAIT: Duration = Duration::from_secs(30);
pub const POLL: Duration = Duration::from_secs(1);

/// `claude-sync-cli daemon`; on Windows the image may be `.exe`, and `update` may have
/// renamed it to `.old`.
fn is_v1_daemon(p: &Proc) -> bool {
    let Some(first) = p.cmd.first() else {
        return false;
    };
    let mut name = first
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_lowercase();
    while let Some(n) = name
        .strip_suffix(".old")
        .or_else(|| name.strip_suffix(".exe"))
    {
        name = n.to_string();
    }
    name == "claude-sync-cli" && p.cmd[1..].iter().any(|a| a == "daemon")
}

fn lock_free(state: &Path) -> bool {
    let f = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("daemon.lock"));
    f.is_ok_and(|f| f.try_lock().is_ok())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Handover {
    /// `daemon.lock` is free now; `killed` says a v1 daemon was stopped to get it.
    Free { killed: bool },
    /// A live v2 daemon holds it.
    V2Holds,
    /// A v1 daemon still holds it (`pid` 0: none found).
    Stuck { pid: u32 },
}

/// Steps 1 to 4 of the hand-over, within `wait`.
pub fn free_daemon_lock(
    state: &Path,
    procs: &dyn Procs,
    (own_pid, own_exe): (u32, Option<&Path>),
    wait: Duration,
    poll: Duration,
) -> Handover {
    let start = Instant::now();
    if lock_free(state) {
        return Handover::Free { killed: false };
    }
    // a live v2 daemon holds it (its pid is never a match)
    if live_owner(state).is_some() {
        return Handover::V2Holds;
    }
    // the v1 daemon holds sync.lock only during a pass: wait for the pass, then it cannot start one
    let Ok(_pass) = SyncLock::acquire(&state.join("sync.lock"), wait) else {
        return Handover::Stuck { pid: 0 };
    };
    let found: Vec<u32> = procs
        .list()
        .into_iter()
        .filter(|p| {
            p.same_user
                && p.pid != own_pid
                && is_v1_daemon(p)
                && (own_exe.is_none() || p.exe.as_deref() != own_exe)
        })
        .map(|p| p.pid)
        .collect();
    let Some(&pid) = found.first() else {
        return Handover::Stuck { pid: 0 };
    };
    for p in &found {
        procs.terminate(*p);
    }
    loop {
        if lock_free(state) {
            return Handover::Free { killed: true };
        }
        if start.elapsed() >= wait {
            return Handover::Stuck { pid };
        }
        std::thread::sleep(poll.min(wait.saturating_sub(start.elapsed())));
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    /// A process list the test controls; `terminate` runs `on_kill` (e.g. drops a lock).
    #[derive(Default)]
    pub struct FakeProcs {
        pub procs: Vec<Proc>,
        pub unit: bool,
        pub killed: Mutex<Vec<u32>>,
        pub restarts: Mutex<u32>,
        pub on_kill: Mutex<Option<Box<dyn FnMut() + Send>>>,
    }

    impl Procs for FakeProcs {
        fn list(&self) -> Vec<Proc> {
            self.procs.clone()
        }
        fn terminate(&self, pid: u32) -> bool {
            self.killed.lock().unwrap().push(pid);
            if let Some(f) = self.on_kill.lock().unwrap().as_mut() {
                f();
            }
            true
        }
        fn unit_installed(&self) -> bool {
            self.unit
        }
        fn restart_unit(&self) -> bool {
            *self.restarts.lock().unwrap() += 1;
            true
        }
    }

    pub fn v1(pid: u32, exe: &str) -> Proc {
        Proc {
            pid,
            same_user: true,
            cmd: vec![
                "/home/ann/.local/bin/claude-sync-cli".into(),
                "daemon".into(),
            ],
            exe: Some(exe.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use std::sync::Mutex;

    const FAST: Duration = Duration::from_millis(20);

    fn hold(state: &Path) -> File {
        std::fs::create_dir_all(state).unwrap();
        let f = File::create(state.join("daemon.lock")).unwrap();
        f.lock().unwrap();
        f
    }

    #[test]
    fn a_reused_pid_is_not_live() {
        let d = tempfile::tempdir().unwrap();
        let mut o = own_owner("2.0.0");
        write_owner(d.path(), &o).unwrap();
        assert_eq!(live_owner(d.path()), Some(o.clone()));
        // the same pid, started at another time: another process owns the number now
        o.started += 100;
        write_owner(d.path(), &o).unwrap();
        assert!(live_owner(d.path()).is_none());
    }

    #[test]
    fn a_v1_daemon_is_found_and_stopped_and_the_lock_is_freed() {
        let d = tempfile::tempdir().unwrap();
        let lock = Arc::new(Mutex::new(Some(hold(d.path()))));
        let fake = FakeProcs {
            procs: vec![v1(4242, "/x/old")],
            ..Default::default()
        };
        let l = lock.clone();
        *fake.on_kill.lock().unwrap() = Some(Box::new(move || drop(l.lock().unwrap().take())));
        let r = free_daemon_lock(
            d.path(),
            &fake,
            (1, Some(Path::new("/x/new"))),
            FAST * 50,
            FAST,
        );
        assert_eq!(r, Handover::Free { killed: true });
        assert_eq!(*fake.killed.lock().unwrap(), [4242]);
    }

    #[test]
    fn our_own_pid_and_our_own_exe_and_other_users_and_other_commands_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        let _lock = hold(d.path());
        let mut other_user = v1(3, "/x/old");
        other_user.same_user = false;
        let mut other_cmd = v1(4, "/x/old");
        other_cmd.cmd = vec!["vim".into(), "daemon".into()];
        let mut win = v1(5, "C:/x/claude-sync-cli.exe.old");
        win.cmd = vec![
            "C:\\Users\\Ann\\claude-sync-cli.exe.old".into(),
            "daemon".into(),
        ];
        let fake = FakeProcs {
            procs: vec![v1(1, "/x/old"), v1(2, "/x/new"), other_user, other_cmd, win],
            ..Default::default()
        };
        let r = free_daemon_lock(d.path(), &fake, (1, Some(Path::new("/x/new"))), FAST, FAST);
        // only the renamed Windows image matched, and it never let go
        assert_eq!(r, Handover::Stuck { pid: 5 });
        assert_eq!(*fake.killed.lock().unwrap(), [5]);
    }

    #[test]
    fn a_live_v2_daemon_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let _lock = hold(d.path());
        write_owner(d.path(), &own_owner("2.0.0")).unwrap();
        let fake = FakeProcs {
            procs: vec![v1(9, "/x/old")],
            ..Default::default()
        };
        let r = free_daemon_lock(d.path(), &fake, (1, None), FAST, FAST);
        assert_eq!(r, Handover::V2Holds);
        assert!(fake.killed.lock().unwrap().is_empty());
    }

    #[test]
    fn a_daemon_that_does_not_stop_in_time_is_reported_with_its_pid() {
        let d = tempfile::tempdir().unwrap();
        let _lock = hold(d.path());
        let fake = FakeProcs {
            procs: vec![v1(77, "/x/old")],
            ..Default::default()
        };
        let t = Instant::now();
        let r = free_daemon_lock(d.path(), &fake, (1, None), FAST * 5, FAST);
        assert_eq!(r, Handover::Stuck { pid: 77 });
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(!owner_path(d.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn hold_lock_child() {
        // Run only as the child of `sigterm_frees_the_lock_of_a_real_process`.
        let Some(p) = std::env::var_os("CLAUDE_SYNC_TEST_HOLD") else {
            return;
        };
        let f = File::create(p).unwrap();
        f.lock().unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }

    #[cfg(unix)]
    #[test]
    fn sigterm_frees_the_lock_of_a_real_process() {
        let d = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "handover::tests::hold_lock_child", "--nocapture"])
            .env("CLAUDE_SYNC_TEST_HOLD", d.path().join("daemon.lock"))
            .spawn()
            .unwrap();
        let t = Instant::now();
        while lock_free(d.path()) && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!lock_free(d.path()), "the child never took the lock");
        assert!(SystemProcs.terminate(child.id()));
        let _ = child.wait();
        assert!(lock_free(d.path()));
    }
}
