//! The runner behind `claude-sync run-job <owner/repo> <id>` (§16). It checks the
//! acceptance again, takes the job's lock, skips when offline, runs
//! `claude -p <prompt> [args] --permission-mode <mode>` in the job's folder without a window, kills
//! it at the timeout, and keeps the output and the outcome.
use super::{job_dir, log_dir, Job, Jobs, JobsState};
use crate::claude_cli;
use crate::state::{now_unix, utc_stamp, write_json};
use crate::Paths;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Logs kept per job.
pub const KEEP_LOGS: usize = 20;
/// A lock older than the job's timeout plus this is left over from a crash.
pub const STALE_GRACE_SECS: u64 = 5 * 60;

/// `jobs/<id>/last-run.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    pub start_unix: u64,
    pub end_unix: u64,
    pub exit_code: Option<i32>,
    /// `ok`, `failed`, `timed out`, `skipped: offline`, or `error: <why>`.
    pub status: String,
    pub manual: bool,
    pub log: Option<PathBuf>,
    /// The job's report, when the run wrote it.
    pub report: Option<PathBuf>,
}

impl LastRun {
    fn file(paths: &Paths, id: &str) -> PathBuf {
        job_dir(paths, id).join("last-run.json")
    }
    pub fn load(paths: &Paths, id: &str) -> Option<Self> {
        let bytes = std::fs::read(Self::file(paths, id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
    fn save(&self, paths: &Paths, id: &str) -> crate::Result<()> {
        write_json(&Self::file(paths, id), self)
    }
}

/// Held while a job runs; removed on drop.
#[derive(Debug)]
pub struct Lock {
    path: PathBuf,
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_file(paths: &Paths, id: &str) -> PathBuf {
    job_dir(paths, id).join("lock")
}

/// When the lock was taken, from its content (`<pid> <unix>`), else its modification time.
fn lock_age(path: &Path, now: u64) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    let started = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| {
            let m = std::fs::metadata(path).ok()?.modified().ok()?;
            Some(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())
        })?;
    Some(now.saturating_sub(started))
}

fn stale_after(job: &Job) -> u64 {
    u64::from(job.timeout_minutes) * 60 + STALE_GRACE_SECS
}

/// Take the job's lock. `None`: a run holds it. A lock older than the timeout plus five
/// minutes is a crashed run's and is broken.
pub fn lock(paths: &Paths, job: &Job, now: u64) -> std::io::Result<Option<Lock>> {
    let path = lock_file(paths, &job.id);
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                write!(f, "{} {now}", std::process::id())?;
                return Ok(Some(Lock { path }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Unreadable (being written right now) counts as fresh.
                if lock_age(&path, now).unwrap_or(0) <= stale_after(job) {
                    return Ok(None);
                }
                tracing::warn!(job = %job.id, "breaking a stale job lock");
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

pub fn is_running(paths: &Paths, job: &Job) -> bool {
    lock_age(&lock_file(paths, &job.id), now_unix()).is_some_and(|age| age <= stale_after(job))
}

/// Keep the newest `keep` `.log` files of `dir` (names are UTC stamps, so they sort by time).
pub fn rotate(dir: &Path, keep: usize) -> std::io::Result<()> {
    let mut logs: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "log") && p.is_file())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    logs.sort();
    let excess = logs.len().saturating_sub(keep);
    for old in &logs[..excess] {
        std::fs::remove_file(old)?;
    }
    Ok(())
}

/// One `claude` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout: Duration,
}

/// Runs an invocation with its stdout and stderr going to `log`. `Ok(None)`: it timed out
/// and was killed.
pub trait Exec {
    fn exec(&self, inv: &Invocation, log: &File) -> std::io::Result<Option<i32>>;
}

/// The real process: no window, killed with its tree at the timeout.
pub struct RealExec;

impl Exec for RealExec {
    fn exec(&self, inv: &Invocation, log: &File) -> std::io::Result<Option<i32>> {
        let mut cmd = Command::new(&inv.program);
        cmd.args(&inv.args)
            .current_dir(&inv.cwd)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log.try_clone()?);
        let mut child = claude_cli::quiet(&mut cmd).spawn()?;
        claude_cli::wait_until(&mut child, Instant::now() + inv.timeout)
    }
}

/// Whether GitHub answers within a few seconds: no network, no run.
pub async fn is_online() -> bool {
    let Ok(client) = crate::github::client_builder()
        .timeout(Duration::from_secs(5))
        .build()
    else {
        return false;
    };
    client.head("https://api.github.com").send().await.is_ok()
}

impl Jobs<'_> {
    /// Run a job now. `manual` (Run now) also runs a job that is turned off; neither runs one
    /// that is not accepted in its current version. `Ok(None)`: another run holds the lock.
    pub fn run(
        &self,
        group: &crate::GroupId,
        id: &str,
        manual: bool,
        online: &dyn Fn() -> bool,
        exec: &dyn Exec,
    ) -> crate::Result<Option<LastRun>> {
        let job = self.find(group, id)?;
        let state = JobsState::load(self.paths)?;
        if !state.accepted(&job) {
            return Err(crate::Error::Invalid(format!(
                "job {id:?} is not accepted in its current version on this device"
            )));
        }
        let rec = state.of(&job).expect("accepted");
        if !manual && !rec.enabled {
            return Err(crate::Error::Invalid(format!("job {id:?} is turned off")));
        }
        let start = now_unix();
        let Some(_lock) = lock(self.paths, &job, start)? else {
            tracing::info!(job = id, "already running; skipped");
            return Ok(None);
        };
        let mut last = LastRun {
            start_unix: start,
            end_unix: start,
            exit_code: None,
            status: String::new(),
            manual,
            log: None,
            report: None,
        };
        let finish = |mut last: LastRun, status: String| -> crate::Result<Option<LastRun>> {
            last.status = status;
            last.end_unix = now_unix();
            last.save(self.paths, &job.id)?;
            Ok(Some(last))
        };
        if !online() {
            return finish(last, "skipped: offline".into());
        }
        let claude = rec
            .claude
            .clone()
            .filter(|p| p.is_file())
            .or_else(|| (self.resolve_claude)(&self.home));
        let Some(claude) = claude else {
            return finish(
                last,
                "error: Claude Code (the claude command) was not found".into(),
            );
        };
        if !job.cwd_path.is_dir() {
            return finish(
                last,
                format!("error: the folder {} does not exist", job.cwd),
            );
        }

        let dir = log_dir(self.paths, &job.id);
        std::fs::create_dir_all(&dir)?;
        let log_path = dir.join(format!("{}.log", utc_stamp(start)));
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        rotate(&dir, KEEP_LOGS)?;
        last.log = Some(log_path);
        let inv = Invocation {
            program: claude,
            args: ["-p".to_string(), job.prompt.clone()]
                .into_iter()
                .chain(job.args.iter().cloned())
                .chain(["--permission-mode".to_string(), job.permission_mode.clone()])
                .collect(),
            cwd: job.cwd_path.clone(),
            timeout: Duration::from_secs(u64::from(job.timeout_minutes) * 60),
        };
        let _ = writeln!(
            log,
            "[claude-sync] job {} of {} started {} ({}): {} -p <prompt> {}--permission-mode {} in {}\n",
            job.id,
            job.group,
            utc_stamp(start),
            if manual { "run now" } else { "scheduled" },
            inv.program.display(),
            job.args.iter().map(|a| format!("{a} ")).collect::<String>(),
            job.permission_mode,
            job.cwd_path.display(),
        );
        let result = exec.exec(&inv, &log);
        let status = match &result {
            Ok(Some(0)) => "ok".to_string(),
            Ok(Some(_)) => "failed".to_string(),
            Ok(None) => format!("timed out after {} min", job.timeout_minutes),
            Err(e) => format!("error: {e}"),
        };
        last.exit_code = result.ok().flatten();
        let _ = writeln!(
            log,
            "\n[claude-sync] {status} (exit code {}) at {}",
            last.exit_code.map_or("none".into(), |c| c.to_string()),
            utc_stamp(now_unix())
        );
        last.report = job.report_path.clone().filter(|p| written_since(p, start));
        finish(last, status)
    }
}

