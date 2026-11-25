//! Opt-in packs (§23): extras of a group that turn on for one computer only after the member
//! set them up and the pack's check passed there.
//!
//! - A pack is a folder `packs/<id>/` in the group repo (`pack.json`, `setup.md`, an optional
//!   `CLAUDE.md` and check script). Like `schedules.json` it is repo only: read from the base
//!   snapshot, copied to `<state>/groups/<owner>__<repo>/packs/<id>/`, never into `~/.claude`.
//! - What each machine decided is in `<state>/packs.json` ([`state`]); it is never synced,
//!   because a pack needs an app on that machine.
//! - A pack's plugins are `false` in the group's `settings.shared.json`. While the pack is on,
//!   the machine's own `true` is a local-only override, handled like a mods-canary override
//!   (`settings::canary_pointers`): not a local edit, never pushed, not overwritten.
//! - Packs reach the rest of the app through three small views: [`pinned_pointers`] (settings),
//!   [`skipped_files`] (apply) and [`import_files`] (the `CLAUDE.md` block); jobs ask
//!   [`job_gate`].
//! - A group with no `packs/` folder has no catalog, and every view is empty: nothing changes.
use crate::state::write_atomic;
use crate::{FileSet, GroupId, Paths};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub mod check;
pub mod cli;
pub mod launch;
pub mod ops;
pub mod state;

pub use state::{By, CheckResult, PackState, PacksFile, Record};

pub const MANIFEST: &str = "pack.json";
pub const SCHEMA: u32 = 1;
pub const MAX_PACKS: usize = 20;
/// The setup prompt, at most.
pub const MAX_SETUP: usize = 8 * 1024;
pub const MAX_CHECK_ARGS: usize = 8;
pub const DEFAULT_TIMEOUT: u32 = 60;
pub const MAX_TIMEOUT: u32 = 120;
pub const OSES: [&str; 3] = ["windows", "macos", "linux"];
/// The catalog of the last sync, next to the pack folders.
const INDEX: &str = "packs.index.json";
/// The pack folders of a group on this machine.
const DIR: &str = "packs";

/// `^[a-z0-9][a-z0-9-]{0,30}$`
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 31
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// This machine's OS in the words of `pack.json` (`os`); WSL is `linux`.
pub fn this_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Need {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckSpec {
    /// The program and its arguments, run with no shell in the pack folder.
    pub run: Vec<String>,
    pub timeout_seconds: u32,
}

/// One valid pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub needs: Vec<Need>,
    pub os: Vec<String>,
    /// `name@marketplace`, each `false` in the group's `settings.shared.json`.
    pub plugins: Vec<String>,
    /// Ids of jobs in the group's `schedules.json`.
    pub jobs: Vec<String>,
    /// Group paths: `skills/<name>` or `agents/<name>.md`.
    pub files: Vec<String>,
    /// `CLAUDE.md` in the pack folder: imported only where the pack is on.
    pub claude_md: Option<String>,
    pub setup: String,
    pub check: CheckSpec,
    pub host_carry_over: bool,
    /// Shows first when lower; then by id.
    pub order: i32,
    /// Plugins the group still turns on for everyone (the move to packs is not finished):
    /// they stay on whatever the pack's state is.
    #[serde(default)]
    pub group_enabled: Vec<String>,
    /// Change detection: the folder, the plugins' settings and the placed files.
    pub hash: String,
}

impl Pack {
    pub fn runs_on(&self, os: &str) -> bool {
        self.os.iter().any(|o| o == os)
    }
    /// The folder of this pack on this machine.
    pub fn folder(&self, paths: &Paths, group: &GroupId) -> PathBuf {
        folder(paths, group, &self.id)
    }
}

/// A pack that was skipped, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    /// The folder name.
    pub id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub packs: Vec<Pack>,
    pub problems: Vec<Problem>,
    /// The signed-in login owns the group repo (as of the last sync).
    #[serde(default)]
    pub host: bool,
}

