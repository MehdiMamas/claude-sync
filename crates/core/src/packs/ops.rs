//! What a member can do with a pack, shared by the app commands and the command line:
//! list, start setup, check, turn on, turn off. These change `packs.json` and the machine's
//! own plugin values; applying them (jobs, rule lines, placed files) is the engine's
//! `reapply` and the jobs reconcile, which the callers run next.
use super::state::{self, By, CheckResult, PackState, Record};
use super::{check, launch, load_catalog, Pack, Problem};
use crate::jobs::Job;
use crate::{GroupId, Paths};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// This machine, for tests to change.
#[derive(Debug, Clone)]
pub struct Env<'a> {
    pub home: &'a Path,
    /// `windows`, `macos` or `linux` (see [`super::this_os`]).
    pub os: &'a str,
    pub now_ms: u64,
}

impl<'a> Env<'a> {
    pub fn real(home: &'a Path) -> Self {
        Self {
            home,
            os: super::this_os(),
            now_ms: crate::state::now_unix() * 1000,
        }
    }
}

/// A job of a pack, as the member sees it before turning the pack on.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackJob {
    pub id: String,
    pub title: String,
    pub schedule: String,
    pub prompt: String,
    pub cwd: String,
    pub permission_mode: String,
    pub timeout_minutes: u32,
    pub args: Vec<String>,
    pub report: Option<String>,
    /// What "Turn on" sends back: the consent hash of the version shown.
    pub hash: String,
}

impl From<&Job> for PackJob {
    fn from(j: &Job) -> Self {
        Self {
            id: j.id.clone(),
            title: j.title.clone(),
            schedule: j.schedule.describe(),
            prompt: j.prompt.clone(),
            cwd: j.cwd.clone(),
            permission_mode: j.permission_mode.clone(),
            timeout_minutes: j.timeout_minutes,
            args: j.args.clone(),
            report: j.report.clone(),
            hash: j.consent_hash(),
        }
    }
}

/// One row of the Optional packs card.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackView {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub needs: Vec<super::Need>,
    pub os: Vec<String>,
    /// `available` (not set up), `tried`, `ready`, `on`, `failing`, `off` or `unavailable`
    /// (not for this OS).
    pub state: &'static str,
    pub by: Option<By>,
    /// Unix milliseconds.
    pub since: Option<u64>,
    pub last_check: Option<CheckResult>,
    pub plugins: Vec<String>,
    pub jobs: Vec<PackJob>,
    /// The first line of the pack's rules and how many lines there are.
    pub rule_preview: Option<String>,
    pub rule_lines: usize,
    pub has_files: bool,
    /// A setup terminal was opened and Claude Code is still running in it.
    pub setting_up: bool,
    /// The setup terminal closed; the check has not run yet.
    pub claude_closed: bool,
    /// The pack changed since this machine accepted it; its check is due.
    pub changed: bool,
    /// The group still turns the plugin on for everyone, so turning the pack off keeps it.
    pub group_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PacksOverview {
    pub packs: Vec<PackView>,
    pub problems: Vec<Problem>,
    /// This machine is signed in as the group's host: packs in use stayed on.
    pub host: bool,
    pub os: String,
}

fn err(msg: impl Into<String>) -> crate::Error {
    crate::Error::Invalid(msg.into())
}

/// The valid jobs of the group's `schedules.json` (its base snapshot).
fn group_jobs(paths: &Paths, group: &GroupId, home: &Path) -> Vec<Job> {
    crate::engine::read_base_file(paths, group, crate::group::SCHEDULES)
        .ok()
        .flatten()
        .map(|b| crate::jobs::parse_file(group, &b, home).0)
        .unwrap_or_default()
}

fn find<'a>(c: &'a super::Catalog, id: &str) -> crate::Result<&'a Pack> {
    c.pack(id).ok_or_else(|| {
        let why = c
            .problems
            .iter()
            .find(|p| p.id.as_deref() == Some(id))
            .map(|p| format!(" It is skipped: {}", p.message))
            .unwrap_or_default();
        err(format!("this group has no pack {id:?}.{why}"))
    })
}