/// `path` was modified at or after `start` (seconds; file times can lag a little).
fn written_since(path: &Path, start: u64) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .is_some_and(|t| t.as_secs() + 2 >= start)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{group, Env, SCOUT};
    use super::*;
    use std::cell::RefCell;

    struct FakeExec {
        code: Option<i32>,
        write_report: Option<PathBuf>,
        seen: RefCell<Vec<Invocation>>,
    }

    impl FakeExec {
        fn new(code: Option<i32>) -> Self {
            Self {
                code,
                write_report: None,
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl Exec for FakeExec {
        fn exec(&self, inv: &Invocation, mut log: &File) -> std::io::Result<Option<i32>> {
            self.seen.borrow_mut().push(inv.clone());
            writeln!(log, "claude output")?;
            if let Some(p) = &self.write_report {
                std::fs::create_dir_all(p.parent().unwrap())?;
                std::fs::write(p, "# report")?;
            }
            Ok(self.code)
        }
    }

    fn accepted() -> (Env, crate::GroupId) {
        let e = Env::new();
        let g = group("a");
        e.groups(&[(&g, Some(SCOUT))]);
        let hash = e.jobs().find(&g, "setup-scout").unwrap().consent_hash();
        e.jobs().accept(&g, "setup-scout", &hash).unwrap();
        (e, g)
    }

    const ONLINE: &dyn Fn() -> bool = &|| true;

    #[test]
    fn a_run_logs_and_records_the_outcome() {
        let (e, g) = accepted();
        let mut exec = FakeExec::new(Some(0));
        let report = e.home.join(".claude/setup-scout/reports/latest.md");
        exec.write_report = Some(report.clone());
        let last = e
            .jobs()
            .run(&g, "setup-scout", false, ONLINE, &exec)
            .unwrap()
            .unwrap();
        assert_eq!(last.status, "ok");
        assert_eq!(last.exit_code, Some(0));
        assert_eq!(last.report.as_deref(), Some(report.as_path()));
        let inv = &exec.seen.borrow()[0];
        assert_eq!(inv.program, e.home.join("claude.exe"));
        assert_eq!(
            inv.args,
            [
                "-p",
                "/setup-scout:setup-scout unattended",
                "--chrome",
                "--permission-mode",
                "bypassPermissions"
            ]
        );
        assert_eq!(inv.cwd, e.home);
        assert_eq!(inv.timeout, Duration::from_secs(3600));
        let log = std::fs::read_to_string(last.log.as_ref().unwrap()).unwrap();
        assert!(log.contains("claude output"), "{log}");
        assert!(log.contains("[claude-sync] ok (exit code 0)"), "{log}");
        assert_eq!(LastRun::load(&e.paths, "setup-scout"), Some(last));
        // The lock is gone after the run.
        assert!(!lock_file(&e.paths, "setup-scout").exists());
        let v = &e.jobs().overview(&g).unwrap().jobs[0];
        assert!(v.has_log && v.has_report && !v.running);
    }

    #[test]
    fn outcomes() {
        let (e, g) = accepted();
        let run = |code| {
            e.jobs()
                .run(&g, "setup-scout", false, ONLINE, &FakeExec::new(code))
                .unwrap()
                .unwrap()
        };
        let failed = run(Some(3));
        assert_eq!(
            (failed.status.as_str(), failed.exit_code),
            ("failed", Some(3))
        );
        assert_eq!(failed.report, None);
        let timed_out = run(None);
        assert_eq!(timed_out.status, "timed out after 60 min");
        let offline = e
            .jobs()
            .run(&g, "setup-scout", false, &|| false, &FakeExec::new(Some(0)))
            .unwrap()
            .unwrap();
        assert_eq!(offline.status, "skipped: offline");
        assert_eq!(offline.log, None);
    }

    #[test]
    fn consent_is_checked_again_at_run_time() {
        let (e, g) = accepted();
        let exec = FakeExec::new(Some(0));
        e.jobs().set_enabled(&g, "setup-scout", false).unwrap();
        assert!(e
            .jobs()
            .run(&g, "setup-scout", false, ONLINE, &exec)
            .is_err());
        // Run now still works for a job that is turned off.
        assert!(e
            .jobs()
            .run(&g, "setup-scout", true, ONLINE, &exec)
            .unwrap()
            .is_some());
        // A changed job never runs, not even by hand.
        e.groups(&[(&g, Some(&SCOUT.replace("unattended", "x")))]);
        assert!(e
            .jobs()
            .run(&g, "setup-scout", true, ONLINE, &exec)
            .is_err());
        assert_eq!(exec.seen.borrow().len(), 1);
    }

    #[test]
    fn missing_claude_or_folder_is_an_error_run() {
        let (e, g) = accepted();
        std::fs::remove_file(e.home.join("claude.exe")).unwrap();
        let last = e
            .jobs()
            .run(&g, "setup-scout", false, ONLINE, &FakeExec::new(Some(0)))
            .unwrap()
            .unwrap();
        assert!(last.status.contains("not found"), "{}", last.status);
    }

    #[test]
    fn one_run_at_a_time_and_stale_locks_break() {
        let (e, g) = accepted();
        let job = e.jobs().find(&g, "setup-scout").unwrap();
        let now = now_unix();
        let held = lock(&e.paths, &job, now).unwrap().unwrap();
        assert!(lock(&e.paths, &job, now).unwrap().is_none());
        assert!(is_running(&e.paths, &job));
        let exec = FakeExec::new(Some(0));
        assert_eq!(
            e.jobs()
                .run(&g, "setup-scout", true, ONLINE, &exec)
                .unwrap(),
            None
        );
        assert!(exec.seen.borrow().is_empty());
        // Still fresh just inside the timeout plus grace.
        let limit = 3600 + STALE_GRACE_SECS;
        assert!(lock(&e.paths, &job, now + limit).unwrap().is_none());
        // Older than that: a crashed run's lock.
        let broken = lock(&e.paths, &job, now + limit + 1).unwrap();
        assert!(broken.is_some());
        drop(held);
        drop(broken);
        assert!(!lock_file(&e.paths, "setup-scout").exists());
        assert!(lock(&e.paths, &job, now).unwrap().is_some());
    }

    #[test]
    fn logs_rotate() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..25 {
            std::fs::write(
                tmp.path()
                    .join(format!("2026010{}T0000{:02}Z.log", i / 10, i)),
                "x",
            )
            .unwrap();
        }
        std::fs::write(tmp.path().join("notes.txt"), "keep").unwrap();
        rotate(tmp.path(), KEEP_LOGS).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left.len(), KEEP_LOGS + 1);
        assert_eq!(left[0], "20260100T000005Z.log");
        assert!(left.contains(&"notes.txt".to_string()));
        rotate(&tmp.path().join("missing"), KEEP_LOGS).unwrap();
    }

    #[test]
    fn a_real_run_with_a_fake_claude() {
        // The real executor on a stand-in for claude that prints its arguments.
        let (e, g) = accepted();
        #[cfg(windows)]
        let script = {
            let p = e.home.join("claude.cmd");
            std::fs::write(&p, "@echo args: %*\r\n@exit /b 4\r\n").unwrap();
            p
        };
        #[cfg(unix)]
        let script = {
            use std::os::unix::fs::PermissionsExt;
            let p = e.home.join("claude.sh");
            std::fs::write(&p, "#!/bin/sh\necho \"args: $*\"\nexit 4\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let inv = Invocation {
            program: script,
            args: vec!["-p".into(), "hello world".into()],
            cwd: e.home.clone(),
            timeout: Duration::from_secs(60),
        };
        let log_path = e.home.join("out.log");
        let log = File::create(&log_path).unwrap();
        assert_eq!(RealExec.exec(&inv, &log).unwrap(), Some(4));
        drop(log);
        let out = std::fs::read_to_string(&log_path).unwrap();
        assert!(out.contains("args: -p"), "{out}");
        assert!(out.contains("hello world"), "{out}");
        let _ = g;
    }
}