impl Catalog {
    pub fn pack(&self, id: &str) -> Option<&Pack> {
        self.packs.iter().find(|p| p.id == id)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPack {
    schema: u32,
    id: String,
    title: String,
    summary: String,
    #[serde(default)]
    needs: Vec<RawNeed>,
    #[serde(default)]
    os: Option<Vec<String>>,
    #[serde(default)]
    plugins: Vec<String>,
    #[serde(default)]
    jobs: Vec<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    claude_md: Option<String>,
    setup: String,
    check: RawCheck,
    #[serde(default = "yes")]
    host_carry_over: bool,
    #[serde(default)]
    order: i32,
}

#[derive(Deserialize)]
struct RawNeed {
    name: String,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCheck {
    run: Vec<String>,
    #[serde(default)]
    timeout_seconds: Option<u32>,
}

fn yes() -> bool {
    true
}

// ---------------------------------------------------------------- paths

/// `<state>/groups/<owner>__<repo>/packs`
pub fn packs_dir(paths: &Paths, group: &GroupId) -> PathBuf {
    paths.group_dir(group).join(DIR)
}

pub fn folder(paths: &Paths, group: &GroupId, id: &str) -> PathBuf {
    packs_dir(paths, group).join(id)
}

fn index_file(paths: &Paths, group: &GroupId) -> PathBuf {
    paths.group_dir(group).join(INDEX)
}

// ---------------------------------------------------------------- parse and validate

fn plugin_ok(p: &str) -> bool {
    let word = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    p.split_once('@')
        .is_some_and(|(name, market)| word(name) && word(market))
}

/// `skills/<name>` or `agents/<name>.md`.
fn file_path_ok(p: &str) -> bool {
    match p.split('/').collect::<Vec<_>>().as_slice() {
        ["skills", name] => !name.is_empty() && !name.starts_with('.') && !name.contains(':'),
        ["agents", file] => file.ends_with(".md") && file.len() > 3 && !file.starts_with('.'),
        _ => false,
    }
}

/// The repo files a group path of `files` stands for.
fn files_of<'a>(all: &'a FileSet, path: &str) -> Vec<(&'a String, &'a Vec<u8>)> {
    let dir = format!("{path}/");
    all.iter()
        .filter(|(p, _)| p.as_str() == path || p.starts_with(&dir))
        .collect()
}

fn too_long(s: &str, max: usize) -> bool {
    s.chars().count() > max
}

/// Everything `validate` needs from the group besides the pack folder.
struct Group<'a> {
    files: &'a FileSet,
    /// `enabledPlugins` of `settings.shared.json`.
    plugins: BTreeMap<String, Value>,
    marketplaces: Option<&'a Value>,
    jobs: BTreeSet<String>,
}

fn validate(g: &Group, folder_id: &str) -> Result<Pack, String> {
    let manifest = format!("packs/{folder_id}/{MANIFEST}");
    let bytes = g
        .files
        .get(&manifest)
        .ok_or_else(|| format!("{MANIFEST} is missing"))?;
    if bytes.len() > 64 * 1024 {
        return Err(format!("{MANIFEST} is larger than 64 KiB"));
    }
    let raw: RawPack =
        serde_json::from_slice(bytes).map_err(|e| format!("{MANIFEST} is not valid: {e}"))?;
    if raw.schema != SCHEMA {
        return Err(format!(
            "schema {} is not supported (this app reads schema {SCHEMA})",
            raw.schema
        ));
    }
    if !valid_id(&raw.id) {
        return Err(format!(
            "id {:?} must be lowercase letters, digits and dashes (at most 31, starting with a letter or digit)",
            raw.id
        ));
    }
    if raw.id != folder_id {
        return Err(format!(
            "id {:?} must equal the folder name {folder_id:?}",
            raw.id
        ));
    }
    let title = raw.title.trim().to_string();
    if title.is_empty() || too_long(&title, 40) {
        return Err("title must be 1 to 40 characters".into());
    }
    let summary = raw.summary.trim().to_string();
    if summary.is_empty() || too_long(&summary, 200) {
        return Err("summary must be 1 to 200 characters".into());
    }
    let mut needs = Vec::new();
    for n in raw.needs {
        let name = n.name.trim().to_string();
        if name.is_empty() || too_long(&name, 80) {
            return Err("a need's name must be 1 to 80 characters".into());
        }
        if let Some(u) = &n.url {
            if !u.starts_with("https://") || u.contains(char::is_whitespace) {
                return Err(format!("need {name:?}: url must start with https://"));
            }
        }
        needs.push(Need { name, url: n.url });
    }
    let os = raw
        .os
        .unwrap_or_else(|| OSES.iter().map(|s| s.to_string()).collect());
    if os.is_empty() {
        return Err("os is empty; list windows, macos and/or linux".into());
    }
    if let Some(bad) = os.iter().find(|o| !OSES.contains(&o.as_str())) {
        return Err(format!(
            "os {bad:?} must be one of {} (WSL is linux)",
            OSES.join(", ")
        ));
    }
    let mut group_enabled = Vec::new();
    for p in &raw.plugins {
        if !plugin_ok(p) {
            return Err(format!("plugin {p:?} must look like name@marketplace"));
        }
        match g.plugins.get(p) {
            None | Some(Value::Bool(false)) => {}
            // The group has not turned it off yet: the pack is listed, the plugin stays on.
            Some(Value::Bool(true)) => group_enabled.push(p.clone()),
            Some(_) => {
                return Err(format!(
                    "plugin {p} is not a true or false value in settings.shared.json"
                ))
            }
        }
    }
    if let Some(dup) = raw
        .plugins
        .iter()
        .enumerate()
        .find(|(i, p)| raw.plugins[..*i].contains(p))
    {
        return Err(format!("plugin {} is listed twice", dup.1));
    }
    for j in &raw.jobs {
        if !g.jobs.contains(j) {
            return Err(format!("job {j:?} is not a valid job of schedules.json"));
        }
    }
    for f in &raw.files {
        if !file_path_ok(f) {
            return Err(format!(
                "file {f:?} must be skills/<name> or agents/<name>.md"
            ));
        }
        if files_of(g.files, f).is_empty() {
            return Err(format!("file {f:?} is not in the group"));
        }
    }
    let claude_md = match raw.claude_md {
        None => None,
        Some(c) if c == "CLAUDE.md" => {
            if !g
                .files
                .contains_key(&format!("packs/{folder_id}/CLAUDE.md"))
            {
                return Err("claudeMd names CLAUDE.md, which is not in the pack folder".into());
            }
            Some(c)
        }
        Some(c) => return Err(format!("claudeMd must be \"CLAUDE.md\", not {c:?}")),
    };
    if raw.setup != "setup.md" {
        return Err(format!("setup must be \"setup.md\", not {:?}", raw.setup));
    }
    let setup = g
        .files
        .get(&format!("packs/{folder_id}/setup.md"))
        .ok_or("setup.md is not in the pack folder")?;
    if setup.is_empty() {
        return Err("setup.md is empty".into());
    }
    if setup.len() > MAX_SETUP {
        return Err(format!(
            "setup.md is {} bytes; at most {MAX_SETUP}",
            setup.len()
        ));
    }
    let run = raw.check.run;
    if run.is_empty() || run.len() > MAX_CHECK_ARGS {
        return Err(format!(
            "check.run needs 1 to {MAX_CHECK_ARGS} items, not {}",
            run.len()
        ));
    }
    if run.iter().any(|a| a.is_empty() || a.contains('\0')) {
        return Err("check.run has an empty item or a NUL character".into());
    }
    let timeout_seconds = match raw.check.timeout_seconds {
        None => DEFAULT_TIMEOUT,
        Some(t) if (1..=MAX_TIMEOUT).contains(&t) => t,
        Some(t) => {
            return Err(format!(
                "check.timeoutSeconds {t} must be from 1 to {MAX_TIMEOUT}"
            ))
        }
    };
    let mut pack = Pack {
        id: raw.id,
        title,
        summary,
        needs,
        os,
        plugins: raw.plugins,
        jobs: raw.jobs,
        files: raw.files,
        claude_md,
        setup: raw.setup,
        check: CheckSpec {
            run,
            timeout_seconds,
        },
        host_carry_over: raw.host_carry_over,
        order: raw.order,
        group_enabled,
        hash: String::new(),
    };
    pack.hash = hash_of(g, &pack);
    Ok(pack)
}

/// The pack folder, the plugins' marketplace entries and the group files the pack places.
fn hash_of(g: &Group, pack: &Pack) -> String {
    let mut parts: Vec<(String, String)> = Vec::new();
    let prefix = format!("packs/{}/", pack.id);
    for (p, b) in g.files.iter().filter(|(p, _)| p.starts_with(&prefix)) {
        parts.push((p.clone(), crate::apply::sha256_hex(b)));
    }
    for f in &pack.files {
        for (p, b) in files_of(g.files, f) {
            parts.push((p.clone(), crate::apply::sha256_hex(b)));
        }
    }
    for plugin in &pack.plugins {
        parts.push((format!("plugin:{plugin}"), String::new()));
        if let Some((_, market)) = plugin.split_once('@') {
            let entry = g.marketplaces.and_then(|m| m.get(market));
            parts.push((
                format!("marketplace:{market}"),
                entry.map(Value::to_string).unwrap_or_default(),
            ));
        }
    }
    parts.sort();
    crate::apply::sha256_hex(&serde_json::to_vec(&parts).expect("serializes"))
}

/// The catalog of a group repo's files (the base snapshot or a fresh pull).
pub fn parse_catalog(group: &GroupId, files: &FileSet, home: &Path) -> Catalog {
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    for p in files.keys() {
        if let Some(rest) = p.strip_prefix("packs/") {
            if let Some((id, _)) = rest.split_once('/') {
                ids.insert(id);
            }
        }
    }
    let shared: Value = files
        .get(crate::group::SHARED_SETTINGS)
        .and_then(|b| crate::settings::parse_object(b))
        .unwrap_or(Value::Null);
    let plugins = shared
        .get("enabledPlugins")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let jobs = files
        .get(crate::group::SCHEDULES)
        .map(|b| {
            crate::jobs::parse_file(group, b, home)
                .0
                .into_iter()
                .map(|j| j.id)
                .collect()
        })
        .unwrap_or_default();
    let g = Group {
        files,
        plugins,
        marketplaces: shared.get("extraKnownMarketplaces"),
        jobs,
    };
    let mut catalog = Catalog::default();
    // What a pack uses, to keep one owner for each.
    let mut plugin_owner: BTreeMap<String, String> = BTreeMap::new();
    let mut job_owner: BTreeMap<String, String> = BTreeMap::new();
    let mut file_owner: BTreeMap<String, String> = BTreeMap::new();
    let mut valid: Vec<Pack> = Vec::new();
    for id in ids {
        let problem = |message: String| Problem {
            id: Some(id.to_string()),
            message,
        };
        if !valid_id(id) {
            catalog.problems.push(problem(format!(
                "the folder name {id:?} must be lowercase letters, digits and dashes"
            )));
            continue;
        }
        let pack = match validate(&g, id) {
            Ok(p) => p,
            Err(e) => {
                catalog.problems.push(problem(e));
                continue;
            }
        };
        let clash = pack
            .plugins
            .iter()
            .find_map(|p| plugin_owner.get(p).map(|o| ("plugin", p, o)))
            .or_else(|| {
                pack.jobs
                    .iter()
                    .find_map(|j| job_owner.get(j).map(|o| ("job", j, o)))
            })
            .or_else(|| {
                pack.files
                    .iter()
                    .find_map(|f| file_owner.get(f).map(|o| ("file", f, o)))
            });
        if let Some((kind, what, owner)) = clash {
            catalog.problems.push(problem(format!(
                "the {kind} {what} already belongs to the pack {owner}; it can be in one pack only"
            )));
            continue;
        }
        if valid.len() >= MAX_PACKS {
            catalog.problems.push(problem(format!(
                "more than {MAX_PACKS} packs; the rest are skipped"
            )));
            continue;
        }
        for p in &pack.plugins {
            plugin_owner.insert(p.clone(), pack.id.clone());
        }
        for j in &pack.jobs {
            job_owner.insert(j.clone(), pack.id.clone());
        }
        for f in &pack.files {
            file_owner.insert(f.clone(), pack.id.clone());
        }
        valid.push(pack);
    }
    valid.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
    catalog.packs = valid;
    catalog
}

// ---------------------------------------------------------------- catalog on disk

/// The catalog of the last sync. A group without packs has none (empty).
pub fn load_catalog(paths: &Paths, group: &GroupId) -> crate::Result<Catalog> {
    match std::fs::read(index_file(paths, group)) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Catalog::default()),
        Err(e) => Err(e.into()),
    }
}

