//! Group jobs (Parity 41, 42, 64, 73, 74, 75): accept, on/off, auto-accept, run now, open a
//! file, the OS task of every accepted job, and the run itself (`run-job <id>`).
//!
//! Acceptance, the off switch and auto-accept are choices (they follow the person); the store
//! `jobs` table keeps the hash of the task this device registered and the last result.
//! The group's `schedules.json` is read from the copy the pass keeps (`schedules/<repo>`).
use crate::choices::Choices;
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{Event, JobView};
use claude_sync_engine::formats::schedules::{validate, JobEntry, DEFAULT_TIMEOUT};
use claude_sync_engine::traits::Store;
use claude_sync_fs::atomic::sha256_hex;
use claude_sync_fs::exec::utc_stamp;
use claude_sync_fs::tasks::{remove_all, Kind, Runner, SystemRunner, TaskSpec, Tasks};
use claude_sync_store::Db;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const KEEP_LOGS: usize = 20;
/// A lock older than the timeout plus this is a crashed run's.
pub const STALE_GRACE_SECS: u64 = 300;

type Spawn = Arc<dyn Fn(&[String]) -> std::io::Result<()> + Send + Sync>;

/// What the jobs need from the machine; tests inject fakes.
#[derive(Clone)]
pub struct JobsEnv {
    /// The program the OS tasks and Run now start (the app binary).
    pub exe: PathBuf,
    pub kind: Kind,
    pub runner: Arc<dyn Runner + Send + Sync>,
    pub claude: Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    pub login_path: Arc<dyn Fn() -> Option<OsString> + Send + Sync>,
    pub online: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Start `exe args` detached (Run now must not block the actor).
    pub spawn: Spawn,
}

impl JobsEnv {
    pub fn system(home: &Path) -> Self {
        let exe = std::env::current_exe().unwrap_or_default();
        let spawn_exe = exe.clone();
        let h = home.to_path_buf();
        JobsEnv {
            exe,
            kind: os_kind(home),
            runner: Arc::new(SystemRunner),
            claude: Arc::new(move || find_claude(&h)),
            login_path: Arc::new(login_shell_path),
            online: Arc::new(online),
            spawn: Arc::new(move |args| {
                let mut c = Command::new(&spawn_exe);
                claude_sync_fs::quiet(&mut c);
                c.args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map(|_| ())
            }),
        }
    }
}

fn os_kind(home: &Path) -> Kind {
    let out = |p: &str, a: &[&str]| {
        claude_sync_fs::quiet(Command::new(p).args(a))
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    if cfg!(windows) {
        Kind::Windows {
            temp: home.join(".claude-sync").join("tmp"),
            user: String::new(),
        }
    } else if cfg!(target_os = "macos") {
        Kind::Launchd {
            agents_dir: home.join("Library/LaunchAgents"),
            uid: out("id", &["-u"]).unwrap_or_default(),
        }
    } else if Path::new("/run/systemd/system").is_dir() {
        Kind::Systemd {
            units_dir: home.join(".config/systemd/user"),
        }
    } else {
        Kind::Cron
    }
}

fn find_claude(home: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    let fixed = [
        home.join(".local/bin").join(name),
        home.join(".claude/local").join(name),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
        home.join(".npm-global/bin/claude"),
    ];
    let on_path = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join(name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    fixed.into_iter().chain(on_path).find(|p| p.is_file())
}

/// The `PATH` an interactive login shell builds (nvm, asdf, brew ...). Windows tasks already
/// run with the user's environment.
fn login_shell_path() -> Option<OsString> {
    if cfg!(windows) {
        return None;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut child =
        claude_sync_fs::quiet(Command::new(shell).args(["-ilc", "printf %s \"$PATH\""]))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().ok()?.is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut child.stdout?, &mut text).ok()?;
    // rc files may print noise first; the PATH is the last line
    let last = text.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    last.contains('/').then(|| OsString::from(last))
}

/// Whether GitHub answers within a few seconds: no network, no run.
fn online() -> bool {
    use std::net::ToSocketAddrs;
    "api.github.com:443"
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .is_some_and(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_secs(5)).is_ok())
}

/// `PATH` of a scheduled session: login shell entries, then the current ones, then claude's
/// own folder and the usual user bin folders that exist; first one wins.
pub fn session_path(
    home: &Path,
    claude: &Path,
    current: Option<&std::ffi::OsStr>,
    login: Option<&std::ffi::OsStr>,
) -> OsString {
    let mut dirs: Vec<PathBuf> = [login, current]
        .into_iter()
        .flatten()
        .flat_map(|l| std::env::split_paths(l).collect::<Vec<_>>())
        .collect();
    dirs.extend(claude.parent().map(Path::to_path_buf));
    dirs.extend(
        [
            home.join(".local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            home.join(".cargo/bin"),
            home.join(".bun/bin"),
            home.join(".npm-global/bin"),
        ]
        .into_iter()
        .filter(|d| d.is_dir()),
    );
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| !d.as_os_str().is_empty() && seen.insert(d.clone()));
    std::env::join_paths(dirs).unwrap_or_default()
}

/// One valid job of a group's `schedules.json`, with the defaults filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Job {
    pub repo: String,
    pub id: String,
    pub title: String,
    pub cron: String,
    pub prompt: String,
    pub cwd: String,
    pub mode: String,
    pub timeout: u32,
    pub args: Vec<String>,
    pub report: Option<String>,
}