fn rules_of(paths: &Paths, group: &GroupId, pack: &Pack) -> (Option<String>, usize) {
    let Some(file) = &pack.claude_md else {
        return (None, 0);
    };
    let text = std::fs::read_to_string(pack.folder(paths, group).join(file)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let first = lines.first().map(|l| {
        let t = l.trim().trim_start_matches('#').trim();
        t.chars().take(120).collect::<String>()
    });
    (first, lines.len())
}

pub fn list(paths: &Paths, group: &GroupId, env: &Env) -> crate::Result<PacksOverview> {
    let catalog = load_catalog(paths, group)?;
    let file = state::load(paths)?;
    let records = file.groups.get(&group.to_string());
    let jobs = group_jobs(paths, group, env.home);
    let packs = catalog
        .packs
        .iter()
        .map(|k| {
            let rec = records.and_then(|r| r.get(&k.id));
            let applied = rec.is_some_and(|r| r.state.applied());
            let state = match rec.map(|r| r.state) {
                _ if !k.runs_on(env.os) && !applied => "unavailable",
                None => "available",
                Some(PackState::Tried) => "tried",
                Some(PackState::Ready) => "ready",
                Some(PackState::On) => "on",
                Some(PackState::Failing) => "failing",
                Some(PackState::Off) => "off",
            };
            let (rule_preview, rule_lines) = rules_of(paths, group, k);
            let set = launch::session(paths, group, &k.id);
            PackView {
                id: k.id.clone(),
                title: k.title.clone(),
                summary: k.summary.clone(),
                needs: k.needs.clone(),
                os: k.os.clone(),
                state,
                by: rec.map(|r| r.by),
                since: rec.map(|r| r.since),
                last_check: rec.and_then(|r| r.last_check.clone()),
                plugins: k.plugins.clone(),
                jobs: jobs
                    .iter()
                    .filter(|j| k.jobs.contains(&j.id))
                    .map(PackJob::from)
                    .collect(),
                rule_preview,
                rule_lines,
                has_files: !k.files.is_empty(),
                setting_up: set == launch::Session::Open,
                claude_closed: set == launch::Session::Closed,
                changed: rec.is_some_and(|r| r.state == PackState::On && r.pack_hash != k.hash),
                group_enabled: !k.group_enabled.is_empty(),
            }
        })
        .collect();
    Ok(PacksOverview {
        packs,
        problems: catalog.problems,
        host: catalog.host,
        os: env.os.to_string(),
    })
}

/// What `start_setup` hands the launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupPlan {
    pub folder: PathBuf,
    pub title: String,
    pub pack: String,
}

/// The pack's folder and title for a setup chat. A pack never tried here is `tried` from now
/// on; one that is on, failing or ready keeps its state (Fix with Claude changes nothing).
pub fn start_setup(
    paths: &Paths,
    group: &GroupId,
    id: &str,
    env: &Env,
) -> crate::Result<SetupPlan> {
    let catalog = load_catalog(paths, group)?;
    let pack = find(&catalog, id)?;
    if !pack.runs_on(env.os) {
        return Err(err(format!(
            "{} is not for this computer's system",
            pack.title
        )));
    }
    let folder = pack.folder(paths, group);
    if !folder.join(&pack.setup).is_file() {
        return Err(err(format!(
            "{} has not been copied to this computer yet; sync first",
            pack.title
        )));
    }
    state::update(paths, |f| {
        let records = f.groups.entry(group.to_string()).or_default();
        match records.get_mut(id) {
            Some(r) if r.state != PackState::Off => {}
            Some(r) => {
                r.state = PackState::Tried;
                r.since = env.now_ms;
            }
            None => {
                records.insert(
                    id.to_string(),
                    Record {
                        state: PackState::Tried,
                        by: By::Setup,
                        since: env.now_ms,
                        pack_hash: pack.hash.clone(),
                        last_check: None,
                        job_hashes: BTreeMap::new(),
                    },
                );
            }
        }
        Ok(())
    })?;
    Ok(SetupPlan {
        folder,
        title: pack.title.clone(),
        pack: pack.id.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub passed: bool,
    pub exit: i32,
    pub tail: String,
    /// The state the pack is in now.
    pub state: &'static str,
}

fn state_name(s: PackState) -> &'static str {
    match s {
        PackState::Tried => "tried",
        PackState::Ready => "ready",
        PackState::On => "on",
        PackState::Failing => "failing",
        PackState::Off => "off",
    }
}

/// Run the pack's check and record the result. A pack that is on stays on (it is `failing`
/// while the check fails, so nothing that worked stops); any other pack becomes `ready` or
/// `tried`.
pub fn check_pack(
    paths: &Paths,
    group: &GroupId,
    id: &str,
    env: &Env,
) -> crate::Result<CheckReport> {
    let catalog = load_catalog(paths, group)?;
    let pack = find(&catalog, id)?;
    let applied = state::record(paths, group, id)?.is_some_and(|r| r.state.applied());
    if !pack.runs_on(env.os) && !applied {
        return Err(err(format!(
            "{} is not for this computer's system",
            pack.title
        )));
    }
    let outcome = check::run(&pack.folder(paths, group), &pack.check);
    let new_state = state::update(paths, |f| {
        let records = f.groups.entry(group.to_string()).or_default();
        let rec = records.entry(id.to_string()).or_insert_with(|| Record {
            state: PackState::Tried,
            by: By::Setup,
            since: env.now_ms,
            pack_hash: pack.hash.clone(),
            last_check: None,
            job_hashes: BTreeMap::new(),
        });
        rec.last_check = Some(CheckResult {
            at: env.now_ms,
            exit: outcome.exit,
            tail: outcome.tail.clone(),
            hash: pack.hash.clone(),
        });
        rec.state = match (rec.state.applied(), outcome.passed()) {
            (true, true) => PackState::On,
            (true, false) => PackState::Failing,
            (false, true) => PackState::Ready,
            (false, false) => PackState::Tried,
        };
        if rec.state.applied() {
            // The pack as it is now was looked at; the next change asks again.
            rec.pack_hash = pack.hash.clone();
        }
        Ok(rec.state)
    })?;
    launch::clear(paths, group, id);
    Ok(CheckReport {
        passed: outcome.passed(),
        exit: outcome.exit,
        tail: outcome.tail,
        state: state_name(new_state),
    })
}

/// Re-run the check of packs that are on and changed since this machine accepted them, and,
/// with `tried`, of packs set up before that never passed (once per app start). Packs that
/// are not set up, off or ready never run anything (D8). Returns the ids checked.
pub fn recheck_due(
    paths: &Paths,
    group: &GroupId,
    env: &Env,
    tried: bool,
) -> crate::Result<Vec<String>> {
    let catalog = load_catalog(paths, group)?;
    let file = state::load(paths)?;
    let Some(records) = file.groups.get(&group.to_string()) else {
        return Ok(Vec::new());
    };
    let mut done = Vec::new();
    for pack in &catalog.packs {
        let Some(r) = records.get(&pack.id) else {
            continue;
        };
        let due = match r.state {
            PackState::On => r.pack_hash != pack.hash,
            PackState::Tried => tried && r.last_check.is_some() && pack.runs_on(env.os),
            _ => false,
        };
        // A pack whose setup chat is open is the member's to finish.
        if due && launch::session(paths, group, &pack.id) != launch::Session::Open {
            check_pack(paths, group, &pack.id, env)?;
            done.push(pack.id.clone());
        }
    }
    Ok(done)
}

/// What `turn_on` decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnedOn {
    /// `(job id, consent hash)` the member accepted; the caller accepts them with the jobs.
    pub jobs: Vec<(String, String)>,
    pub plugins_changed: bool,
}