fn save_catalog(paths: &Paths, group: &GroupId, c: &Catalog) -> crate::Result<()> {
    crate::state::write_json(&index_file(paths, group), c)
}

/// Copy each valid pack's folder under `<state>/groups/<g>/packs/<id>/` and drop what the
/// group no longer has. Files are written only when they differ.
fn place(paths: &Paths, group: &GroupId, files: &FileSet, catalog: &Catalog) -> crate::Result<()> {
    let root = packs_dir(paths, group);
    let mut want: BTreeMap<PathBuf, &Vec<u8>> = BTreeMap::new();
    for pack in &catalog.packs {
        let prefix = format!("packs/{}/", pack.id);
        for (p, b) in files.iter().filter(|(p, _)| p.starts_with(&prefix)) {
            want.insert(root.join(&p["packs/".len()..]), b);
        }
    }
    for (target, bytes) in &want {
        if std::fs::read(target).ok().as_deref() != Some(bytes.as_slice()) {
            write_atomic(target, bytes)?;
        }
    }
    // Stale files, then empty folders and the folders of packs that are gone.
    if root.is_dir() {
        let mut stack = vec![root.clone()];
        let mut dirs = Vec::new();
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)?.flatten() {
                let p = e.path();
                match e.file_type() {
                    Ok(t) if t.is_dir() => {
                        dirs.push(p.clone());
                        stack.push(p);
                    }
                    Ok(_) if !want.contains_key(&p) => {
                        let _ = std::fs::remove_file(&p);
                    }
                    _ => {}
                }
            }
        }
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for d in dirs {
            // Fails while it holds files: that is the point.
            let _ = std::fs::remove_dir(&d);
        }
    }
    Ok(())
}

