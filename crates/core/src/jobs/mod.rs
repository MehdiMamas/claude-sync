//! Group scheduled jobs (§16): `schedules.json` in a group repo lists Claude Code prompts to
//! run on a schedule. A job runs on a device only after its member accepted it there; the
//! acceptance is bound to a hash of what the job does, so any change to it revokes it.
//!
//! - `schedules.json` is read from the group's base snapshot (it is repo only).
//! - `<state_dir>/jobs.json`: per job id, the group, the accepted hash, the on/off toggle, the
//!   resolved `claude` path and what is registered with the OS scheduler.
//! - `<state_dir>/jobs/<id>/`: the run lock and `last-run.json`, written by the runner.
//! - `<state_dir>/logs/jobs/<id>/<UTC stamp>.log`: the output of each run (last 20 kept).
//!
//! Job ids are global on a device (they name the OS task): when two groups use the same id,
//! the first group in the joined order keeps it and the other is reported.
pub mod cron;
pub mod os;
pub mod run;

use crate::state::{write_json, AppState};
use crate::{GroupId, Paths};
use cron::Schedule;
use os::{Scheduler, TaskSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const PERMISSION_MODES: [&str; 4] = ["default", "acceptEdits", "plan", "bypassPermissions"];
pub const MAX_PROMPT: usize = 4096;
pub const MAX_JOBS: usize = 20;
pub const DEFAULT_TIMEOUT: u32 = 60;
pub const MAX_TIMEOUT: u32 = 240;
/// The subcommand of the app binary a task runs: `claude-sync run-job <owner/repo> <id>`.
pub const RUN_JOB_ARG: &str = "run-job";

/// One valid job of a group's `schedules.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub group: GroupId,
    pub id: String,
    pub title: String,
    /// The cron fields joined by single spaces.
    pub cron: String,
    pub schedule: Schedule,
    pub prompt: String,
    /// As written (`~` or `~/...`); `cwd_path` is where it resolves on this device.
    pub cwd: String,
    pub cwd_path: PathBuf,
    pub permission_mode: String,
    pub timeout_minutes: u32,
    pub report: Option<String>,
    pub report_path: Option<PathBuf>,
}

impl Job {
    /// What a member accepts: changing any of these revokes the acceptance.
    pub fn consent_hash(&self) -> String {
        let fields = serde_json::json!([
            self.cron,
            self.prompt,
            self.cwd,
            self.permission_mode,
            self.timeout_minutes
        ]);
        crate::apply::sha256_hex(&serde_json::to_vec(&fields).expect("serializes"))
    }

    /// The OS task that runs this job through `program` (the app binary).
    pub fn task(&self, program: &Path) -> TaskSpec {
        TaskSpec::new(
            &self.id,
            &self.title,
            &self.schedule,
            program,
            vec![RUN_JOB_ARG.into(), self.group.to_string(), self.id.clone()],
            self.timeout_minutes,
        )
    }
}

/// A job (or the whole file) that was skipped, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub group: GroupId,
    pub id: Option<String>,
    pub message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawJob {
    id: String,
    #[serde(default)]
    title: Option<String>,
    cron: String,
    prompt: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    permission_mode: Option<String>,
    #[serde(default)]
    timeout_minutes: Option<u64>,
    #[serde(default)]
    report: Option<String>,
}

pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 41
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `~` or `~/a/b` under `home`. Nothing else: absolute paths differ between members, and
/// `..` could leave the home folder.
pub fn resolve_home_path(raw: &str, home: &Path) -> Result<PathBuf, String> {
    let rest = match raw {
        "~" => "",
        r => r.strip_prefix("~/").ok_or_else(|| {
            format!("{raw:?} must be ~ or start with ~/ (a folder in your home folder)")
        })?,
    };
    let mut out = home.to_path_buf();
    for part in rest.split('/').filter(|p| !p.is_empty()) {
        if part == "." || part == ".." || part.contains(['\\', ':']) || part.contains('\0') {
            return Err(format!("{raw:?} may not contain {part:?}"));
        }
        out.push(part);
    }
    Ok(out)
}