/// Turn a ready pack on for this machine: its check passed against the pack as it is now, and
/// the member saw the jobs they accept (`hashes`). Sets the plugins' local value to `true`
/// (the pack's override) and records the pack as on.
pub fn turn_on(
    paths: &Paths,
    group: &GroupId,
    id: &str,
    hashes: &BTreeMap<String, String>,
    env: &Env,
) -> crate::Result<TurnedOn> {
    let catalog = load_catalog(paths, group)?;
    let pack = find(&catalog, id)?;
    if !pack.runs_on(env.os) {
        return Err(err(format!(
            "{} is not for this computer's system",
            pack.title
        )));
    }
    let rec = state::record(paths, group, id)?;
    let fresh = rec.as_ref().is_some_and(|r| {
        r.state == PackState::Ready
            && r.last_check
                .as_ref()
                .is_some_and(|c| c.passed() && c.hash == pack.hash)
    });
    if !fresh {
        return Err(err(format!(
            "the check of {} has not passed against its current version; run it again first",
            pack.title
        )));
    }
    let jobs = group_jobs(paths, group, env.home);
    let mut accept = Vec::new();
    for job_id in &pack.jobs {
        let job = jobs
            .iter()
            .find(|j| &j.id == job_id)
            .ok_or_else(|| err(format!("job {job_id:?} of {} is not valid", pack.title)))?;
        let shown = hashes.get(job_id).map(String::as_str);
        if shown != Some(job.consent_hash().as_str()) {
            return Err(err(format!(
                "job {job_id:?} changed while you were looking at it; review it again"
            )));
        }
        accept.push((job_id.clone(), job.consent_hash()));
    }
    let mut changed = false;
    for plugin in &pack.plugins {
        changed |= crate::apply::set_local_plugin(paths, plugin, Some(true))?;
    }
    state::update(paths, |f| {
        let r = f
            .groups
            .entry(group.to_string())
            .or_default()
            .get_mut(id)
            .ok_or_else(|| err("the pack has no record"))?;
        r.state = PackState::On;
        r.by = By::Setup;
        r.since = env.now_ms;
        r.pack_hash = pack.hash.clone();
        r.job_hashes = accept.iter().cloned().collect();
        Ok(())
    })?;
    Ok(TurnedOn {
        jobs: accept,
        plugins_changed: changed,
    })
}

/// Turn a pack off here: the override goes (the plugin gets the group's value back), and the
/// state is `off`. Jobs, the rule line and the pack's files follow at the next apply and
/// reconcile. Returns whether a plugin value changed.
pub fn turn_off(paths: &Paths, group: &GroupId, id: &str, env: &Env) -> crate::Result<bool> {
    let catalog = load_catalog(paths, group)?;
    let pack = find(&catalog, id)?;
    let rec = state::record(paths, group, id)?;
    if !rec.as_ref().is_some_and(|r| r.state.applied()) {
        return Err(err(format!("{} is not on", pack.title)));
    }
    // Off first: even if the settings write fails, nothing stays pinned.
    state::update(paths, |f| {
        if let Some(r) = f.groups.entry(group.to_string()).or_default().get_mut(id) {
            r.state = PackState::Off;
            r.by = By::Setup;
            r.since = env.now_ms;
            r.job_hashes.clear();
        }
        Ok(())
    })?;
    let mut changed = false;
    for plugin in &pack.plugins {
        let group_value = super::group_plugin_value(paths, group, plugin);
        changed |= crate::apply::set_local_plugin(paths, plugin, group_value)?;
    }
    Ok(changed)
}