/// What `prepare` needs besides the files.
#[derive(Debug, Clone)]
pub struct Prepare<'a> {
    pub home: &'a Path,
    /// `Some`: this sync learned whether the signed-in login owns the repo. `None`: reuse
    /// what the last sync learned.
    pub host: Option<bool>,
    pub now_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrepareReport {
    /// Packs whose state this call decided (host carry-over).
    pub carried: Vec<String>,
}

/// Before the apply of a sync: read the catalog from `files`, place the pack folders and, on a
/// machine signed in as the repo owner, decide what each pack without a record is by what is
/// in use now (host carry-over). Carry-over reads `settings.json` and the job choices as they
/// are, so it must run before the apply merges the group's new values. A group that never had
/// packs is left alone.
pub fn prepare(
    paths: &Paths,
    group: &GroupId,
    files: &FileSet,
    p: &Prepare,
) -> crate::Result<PrepareReport> {
    let had = index_file(paths, group).is_file();
    if !files.keys().any(|f| crate::group::is_pack_file(f)) && !had {
        return Ok(PrepareReport::default());
    }
    let mut catalog = parse_catalog(group, files, p.home);
    let before = load_catalog(paths, group)?;
    catalog.host = p.host.unwrap_or(before.host);
    place(paths, group, files, &catalog)?;
    if catalog != before {
        if catalog.packs.is_empty() && catalog.problems.is_empty() {
            let _ = std::fs::remove_file(index_file(paths, group));
        } else {
            save_catalog(paths, group, &catalog)?;
        }
    }
    let mut report = PrepareReport::default();
    if catalog.host {
        report.carried = carry_over(paths, group, &catalog, &before, p)?;
    }
    Ok(report)
}