fn validate(group: &GroupId, raw: RawJob, home: &Path) -> Result<Job, String> {
    if !valid_id(&raw.id) {
        return Err(format!(
            "id {:?} must be lowercase letters, digits and dashes (at most 41, starting with a letter or digit)",
            raw.id
        ));
    }
    let schedule = Schedule::parse(&raw.cron)?;
    let prompt = raw.prompt;
    if prompt.trim().is_empty() {
        return Err("prompt is empty".into());
    }
    if prompt.len() > MAX_PROMPT {
        return Err(format!(
            "prompt is {} bytes; at most {MAX_PROMPT}",
            prompt.len()
        ));
    }
    if prompt.contains('\0') {
        return Err("prompt contains a NUL character".into());
    }
    if prompt.starts_with('-') {
        return Err("prompt may not start with - (it would be read as an option)".into());
    }
    let permission_mode = raw.permission_mode.unwrap_or_else(|| "default".into());
    if !PERMISSION_MODES.contains(&permission_mode.as_str()) {
        return Err(format!(
            "permissionMode {permission_mode:?} must be one of {}",
            PERMISSION_MODES.join(", ")
        ));
    }
    let timeout_minutes = match raw.timeout_minutes {
        None => DEFAULT_TIMEOUT,
        Some(t) if (1..=u64::from(MAX_TIMEOUT)).contains(&t) => t as u32,
        Some(t) => {
            return Err(format!(
                "timeoutMinutes {t} must be from 1 to {MAX_TIMEOUT}"
            ))
        }
    };
    let cwd = raw.cwd.unwrap_or_else(|| "~".into());
    let cwd_path = resolve_home_path(&cwd, home).map_err(|e| format!("cwd {e}"))?;
    let report_path = match &raw.report {
        None => None,
        Some(r) => Some(resolve_home_path(r, home).map_err(|e| format!("report {e}"))?),
    };
    if report_path.as_deref() == Some(home) {
        return Err("report must be a file, not ~".into());
    }
    let title = raw
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| raw.id.clone());
    Ok(Job {
        group: group.clone(),
        id: raw.id,
        title: title.chars().take(100).collect(),
        cron: raw.cron.split_whitespace().collect::<Vec<_>>().join(" "),
        schedule,
        prompt,
        cwd,
        cwd_path,
        permission_mode,
        timeout_minutes,
        report: raw.report,
        report_path,
    })
}

/// The valid jobs of one `schedules.json`, and what was skipped. A bad job never takes the
/// others down with it.
pub fn parse_file(group: &GroupId, bytes: &[u8], home: &Path) -> (Vec<Job>, Vec<Problem>) {
    let problem = |id: Option<String>, message: String| Problem {
        group: group.clone(),
        id,
        message,
    };
    let entries = match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(mut o)) => match o.remove("jobs") {
            Some(Value::Array(a)) => a,
            None => Vec::new(),
            Some(_) => {
                return (
                    Vec::new(),
                    vec![problem(
                        None,
                        format!("{}: \"jobs\" is not a list", crate::group::SCHEDULES),
                    )],
                )
            }
        },
        Ok(_) => {
            return (
                Vec::new(),
                vec![problem(
                    None,
                    format!("{} is not a JSON object", crate::group::SCHEDULES),
                )],
            )
        }
        Err(e) => {
            return (
                Vec::new(),
                vec![problem(
                    None,
                    format!("{} is not valid JSON: {e}", crate::group::SCHEDULES),
                )],
            )
        }
    };
    let mut jobs: Vec<Job> = Vec::new();
    let mut problems = Vec::new();
    for (i, entry) in entries.into_iter().enumerate() {
        let id = entry.get("id").and_then(Value::as_str).map(String::from);
        if i >= MAX_JOBS {
            problems.push(problem(
                id,
                format!("more than {MAX_JOBS} jobs; the rest are skipped"),
            ));
            continue;
        }
        let result = serde_json::from_value::<RawJob>(entry)
            .map_err(|e| e.to_string())
            .and_then(|raw| validate(group, raw, home));
        match result {
            Ok(job) if jobs.iter().any(|j| j.id == job.id) => {
                problems.push(problem(id, "the id is used twice in this file".into()));
            }
            Ok(job) => jobs.push(job),
            Err(e) => problems.push(problem(id, e)),
        }
    }
    (jobs, problems)
}

fn yes() -> bool {
    true
}

/// `jobs.json`: what this device decided about each job.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobsState {
    #[serde(default)]
    pub jobs: BTreeMap<String, Record>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub group: GroupId,
    /// [`Job::consent_hash`] of the version the member accepted.
    #[serde(default)]
    pub accepted: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The `claude` binary, resolved when the task was registered.
    #[serde(default)]
    pub claude: Option<PathBuf>,
    /// [`TaskSpec::hash`] of the task registered with the OS; `None`: none is.
    #[serde(default)]
    pub registered: Option<String>,
    /// Why the last registration failed.
    #[serde(default)]
    pub error: Option<String>,
}

impl JobsState {
    pub fn load(paths: &Paths) -> crate::Result<Self> {
        match std::fs::read(state_file(paths)) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, paths: &Paths) -> crate::Result<()> {
        write_json(&state_file(paths), self)
    }

    /// The record of `job` when it is the same group's; a record of another group with the
    /// same id says nothing about this job.
    pub fn of(&self, job: &Job) -> Option<&Record> {
        self.jobs.get(&job.id).filter(|r| r.group == job.group)
    }

    /// Accepted in exactly this version.
    pub fn accepted(&self, job: &Job) -> bool {
        self.of(job)
            .is_some_and(|r| r.accepted.as_deref() == Some(&*job.consent_hash()))
    }
}

pub fn state_file(paths: &Paths) -> PathBuf {
    paths.state_dir.join("jobs.json")
}
pub fn job_dir(paths: &Paths, id: &str) -> PathBuf {
    paths.state_dir.join("jobs").join(id)
}
pub fn log_dir(paths: &Paths, id: &str) -> PathBuf {
    paths.logs_dir().join("jobs").join(id)
}