impl Job {
    fn new(repo: &str, e: JobEntry) -> Job {
        let title = e.title.as_deref().map(str::trim).unwrap_or_default();
        Job {
            repo: repo.into(),
            title: if title.is_empty() {
                e.id.clone()
            } else {
                title.into()
            },
            id: e.id,
            cron: e.cron.split_whitespace().collect::<Vec<_>>().join(" "),
            prompt: e.prompt,
            cwd: e.cwd.unwrap_or_else(|| "~".into()),
            mode: e.permission_mode.unwrap_or_else(|| "default".into()),
            timeout: e.timeout_minutes.map_or(DEFAULT_TIMEOUT, |t| t as u32),
            args: e.args.unwrap_or_default(),
            report: e.report,
        }
    }

    /// What a member accepts; the same bytes as v1, so an old acceptance still holds.
    pub fn consent_hash(&self) -> String {
        let f = serde_json::json!([
            self.cron,
            self.prompt,
            self.cwd,
            self.mode,
            self.timeout,
            self.args
        ]);
        sha256_hex(&serde_json::to_vec(&f).unwrap_or_default())
    }

    fn resolve(raw: &str, home: &Path) -> PathBuf {
        match raw.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => home.to_path_buf(),
        }
    }

    fn spec(&self, exe: &Path) -> Result<TaskSpec> {
        TaskSpec::new(&self.id, &self.title, &self.cron, exe, self.timeout).map_err(Error::new)
    }
}

/// shortcut: the Debug text of the spec is the hash input; a compiler that prints it
/// differently re-registers every task once.
fn spec_hash(s: &TaskSpec) -> String {
    sha256_hex(format!("{s:?}").as_bytes())
}

fn job_dir(state: &Path, id: &str) -> PathBuf {
    state.join("jobs").join(id)
}

fn log_dir(state: &Path, id: &str) -> PathBuf {
    state.join("logs").join("jobs").join(id)
}

fn lock_age(path: &Path, now: i64) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    let at = text.split_whitespace().nth(1)?.parse::<i64>().ok()?;
    Some(now.saturating_sub(at).max(0) as u64)
}