/// Host carry-over (D6): a pack with no record that is in use here is on, one that is not is
/// off and stays off. No check runs, so nothing that works today can stop.
fn carry_over(
    paths: &Paths,
    group: &GroupId,
    catalog: &Catalog,
    before: &Catalog,
    p: &Prepare,
) -> crate::Result<Vec<String>> {
    let mut file = state::load(paths)?;
    let records = file.groups.entry(group.to_string()).or_default();
    let todo: Vec<&Pack> = catalog
        .packs
        .iter()
        .filter(|k| k.host_carry_over && !records.contains_key(&k.id))
        .collect();
    if todo.is_empty() {
        return Ok(Vec::new());
    }
    let local = std::fs::read(paths.claude_home.join("settings.json"))
        .ok()
        .and_then(|b| crate::settings::parse_object(&b))
        .unwrap_or(Value::Null);
    let choices = crate::choices::load(paths)?;
    let mine = choices.group(group);
    // The first sync that sees packs is the switch; a pack that arrives later is automatic.
    let by = if before.packs.is_empty() && before.problems.is_empty() {
        By::CarryOver
    } else {
        By::HostAuto
    };
    let mut carried = Vec::new();
    for k in todo {
        let plugin_on = k
            .plugins
            .iter()
            .any(|pl| local.pointer(&plugin_pointer(pl)) == Some(&Value::Bool(true)));
        let job_on = k
            .jobs
            .iter()
            .any(|j| mine.accepted_jobs.contains_key(j) && !mine.off_jobs.contains(j));
        let in_use = if k.plugins.is_empty() && k.jobs.is_empty() {
            true
        } else {
            plugin_on || job_on
        };
        let job_hashes = k
            .jobs
            .iter()
            .filter_map(|j| mine.accepted_jobs.get(j).map(|h| (j.clone(), h.clone())))
            .collect();
        records.insert(
            k.id.clone(),
            Record {
                state: if in_use {
                    PackState::On
                } else {
                    PackState::Off
                },
                by,
                since: p.now_ms,
                pack_hash: k.hash.clone(),
                last_check: None,
                job_hashes,
            },
        );
        carried.push(k.id.clone());
    }
    state::save(paths, &file)?;
    Ok(carried)
}