/// How a job looks on the group screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: String,
    pub title: String,
    pub cron: String,
    /// `Mon and Thu at 10:17` (local time).
    pub schedule: String,
    pub prompt: String,
    pub cwd: String,
    pub permission_mode: String,
    pub timeout_minutes: u32,
    pub report: Option<String>,
    /// Accepted in this version.
    pub accepted: bool,
    /// Accepted before, but the job changed since.
    pub changed: bool,
    pub enabled: bool,
    pub registered: bool,
    pub error: Option<String>,
    pub last_run: Option<run::LastRun>,
    pub running: bool,
    pub has_log: bool,
    pub has_report: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobsOverview {
    /// `None`, or why this OS cannot run jobs.
    pub unsupported: Option<String>,
    pub jobs: Vec<JobView>,
    pub problems: Vec<Problem>,
}

/// What `reconcile` changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileReport {
    pub registered: Vec<String>,
    pub unregistered: Vec<String>,
    pub errors: Vec<String>,
}

/// Jobs on one device: `schedules.json` of every joined group, `jobs.json`, the OS scheduler.
pub struct Jobs<'a> {
    pub paths: &'a Paths,
    pub home: PathBuf,
    pub scheduler: &'a dyn Scheduler,
    /// The app binary the tasks run.
    pub program: PathBuf,
    /// Finds `claude` (`claude_cli::resolve`; tests pass their own).
    pub resolve_claude: fn(&Path) -> Option<PathBuf>,
}