/// Take the job's lock. `None`: a run holds it. A lock older than timeout*60+300 s is a
/// crashed run's and is broken.
fn take_lock(state: &Path, job: &Job, now: i64) -> std::io::Result<Option<LockGuard>> {
    let path = job_dir(state, &job.id).join("lock");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                write!(f, "{} {now}", std::process::id())?;
                return Ok(Some(LockGuard(path)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // unreadable (being written right now) counts as fresh
                let age = lock_age(&path, now).unwrap_or(0);
                if age <= u64::from(job.timeout) * 60 + STALE_GRACE_SECS {
                    return Ok(None);
                }
                eprintln!("claude-sync: breaking a stale job lock ({})", job.id);
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn rotate(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .collect();
    logs.sort(); // UTC stamps sort by time
    let excess = logs.len().saturating_sub(keep);
    for old in &logs[..excess] {
        let _ = std::fs::remove_file(old);
    }
}

fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: killpg takes no pointers; at worst the group is gone.
        unsafe { libc::killpg(pgid, libc::SIGKILL) };
    }
    let _ = child.kill();
}

impl Core {
    pub fn set_jobs_env(&mut self, e: JobsEnv) {
        self.jobs_env = e;
    }

    /// The valid jobs of every group that is on, in the person's order; a repeated id keeps
    /// the first.
    pub(crate) fn jobs_all(&self) -> Vec<Job> {
        let c = self.choices();
        let mut jobs: Vec<Job> = vec![];
        for g in self.db.groups().unwrap_or_default() {
            if g.off || c.groups.get(&g.repo).is_some_and(|x| x.off) {
                continue;
            }
            let Some(bytes) = Store::read(&self.db, &format!("schedules/{}", g.repo))
                .ok()
                .flatten()
                .filter(|b| !b.is_empty())
            else {
                continue;
            };
            let Ok(v) = serde_json::from_slice(&bytes) else {
                continue;
            };
            for e in validate(&v).0 {
                if !jobs.iter().any(|j| j.id == e.id) {
                    jobs.push(Job::new(&g.repo, e));
                }
            }
        }
        jobs
    }

    fn job(&self, repo: &str, id: &str) -> Result<Job> {
        self.jobs_all()
            .into_iter()
            .find(|j| j.repo == repo && j.id == id)
            .ok_or_else(|| Error::new(format!("{repo} has no job {id:?}")))
    }

    fn accepted(c: &Choices, j: &Job) -> bool {
        c.groups
            .get(&j.repo)
            .and_then(|g| g.accepted_jobs.get(&j.id))
            .is_some_and(|h| *h == j.consent_hash())
    }

    fn job_off(c: &Choices, j: &Job) -> bool {
        c.groups
            .get(&j.repo)
            .is_some_and(|g| g.off_jobs.contains(&j.id))
    }

    pub(crate) fn job_views(&self) -> Vec<JobView> {
        let c = self.choices();
        let rows = self.db.jobs().unwrap_or_default();
        self.jobs_all()
            .iter()
            .map(|j| JobView {
                repo: j.repo.clone(),
                id: j.id.clone(),
                accepted: Self::accepted(&c, j),
                enabled: !Self::job_off(&c, j),
                last_result: rows
                    .iter()
                    .find(|r| r.repo == j.repo && r.job_id == j.id)
                    .map(|r| r.last_result.clone())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// Register the task of every accepted, turned-on job and remove the others. A job already
    /// registered the same way is never touched, so it never runs twice. `verify`: also
    /// re-register one whose OS task is gone.
    pub(crate) fn reconcile_jobs(&self, verify: bool) -> Result<()> {
        let c = self.choices();
        let io = &self.jobs_env;
        let t = Tasks {
            kind: io.kind.clone(),
            run: io.runner.as_ref(),
        };
        let rows = self.db.jobs()?;
        let mut errors: Vec<String> = vec![];
        let want: Vec<Job> = self
            .jobs_all()
            .into_iter()
            .filter(|j| Self::accepted(&c, j) && !Self::job_off(&c, j))
            .collect();
        for r in rows.iter().filter(|r| !r.spec_hash.is_empty()) {
            if want.iter().any(|j| j.repo == r.repo && j.id == r.job_id) {
                continue;
            }
            match t.unregister(&r.job_id) {
                Ok(()) => self.db.set_job_spec(&r.repo, &r.job_id, "")?,
                Err(e) => errors.push(format!("{}: {e}", r.job_id)),
            }
        }
        for j in &want {
            let spec = match j.spec(&io.exe) {
                Ok(s) => s,
                Err(e) => {
                    errors.push(format!("{}: {e}", j.id));
                    continue;
                }
            };
            let hash = spec_hash(&spec);
            let have = rows
                .iter()
                .find(|r| r.repo == j.repo && r.job_id == j.id)
                .map(|r| r.spec_hash.as_str());
            if have == Some(hash.as_str()) && !(verify && !t.exists(&j.id).unwrap_or(false)) {
                continue;
            }
            match t.register(&spec) {
                Ok(()) => self.db.set_job_spec(&j.repo, &j.id, &hash)?,
                Err(e) => errors.push(format!("{}: {e}", j.id)),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::new(errors.join("; ")))
        }
    }

    /// After a pass: groups on auto-accept take their new and changed jobs, then the OS tasks
    /// follow.
    pub(crate) fn jobs_after_pass(&self) {
        let mut c = self.choices();
        let mut changed = false;
        for j in self.jobs_all() {
            let auto = c.groups.get(&j.repo).is_some_and(|g| g.auto_run_jobs);
            if auto && !Self::accepted(&c, &j) {
                let now = self.now() as u64 * 1000;
                c.group(&j.repo, now)
                    .accepted_jobs
                    .insert(j.id.clone(), j.consent_hash());
                changed = true;
            }
        }
        if changed {
            let _ = c.save(&self.db);
        }
        if let Err(e) = self.reconcile_jobs(true) {
            let _ = self
                .db
                .add_notification("job", &format!("jobs: {e}"), None, self.now());
        }
    }

    pub(crate) fn job_accept(&mut self, repo: &str, id: &str) -> Result<Event> {
        let j = self.job(repo, id)?;
        let mut c = self.choices();
        let g = c.group(repo, self.now() as u64 * 1000);
        g.accepted_jobs.insert(id.into(), j.consent_hash());
        g.off_jobs.remove(id);
        c.save(&self.db)?;
        self.reconcile_jobs(false)?;
        Ok(Event::Done)
    }

    pub(crate) fn job_enable(&mut self, repo: &str, id: &str, on: bool) -> Result<Event> {
        self.job(repo, id)?;
        let mut c = self.choices();
        let g = c.group(repo, self.now() as u64 * 1000);
        if on {
            g.off_jobs.remove(id);
        } else {
            g.off_jobs.insert(id.into());
        }
        c.save(&self.db)?;
        self.reconcile_jobs(false)?;
        Ok(Event::Done)
    }

    pub(crate) fn jobs_auto_accept(&mut self, repo: &str, on: bool) -> Result<Event> {
        let mut c = self.choices();
        c.group(repo, self.now() as u64 * 1000).auto_run_jobs = on;
        c.save(&self.db)?;
        self.jobs_after_pass();
        Ok(Event::Done)
    }

    /// Run now starts the run in its own process, so a long job never blocks the actor.
    pub(crate) fn job_run_now(&mut self, repo: &str, id: &str) -> Result<Event> {
        let j = self.job(repo, id)?;
        if !Self::accepted(&self.choices(), &j) {
            return Err(Error::new(format!("job {id:?} is not accepted yet")));
        }
        (self.jobs_env.spawn)(&["run-job".into(), id.into(), "--now".into()])?;
        Ok(Event::Done)
    }

    /// The newest log of the job, or its report when the last run wrote it (Parity 42).
    pub(crate) fn job_file(&self, repo: &str, id: &str, report: bool) -> Result<Event> {
        let j = self.job(repo, id)?;
        let path = if report {
            j.report
                .as_deref()
                .map(|r| Job::resolve(r, &self.env.home))
                .filter(|p| p.is_file())
        } else {
            let mut logs: Vec<PathBuf> = std::fs::read_dir(log_dir(&self.home.state, id))
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "log"))
                .collect();
            logs.sort();
            logs.pop()
        };
        Ok(Event::Path(path.map(|p| p.to_string_lossy().into_owned())))
    }

    /// The job's task goes before its group does.
    pub(crate) fn unregister_group_jobs(&self, repo: &str) -> Result<()> {
        let io = &self.jobs_env;
        let t = Tasks {
            kind: io.kind.clone(),
            run: io.runner.as_ref(),
        };
        for r in self.db.jobs()?.iter().filter(|r| r.repo == repo) {
            if !r.spec_hash.is_empty() {
                t.unregister(&r.job_id).map_err(Error::new)?;
            }
        }
        Ok(())
    }

    /// `claude-sync run-job <id>`: what an OS task and Run now call. Returns the status.
    pub fn run_job(&self, id: &str, manual: bool) -> Result<String> {
        let j = self
            .jobs_all()
            .into_iter()
            .find(|j| j.id == id)
            .ok_or_else(|| Error::new(format!("no job {id:?} in any group on this device")))?;
        let c = self.choices();
        if !Self::accepted(&c, &j) {
            return Err(Error::new(format!(
                "job {id:?} is not accepted in its current version"
            )));
        }
        if !manual && Self::job_off(&c, &j) {
            return Err(Error::new(format!("job {id:?} is turned off")));
        }
        let start = self.now();
        let state = &self.home.state;
        let Some(_lock) = take_lock(state, &j, start)? else {
            return Ok("already running".into());
        };
        let finish = |status: String| -> Result<String> {
            self.db
                .set_job_result(&j.repo, &j.id, &status, self.now())?;
            Ok(status)
        };
        let io = &self.jobs_env;
        if !(io.online)() {
            return finish("skipped: offline".into());
        }
        let Some(claude) = (io.claude)() else {
            return finish("error: Claude Code (the claude command) was not found".into());
        };
        let cwd = Job::resolve(&j.cwd, &self.env.home);
        if !cwd.is_dir() {
            return finish(format!("error: the folder {} does not exist", j.cwd));
        }
        let dir = log_dir(state, id);
        std::fs::create_dir_all(&dir)?;
        let log_path = dir.join(format!("{}.log", utc_stamp(start)));
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        rotate(&dir, KEEP_LOGS);
        // launchd hands jobs only /usr/bin:/bin:...; give the session the user's PATH
        let path = (!cfg!(windows)).then(|| {
            session_path(
                &self.env.home,
                &claude,
                std::env::var_os("PATH").as_deref(),
                (io.login_path)().as_deref(),
            )
        });
        let _ = writeln!(
            log,
            "[claude-sync] job {id} of {} started {} ({}): {} -p <prompt> {}--permission-mode {} in {}\n",
            j.repo,
            utc_stamp(start),
            if manual { "run now" } else { "scheduled" },
            claude.display(),
            j.args.iter().map(|a| format!("{a} ")).collect::<String>(),
            j.mode,
            cwd.display(),
        );
        if let Some(p) = &path {
            let _ = writeln!(log, "[claude-sync] PATH: {}", p.to_string_lossy());
        }
        let mut cmd = Command::new(&claude);
        cmd.arg("-p")
            .arg(&j.prompt)
            .args(&j.args)
            .args(["--permission-mode", &j.mode])
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log.try_clone()?);
        if let Some(p) = path {
            cmd.env("PATH", p);
        }
        claude_sync_fs::quiet(&mut cmd);
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let result = (|| -> std::io::Result<Option<i32>> {
            let mut child = cmd.spawn()?;
            let deadline = Instant::now() + Duration::from_secs(u64::from(j.timeout) * 60);
            loop {
                if let Some(s) = child.try_wait()? {
                    return Ok(Some(s.code().unwrap_or(-1)));
                }
                if Instant::now() >= deadline {
                    kill_tree(&mut child);
                    let _ = child.wait();
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })();
        let status = match &result {
            Ok(Some(0)) => "ok".to_string(),
            Ok(Some(_)) => "failed".to_string(),
            Ok(None) => format!("timed out after {} min", j.timeout),
            Err(e) => format!("error: {e}"),
        };
        let _ = writeln!(
            log,
            "\n[claude-sync] {status} (exit code {}) at {}",
            result
                .ok()
                .flatten()
                .map_or("none".into(), |c| c.to_string()),
            utc_stamp(self.now())
        );
        finish(status)
    }
}

/// `claude-sync remove-jobs`: every OS task this device registered goes; the acceptances stay.
/// The ids come from the store, or from v1's `jobs.json` when there is no store yet. Any
/// failure is returned, so the command exits 1.
pub fn remove_jobs(home: &Path, io: &JobsEnv) -> Result<usize> {
    let state = home.join(".claude-sync");
    let db_path = state.join("claude-sync.db");
    let db = db_path.is_file().then(|| Db::open(&db_path)).transpose()?;
    let ids: Vec<String> = match &db {
        Some(db) => db
            .jobs()?
            .into_iter()
            .filter(|r| !r.spec_hash.is_empty())
            .map(|r| r.job_id)
            .collect(),
        None => std::fs::read(state.join("jobs.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["jobs"].as_object().map(|o| o.keys().cloned().collect()))
            .unwrap_or_default(),
    };
    let t = Tasks {
        kind: io.kind.clone(),
        run: io.runner.as_ref(),
    };
    let errors = remove_all(&t, &ids);
    if !errors.is_empty() {
        return Err(Error::new(errors.join("; ")));
    }
    if let Some(db) = &db {
        for r in db.jobs()? {
            db.set_job_spec(&r.repo, &r.job_id, "")?;
        }
    }
    Ok(ids.len())
}