// ---------------------------------------------------------------- views for the rest of the app

/// `/enabledPlugins/<name@marketplace>` with RFC 6901 escaping.
pub fn plugin_pointer(plugin: &str) -> String {
    crate::settings::join("/enabledPlugins", plugin)
}

/// The packs of every joined group that are applied here: state on or failing, in the catalog.
fn applied<'a>(
    paths: &Paths,
    catalogs: &'a [(GroupId, Catalog)],
) -> crate::Result<Vec<(&'a GroupId, &'a Pack)>> {
    let file = state::load(paths)?;
    let mut out = Vec::new();
    for (g, c) in catalogs {
        let Some(records) = file.groups.get(&g.to_string()) else {
            continue;
        };
        for k in &c.packs {
            if records.get(&k.id).is_some_and(|r| r.state.applied()) {
                out.push((g, k));
            }
        }
    }
    Ok(out)
}

fn catalogs(paths: &Paths) -> crate::Result<Vec<(GroupId, Catalog)>> {
    let mut out = Vec::new();
    for g in crate::state::AppState::load(paths)?.groups {
        let c = load_catalog(paths, &g)?;
        if !c.packs.is_empty() {
            out.push((g, c));
        }
    }
    Ok(out)
}

/// Settings pointers of the plugins of packs that are on here. The apply keeps the machine's
/// own value there, the watcher does not call it an edit, and personal sync does not carry it.
pub fn pinned_pointers(paths: &Paths) -> BTreeSet<String> {
    let go = || -> crate::Result<BTreeSet<String>> {
        let cs = catalogs(paths)?;
        Ok(applied(paths, &cs)?
            .into_iter()
            .flat_map(|(_, k)| k.plugins.iter().map(|p| plugin_pointer(p)))
            .collect())
    };
    go().unwrap_or_default()
}