impl Jobs<'_> {
    /// The group's `schedules.json` from its base snapshot. `Err`: it could not be read.
    fn file_of(&self, group: &GroupId) -> crate::Result<Option<Vec<u8>>> {
        crate::engine::read_base_file(self.paths, group, crate::group::SCHEDULES)
    }

    /// The valid jobs of every joined group in order, with what was skipped, and the groups
    /// whose file could not be read (their tasks are left alone).
    pub fn all(&self) -> crate::Result<(Vec<Job>, Vec<Problem>, BTreeSet<GroupId>)> {
        let groups = AppState::load(self.paths)?.groups;
        let (mut jobs, mut problems, mut unreadable) = (Vec::new(), Vec::new(), BTreeSet::new());
        for g in &groups {
            let bytes = match self.file_of(g) {
                Ok(Some(b)) => b,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(group = %g, error = %e, "cannot read schedules");
                    unreadable.insert(g.clone());
                    continue;
                }
            };
            let (found, bad) = parse_file(g, &bytes, &self.home);
            problems.extend(bad);
            for job in found {
                match jobs.iter().find(|j: &&Job| j.id == job.id) {
                    Some(first) => problems.push(Problem {
                        group: g.clone(),
                        id: Some(job.id.clone()),
                        message: format!(
                            "the id is already used by a job of {}, which comes first",
                            first.group
                        ),
                    }),
                    None => jobs.push(job),
                }
            }
        }
        Ok((jobs, problems, unreadable))
    }

    /// The job `id` of `group`, if it is valid and not shadowed by another group.
    pub fn find(&self, group: &GroupId, id: &str) -> crate::Result<Job> {
        let (jobs, problems, _) = self.all()?;
        if let Some(job) = jobs.into_iter().find(|j| &j.group == group && j.id == id) {
            return Ok(job);
        }
        let why = problems
            .into_iter()
            .find(|p| &p.group == group && p.id.as_deref() == Some(id))
            .map_or_else(
                || format!("{group} has no job {id:?}"),
                |p| format!("job {id:?} of {group} is skipped: {}", p.message),
            );
        Err(crate::Error::Invalid(why))
    }

    pub fn overview(&self, group: &GroupId) -> crate::Result<JobsOverview> {
        let (jobs, problems, _) = self.all()?;
        let state = JobsState::load(self.paths)?;
        let views = jobs
            .iter()
            .filter(|j| &j.group == group)
            .map(|job| {
                let rec = state.of(job);
                let accepted = state.accepted(job);
                let last_run = run::LastRun::load(self.paths, &job.id);
                JobView {
                    id: job.id.clone(),
                    title: job.title.clone(),
                    cron: job.cron.clone(),
                    schedule: job.schedule.describe(),
                    prompt: job.prompt.clone(),
                    cwd: job.cwd.clone(),
                    permission_mode: job.permission_mode.clone(),
                    timeout_minutes: job.timeout_minutes,
                    report: job.report.clone(),
                    accepted,
                    changed: !accepted && rec.is_some_and(|r| r.accepted.is_some()),
                    enabled: rec.is_none_or(|r| r.enabled),
                    registered: accepted && rec.is_some_and(|r| r.registered.is_some()),
                    error: rec.and_then(|r| r.error.clone()),
                    running: run::is_running(self.paths, job),
                    has_log: last_run
                        .as_ref()
                        .and_then(|r| r.log.as_ref())
                        .is_some_and(|l| l.is_file()),
                    has_report: job.report_path.as_ref().is_some_and(|p| p.is_file()),
                    last_run,
                }
            })
            .collect();
        Ok(JobsOverview {
            unsupported: self.scheduler.supported().err(),
            jobs: views,
            problems: problems.into_iter().filter(|p| &p.group == group).collect(),
        })
    }

    /// Accept the current version of a job and turn it on, then register it.
    pub fn accept(&self, group: &GroupId, id: &str, hash: &str) -> crate::Result<ReconcileReport> {
        let job = self.find(group, id)?;
        // The member accepted what the screen showed; a job that changed since needs a new look.
        if job.consent_hash() != hash {
            return Err(crate::Error::Invalid(format!(
                "job {id:?} changed while you were looking at it; review it again"
            )));
        }
        let mut state = JobsState::load(self.paths)?;
        let rec = state.jobs.entry(id.to_string()).or_insert_with(|| Record {
            group: group.clone(),
            accepted: None,
            enabled: true,
            claude: None,
            registered: None,
            error: None,
        });
        if &rec.group != group {
            // A left group's record that reconcile has not cleaned up yet.
            rec.group = group.clone();
            rec.registered = None;
        }
        rec.accepted = Some(job.consent_hash());
        rec.enabled = true;
        state.save(self.paths)?;
        self.reconcile(false)
    }

    pub fn set_enabled(
        &self,
        group: &GroupId,
        id: &str,
        on: bool,
    ) -> crate::Result<ReconcileReport> {
        let job = self.find(group, id)?;
        let mut state = JobsState::load(self.paths)?;
        match state.jobs.get_mut(id).filter(|r| r.group == job.group) {
            Some(rec) if rec.accepted.is_some() => rec.enabled = on,
            _ => {
                return Err(crate::Error::Invalid(format!(
                    "job {id:?} is not accepted on this device"
                )))
            }
        }
        state.save(self.paths)?;
        self.reconcile(false)
    }

    /// Make the OS tasks match: accepted and enabled jobs registered, everything else not.
    /// `verify` also asks the OS whether registered tasks still exist (at startup).
    pub fn reconcile(&self, verify: bool) -> crate::Result<ReconcileReport> {
        let joined = AppState::load(self.paths)?.groups;
        let (jobs, _, unreadable) = self.all()?;
        let mut state = JobsState::load(self.paths)?;
        let mut report = ReconcileReport::default();
        let sched = self.scheduler;
        let unregister = |id: &str, rec: &mut Record, report: &mut ReconcileReport| {
            if rec.registered.is_none() {
                return true;
            }
            match sched.unregister(id) {
                Ok(()) => {
                    rec.registered = None;
                    report.unregistered.push(id.to_string());
                    true
                }
                Err(e) => {
                    rec.error = Some(e.to_string());
                    report.errors.push(format!("{id}: {e}"));
                    false
                }
            }
        };

        // Left groups: remove their tasks and forget their records.
        let left: Vec<String> = state
            .jobs
            .iter()
            .filter(|(_, r)| !joined.contains(&r.group))
            .map(|(id, _)| id.clone())
            .collect();
        for id in left {
            let rec = state.jobs.get_mut(&id).expect("listed");
            if unregister(&id, rec, &mut report) {
                state.jobs.remove(&id);
            }
        }

        for job in &jobs {
            let accepted = state.accepted(job);
            let Some(rec) = state.jobs.get_mut(&job.id).filter(|r| r.group == job.group) else {
                continue;
            };
            if !(accepted && rec.enabled) {
                unregister(&job.id, rec, &mut report);
                continue;
            }
            if let Err(why) = sched.supported() {
                rec.error = Some(why);
                continue;
            }
            if !rec.claude.as_ref().is_some_and(|p| p.is_file()) {
                rec.claude = (self.resolve_claude)(&self.home);
            }
            if rec.claude.is_none() {
                rec.error = Some("Claude Code (the claude command) was not found".into());
                unregister(&job.id, rec, &mut report);
                continue;
            }
            let task = job.task(&self.program);
            let hash = task.hash();
            let current = rec.registered.as_deref() == Some(&*hash)
                && (!verify || sched.exists(&job.id).unwrap_or(false));
            if current {
                rec.error = None;
                continue;
            }
            match sched.register(&task) {
                Ok(()) => {
                    rec.registered = Some(hash);
                    rec.error = None;
                    report.registered.push(job.id.clone());
                }
                Err(e) => {
                    rec.registered = None;
                    rec.error = Some(e.to_string());
                    report.errors.push(format!("{}: {e}", job.id));
                }
            }
        }

        // Jobs that left their (readable) file: unregister, keep the record.
        let gone: Vec<String> = state
            .jobs
            .iter()
            .filter(|(id, r)| {
                r.registered.is_some()
                    && !unreadable.contains(&r.group)
                    && !jobs.iter().any(|j| &j.id == *id && j.group == r.group)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            let rec = state.jobs.get_mut(&id).expect("listed");
            unregister(&id, rec, &mut report);
        }
        state.save(self.paths)?;
        if !report.registered.is_empty() || !report.unregistered.is_empty() {
            tracing::info!(registered = ?report.registered, unregistered = ?report.unregistered, "jobs reconciled");
        }
        for e in &report.errors {
            tracing::warn!(error = %e, "job registration failed");
        }
        Ok(report)
    }

    /// Remove every task this device registered (the uninstaller runs this). The records keep
    /// their acceptance, so a reinstall registers them again.
    pub fn remove_all(&self) -> crate::Result<ReconcileReport> {
        let mut state = JobsState::load(self.paths)?;
        let mut report = ReconcileReport::default();
        for (id, rec) in state.jobs.iter_mut() {
            match self.scheduler.unregister(id) {
                Ok(()) => {
                    if rec.registered.take().is_some() {
                        report.unregistered.push(id.clone());
                    }
                }
                Err(e) => report.errors.push(format!("{id}: {e}")),
            }
        }
        state.save(self.paths)?;
        Ok(report)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::os::TaskSpec;
    use super::*;
    use std::sync::Mutex;

    /// Records what would have been registered; never touches the OS.
    #[derive(Default)]
    pub struct FakeScheduler {
        pub tasks: Mutex<BTreeMap<String, TaskSpec>>,
        pub fail: Mutex<bool>,
        pub calls: Mutex<Vec<String>>,
    }

    impl Scheduler for FakeScheduler {
        fn register(&self, task: &TaskSpec) -> crate::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("register {}", task.id));
            if *self.fail.lock().unwrap() {
                return Err(crate::Error::Invalid("access denied".into()));
            }
            self.tasks
                .lock()
                .unwrap()
                .insert(task.id.clone(), task.clone());
            Ok(())
        }
        fn unregister(&self, id: &str) -> crate::Result<()> {
            self.calls.lock().unwrap().push(format!("unregister {id}"));
            self.tasks.lock().unwrap().remove(id);
            Ok(())
        }
        fn exists(&self, id: &str) -> crate::Result<bool> {
            Ok(self.tasks.lock().unwrap().contains_key(id))
        }
    }

    pub struct Env {
        pub _tmp: tempfile::TempDir,
        pub paths: Paths,
        pub home: PathBuf,
        pub sched: FakeScheduler,
    }

    pub fn fake_claude(home: &Path) -> Option<PathBuf> {
        let p = home.join("claude.exe");
        p.is_file().then_some(p)
    }

    impl Env {
        pub fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("claude.exe"), b"").unwrap();
            let paths = Paths {
                claude_home: home.join(".claude"),
                state_dir: home.join(".claude-sync"),
            };
            Self {
                _tmp: tmp,
                paths,
                home,
                sched: FakeScheduler::default(),
            }
        }

        pub fn jobs(&self) -> Jobs<'_> {
            Jobs {
                paths: &self.paths,
                home: self.home.clone(),
                scheduler: &self.sched,
                program: PathBuf::from("/app/claude-sync"),
                resolve_claude: fake_claude,
            }
        }

        /// Join `groups` (in order) with these `schedules.json` contents in their base.
        pub fn groups(&self, groups: &[(&GroupId, Option<&str>)]) {
            let mut st = AppState::load(&self.paths).unwrap();
            st.groups = groups.iter().map(|(g, _)| (*g).clone()).collect();
            st.save(&self.paths).unwrap();
            for (g, file) in groups {
                let f = self
                    .paths
                    .base_dir(g)
                    .join("files")
                    .join(crate::group::SCHEDULES);
                match file {
                    Some(text) => {
                        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
                        std::fs::write(f, text).unwrap();
                    }
                    None => {
                        let _ = std::fs::remove_file(f);
                    }
                }
            }
        }

        pub fn registered(&self) -> Vec<String> {
            self.sched.tasks.lock().unwrap().keys().cloned().collect()
        }
    }

    pub fn group(name: &str) -> GroupId {
        GroupId::new("acme", name)
    }

    pub const SCOUT: &str = r#"{"jobs":[{"id":"setup-scout","title":"Claude setup refresh","cron":"17 10 * * 1,4","prompt":"/setup-scout:setup-scout unattended","cwd":"~","permissionMode":"bypassPermissions","timeoutMinutes":60,"report":"~/.claude/setup-scout/reports/latest.md"}]}"#;

    fn hash_of(e: &Env, g: &GroupId, id: &str) -> String {
        e.jobs().find(g, id).unwrap().consent_hash()
    }

    #[test]
    fn parses_the_documented_example() {
        let home = Path::new("/home/me");
        let (jobs, problems) = parse_file(&group("a"), SCOUT.as_bytes(), home);
        assert!(problems.is_empty(), "{problems:?}");
        let j = &jobs[0];
        assert_eq!(j.id, "setup-scout");
        assert_eq!(j.title, "Claude setup refresh");
        assert_eq!(j.schedule.describe(), "Mon and Thu at 10:17");
        assert_eq!(j.cwd_path, home);
        assert_eq!(
            j.report_path.as_deref(),
            Some(
                home.join(".claude")
                    .join("setup-scout")
                    .join("reports")
                    .join("latest.md")
                    .as_path()
            )
        );
        assert_eq!(j.permission_mode, "bypassPermissions");
        assert_eq!(j.timeout_minutes, 60);
    }

    #[test]
    fn defaults() {
        let (jobs, _) = parse_file(
            &group("a"),
            br#"{"jobs":[{"id":"x","cron":"0 9 * * *","prompt":"hi"}]}"#,
            Path::new("/h"),
        );
        let j = &jobs[0];
        assert_eq!(j.title, "x");
        assert_eq!(j.cwd, "~");
        assert_eq!(j.permission_mode, "default");
        assert_eq!(j.timeout_minutes, DEFAULT_TIMEOUT);
        assert_eq!(j.report, None);
    }

    #[test]
    fn bad_jobs_are_skipped_and_reported() {
        let long = "x".repeat(MAX_PROMPT + 1);
        let file = serde_json::json!({"jobs": [
            {"id": "ok", "cron": "0 9 * * *", "prompt": "fine"},
            {"id": "Bad_Id", "cron": "0 9 * * *", "prompt": "p"},
            {"id": "-x", "cron": "0 9 * * *", "prompt": "p"},
            {"id": "a".repeat(42), "cron": "0 9 * * *", "prompt": "p"},
            {"id": "cron", "cron": "*/5 * * * *", "prompt": "p"},
            {"id": "mode", "cron": "0 9 * * *", "prompt": "p", "permissionMode": "dangerous"},
            {"id": "cwd-abs", "cron": "0 9 * * *", "prompt": "p", "cwd": "/etc"},
            {"id": "cwd-up", "cron": "0 9 * * *", "prompt": "p", "cwd": "~/../other"},
            {"id": "cwd-win", "cron": "0 9 * * *", "prompt": "p", "cwd": "~/a\\..\\b"},
            {"id": "report-up", "cron": "0 9 * * *", "prompt": "p", "report": "~/x/../../y"},
            {"id": "report-home", "cron": "0 9 * * *", "prompt": "p", "report": "~"},
            {"id": "t0", "cron": "0 9 * * *", "prompt": "p", "timeoutMinutes": 0},
            {"id": "t241", "cron": "0 9 * * *", "prompt": "p", "timeoutMinutes": 241},
            {"id": "empty", "cron": "0 9 * * *", "prompt": "  \n"},
            {"id": "long", "cron": "0 9 * * *", "prompt": long},
            {"id": "noprompt", "cron": "0 9 * * *"},
            {"id": "dash", "cron": "0 9 * * *", "prompt": "--dangerously-skip-permissions"},
            {"id": "ok", "cron": "0 10 * * *", "prompt": "again"},
            "not an object",
        ]});
        let (jobs, problems) =
            parse_file(&group("a"), file.to_string().as_bytes(), Path::new("/h"));
        assert_eq!(
            jobs.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(),
            ["ok"]
        );
        assert_eq!(jobs[0].prompt, "fine");
        assert_eq!(problems.len(), 18, "{problems:#?}");
        let msg = |id: &str| {
            problems
                .iter()
                .find(|p| p.id.as_deref() == Some(id))
                .unwrap()
                .message
                .clone()
        };
        assert!(msg("mode").contains("bypassPermissions"));
        assert!(msg("cwd-abs").contains("~/"));
        assert!(msg("t241").contains("1 to 240"));
        assert!(msg("ok").contains("twice"));
        assert!(msg("cron").contains("*"));

        for (bad, why) in [
            (&b"nope"[..], "not valid JSON"),
            (b"[]", "not a JSON object"),
            (br#"{"jobs":{}}"#, "not a list"),
        ] {
            let (jobs, problems) = parse_file(&group("a"), bad, Path::new("/h"));
            assert!(jobs.is_empty());
            assert!(problems[0].message.contains(why), "{problems:?}");
        }
        let (jobs, problems) = parse_file(&group("a"), b"{}", Path::new("/h"));
        assert!(jobs.is_empty() && problems.is_empty());
    }

    #[test]
    fn home_paths() {
        let h = Path::new("/h");
        assert_eq!(resolve_home_path("~", h).unwrap(), h);
        assert_eq!(
            resolve_home_path("~/a//b/", h).unwrap(),
            h.join("a").join("b")
        );
        for bad in [
            "",
            "/a",
            "a",
            "~a",
            "~/a/../..",
            "~/./a",
            "~/c:x",
            "C:/x",
            "~\\a",
        ] {
            assert!(resolve_home_path(bad, h).is_err(), "{bad}");
        }
    }

    #[test]
    fn consent_hash_covers_what_the_job_does() {
        let h = Path::new("/h");
        let base = serde_json::json!({"id":"x","title":"T","cron":"17 10 * * 1,4","prompt":"p","cwd":"~","permissionMode":"plan","timeoutMinutes":30,"report":"~/r.md"});
        let hash = |v: &Value| {
            let (jobs, _) = parse_file(
                &group("a"),
                serde_json::json!({"jobs":[v]}).to_string().as_bytes(),
                h,
            );
            jobs[0].consent_hash()
        };
        let original = hash(&base);
        // Cosmetic changes keep the acceptance.
        for (k, v) in [
            ("title", "Other"),
            ("report", "~/other.md"),
            ("cron", " 17  10 * * 1,4"),
        ] {
            let mut j = base.clone();
            j[k] = v.into();
            assert_eq!(hash(&j), original, "{k}");
        }
        // What runs, where, how, and when revoke it.
        for (k, v) in [
            ("cron", Value::from("18 10 * * 1,4")),
            ("prompt", "p2".into()),
            ("cwd", "~/x".into()),
            ("permissionMode", "bypassPermissions".into()),
            ("timeoutMinutes", 31.into()),
        ] {
            let mut j = base.clone();
            j[k] = v;
            assert_ne!(hash(&j), original, "{k}");
        }
        // An omitted default is the same as the default spelled out.
        let mut a = base.clone();
        a.as_object_mut().unwrap().remove("permissionMode");
        let mut b = base.clone();
        b["permissionMode"] = "default".into();
        assert_eq!(hash(&a), hash(&b));
    }

    #[test]
    fn nothing_registers_before_acceptance() {
        let e = Env::new();
        let g = group("a");
        e.groups(&[(&g, Some(SCOUT))]);
        let r = e.jobs().reconcile(true).unwrap();
        assert_eq!(r, ReconcileReport::default());
        assert!(e.registered().is_empty());
        let o = e.jobs().overview(&g).unwrap();
        assert_eq!(o.jobs.len(), 1);
        assert!(!o.jobs[0].accepted && !o.jobs[0].changed && !o.jobs[0].registered);
        assert_eq!(o.jobs[0].schedule, "Mon and Thu at 10:17");
    }

    #[test]
    fn accepting_registers_and_a_change_revokes() {
        let e = Env::new();
        let g = group("a");
        e.groups(&[(&g, Some(SCOUT))]);
        let hash = hash_of(&e, &g, "setup-scout");
        assert!(e.jobs().accept(&g, "setup-scout", "stale").is_err());
        let r = e.jobs().accept(&g, "setup-scout", &hash).unwrap();
        assert_eq!(r.registered, ["setup-scout"]);
        let task = e.sched.tasks.lock().unwrap()["setup-scout"].clone();
        assert_eq!(task.args, ["run-job", "acme/a", "setup-scout"]);
        assert_eq!(task.program, PathBuf::from("/app/claude-sync"));
        assert_eq!(task.times, [(10, 17)]);
        let rec = JobsState::load(&e.paths).unwrap().jobs["setup-scout"].clone();
        assert_eq!(rec.claude, Some(e.home.join("claude.exe")));

        // Nothing changed: nothing is registered again.
        e.sched.calls.lock().unwrap().clear();
        assert_eq!(
            e.jobs().reconcile(false).unwrap(),
            ReconcileReport::default()
        );
        assert_eq!(
            e.jobs().reconcile(true).unwrap(),
            ReconcileReport::default()
        );
        assert!(e.sched.calls.lock().unwrap().is_empty());

        // The prompt changes: the task goes until the member accepts again.
        e.groups(&[(&g, Some(&SCOUT.replace("unattended", "and push")))]);
        let r = e.jobs().reconcile(false).unwrap();
        assert_eq!(r.unregistered, ["setup-scout"]);
        assert!(e.registered().is_empty());
        let v = &e.jobs().overview(&g).unwrap().jobs[0];
        assert!(!v.accepted && v.changed && !v.registered);

        let hash = hash_of(&e, &g, "setup-scout");
        e.jobs().accept(&g, "setup-scout", &hash).unwrap();
        assert_eq!(e.registered(), ["setup-scout"]);

        // A cosmetic change re-registers (new title) without asking again.
        let renamed = SCOUT
            .replace("unattended", "and push")
            .replace("Claude setup refresh", "Refresh");
        e.groups(&[(&g, Some(&renamed))]);
        let r = e.jobs().reconcile(false).unwrap();
        assert_eq!(r.registered, ["setup-scout"]);
        assert!(e.jobs().overview(&g).unwrap().jobs[0].accepted);
    }

    #[test]
    fn toggles_and_removal() {
        let e = Env::new();
        let g = group("a");
        e.groups(&[(&g, Some(SCOUT))]);
        assert!(e.jobs().set_enabled(&g, "setup-scout", false).is_err());
        let hash = hash_of(&e, &g, "setup-scout");
        e.jobs().accept(&g, "setup-scout", &hash).unwrap();

        e.jobs().set_enabled(&g, "setup-scout", false).unwrap();
        assert!(e.registered().is_empty());
        let v = &e.jobs().overview(&g).unwrap().jobs[0];
        assert!(v.accepted && !v.enabled && !v.registered);
        e.jobs().set_enabled(&g, "setup-scout", true).unwrap();
        assert_eq!(e.registered(), ["setup-scout"]);

        // The job leaves the file.
        e.groups(&[(&g, Some(r#"{"jobs":[]}"#))]);
        assert_eq!(
            e.jobs().reconcile(false).unwrap().unregistered,
            ["setup-scout"]
        );
        // It comes back unchanged: the acceptance still holds.
        e.groups(&[(&g, Some(SCOUT))]);
        assert_eq!(
            e.jobs().reconcile(false).unwrap().registered,
            ["setup-scout"]
        );

        // A broken file unregisters (nothing valid is left) but keeps the acceptance.
        e.groups(&[(&g, Some("{oops"))]);
        assert_eq!(
            e.jobs().reconcile(false).unwrap().unregistered,
            ["setup-scout"]
        );
        assert_eq!(e.jobs().overview(&g).unwrap().problems.len(), 1);

        // Leaving the group forgets the record.
        e.groups(&[(&g, Some(SCOUT))]);
        e.jobs().reconcile(false).unwrap();
        e.groups(&[]);
        assert_eq!(
            e.jobs().reconcile(false).unwrap().unregistered,
            ["setup-scout"]
        );
        assert!(JobsState::load(&e.paths).unwrap().jobs.is_empty());
    }

    #[test]
    fn verify_restores_a_task_deleted_by_hand() {
        let e = Env::new();
        let g = group("a");
        e.groups(&[(&g, Some(SCOUT))]);
        e.jobs()
            .accept(&g, "setup-scout", &hash_of(&e, &g, "setup-scout"))
            .unwrap();
        e.sched.tasks.lock().unwrap().clear();
        assert_eq!(
            e.jobs().reconcile(false).unwrap(),
            ReconcileReport::default()
        );
        assert_eq!(
            e.jobs().reconcile(true).unwrap().registered,
            ["setup-scout"]
        );
    }

    #[test]
    fn failures_and_missing_claude_are_recorded() {
        let e = Env::new();
        let g = group("a");
        e.groups(&[(&g, Some(SCOUT))]);
        *e.sched.fail.lock().unwrap() = true;
        let r = e
            .jobs()
            .accept(&g, "setup-scout", &hash_of(&e, &g, "setup-scout"))
            .unwrap();
        assert_eq!(r.errors.len(), 1);
        let v = &e.jobs().overview(&g).unwrap().jobs[0];
        assert!(v.accepted && !v.registered);
        assert_eq!(v.error.as_deref(), Some("access denied"));
        *e.sched.fail.lock().unwrap() = false;
        assert_eq!(
            e.jobs().reconcile(false).unwrap().registered,
            ["setup-scout"]
        );
        assert_eq!(e.jobs().overview(&g).unwrap().jobs[0].error, None);

        // claude is uninstalled: the stored path is re-resolved, then the task goes.
        std::fs::remove_file(e.home.join("claude.exe")).unwrap();
        let r = e.jobs().reconcile(false).unwrap();
        assert_eq!(r.unregistered, ["setup-scout"]);
        let v = &e.jobs().overview(&g).unwrap().jobs[0];
        assert!(v.error.as_deref().unwrap().contains("not found"));
    }

    #[test]
    fn the_first_group_keeps_a_shared_id() {
        let e = Env::new();
        let (a, b) = (group("a"), group("b"));
        e.groups(&[(&a, Some(SCOUT)), (&b, Some(SCOUT))]);
        let ob = e.jobs().overview(&b).unwrap();
        assert!(ob.jobs.is_empty());
        assert!(ob.problems[0].message.contains("acme/a"));
        assert!(e.jobs().find(&b, "setup-scout").is_err());
        assert!(e.jobs().accept(&b, "setup-scout", "x").is_err());

        // An acceptance in group a says nothing about b's job of the same id.
        e.jobs()
            .accept(&a, "setup-scout", &hash_of(&e, &a, "setup-scout"))
            .unwrap();
        e.groups(&[(&b, Some(SCOUT)), (&a, Some(SCOUT))]);
        e.jobs().reconcile(false).unwrap();
        assert!(e.registered().is_empty());
        assert!(!e.jobs().overview(&b).unwrap().jobs[0].accepted);
    }

    #[test]
    fn unsupported_os_never_registers() {
        let e = Env::new();
        let g = group("a");
        e.groups(&[(&g, Some(SCOUT))]);
        let un = os::Unsupported;
        let jobs = Jobs {
            scheduler: &un,
            ..e.jobs()
        };
        jobs.accept(&g, "setup-scout", &hash_of(&e, &g, "setup-scout"))
            .unwrap();
        let o = jobs.overview(&g).unwrap();
        assert!(o.unsupported.is_some());
        assert!(!o.jobs[0].registered);
        assert!(o.jobs[0]
            .error
            .as_deref()
            .unwrap()
            .contains("not supported"));
    }

    #[test]
    fn remove_all_unregisters_and_keeps_acceptance() {
        let e = Env::new();
        let g = group("a");
        e.groups(&[(&g, Some(SCOUT))]);
        e.jobs()
            .accept(&g, "setup-scout", &hash_of(&e, &g, "setup-scout"))
            .unwrap();
        assert_eq!(e.jobs().remove_all().unwrap().unregistered, ["setup-scout"]);
        assert!(e.registered().is_empty());
        assert_eq!(
            e.jobs().reconcile(false).unwrap().registered,
            ["setup-scout"]
        );
    }
}