/// Group paths the apply must not place for `group`: the `files` of packs that are not on.
pub fn skipped_files(paths: &Paths, group: &GroupId) -> BTreeSet<String> {
    let go = || -> crate::Result<BTreeSet<String>> {
        let c = load_catalog(paths, group)?;
        if c.packs.is_empty() {
            return Ok(BTreeSet::new());
        }
        let file = state::load(paths)?;
        let records = file.groups.get(&group.to_string());
        Ok(c.packs
            .iter()
            .filter(|k| {
                !records
                    .and_then(|r| r.get(&k.id))
                    .is_some_and(|r| r.state.applied())
            })
            .flat_map(|k| k.files.iter().cloned())
            .collect())
    };
    go().unwrap_or_default()
}

/// Whether `rel` (a group path) is one of the files of a pack that is not on: the apply
/// skips the path itself and everything under it.
pub fn is_skipped(skipped: &BTreeSet<String>, rel: &str) -> bool {
    skipped.iter().any(|s| {
        rel == s
            || rel
                .strip_prefix(s.as_str())
                .is_some_and(|r| r.starts_with('/'))
    })
}

/// The `CLAUDE.md` copies of the packs that are on in `group`, in pack order: one import
/// line each in the managed block.
pub fn import_files(paths: &Paths, group: &GroupId) -> Vec<PathBuf> {
    let go = || -> crate::Result<Vec<PathBuf>> {
        let c = load_catalog(paths, group)?;
        if c.packs.is_empty() {
            return Ok(Vec::new());
        }
        let file = state::load(paths)?;
        let records = file.groups.get(&group.to_string());
        Ok(c.packs
            .iter()
            .filter(|k| {
                records
                    .and_then(|r| r.get(&k.id))
                    .is_some_and(|r| r.state.applied())
            })
            .filter_map(|k| k.claude_md.as_ref().map(|f| k.folder(paths, group).join(f)))
            .filter(|p| p.is_file())
            .collect())
    };
    go().unwrap_or_default()
}

/// What the job reconcile asks about a job of `group`: `None`: the job is in no pack, so
/// nothing changes for it. `Some(on)`: its pack is on here (or not).
pub fn job_gate(paths: &Paths, group: &GroupId, job: &str) -> Option<bool> {
    let c = load_catalog(paths, group).ok()?;
    let pack = c.packs.iter().find(|k| k.jobs.iter().any(|j| j == job))?;
    let on = state::load(paths)
        .ok()
        .and_then(|f| f.groups.get(&group.to_string())?.get(&pack.id).cloned())
        .is_some_and(|r| r.state.applied());
    Some(on)
}

/// The pack that owns `job`, if any.
pub fn pack_of_job(paths: &Paths, group: &GroupId, job: &str) -> Option<String> {
    let c = load_catalog(paths, group).ok()?;
    c.packs
        .iter()
        .find(|k| k.jobs.iter().any(|j| j == job))
        .map(|k| k.id.clone())
}

/// The value the group's `settings.shared.json` (as last applied here) gives `plugin`.
pub fn group_plugin_value(paths: &Paths, group: &GroupId, plugin: &str) -> Option<bool> {
    std::fs::read(paths.group_dir(group).join(crate::group::SHARED_SETTINGS))
        .ok()
        .and_then(|b| crate::settings::parse_object(&b))?
        .pointer(&plugin_pointer(plugin))
        .and_then(Value::as_bool)
}

/// A left group: the plugins of its packs get the group's value back (so removing the group's
/// settings takes them away like the rest), and its records go. The pack folders go with the
/// group's folder.
pub fn forget(paths: &Paths, group: &GroupId) -> crate::Result<()> {
    let c = load_catalog(paths, group)?;
    let file = state::load(paths)?;
    if let Some(records) = file.groups.get(&group.to_string()) {
        for k in c
            .packs
            .iter()
            .filter(|k| records.get(&k.id).is_some_and(|r| r.state.applied()))
        {
            for plugin in &k.plugins {
                let _ = crate::apply::set_local_plugin(
                    paths,
                    plugin,
                    group_plugin_value(paths, group, plugin),
                );
            }
        }
    }
    state::update(paths, |f| {
        f.groups.remove(&group.to_string());
        Ok(())
    })
}

#[cfg(test)]
pub(crate) mod tests;
