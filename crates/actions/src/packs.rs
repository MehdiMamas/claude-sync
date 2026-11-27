//! Opt-in packs (Parity 79): `packs/<id>/` in the group repo (`pack.json`, `setup.md`, the
//! check). A pack goes on here only after its check passed on this machine against the pack as
//! it is now. What each machine decided is kept in the store and never synced.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{Event, PackView};
use claude_sync_engine::traits::{Remote, Store};
use claude_sync_fs::atomic::sha256_hex;
use claude_sync_fs::redact::redact;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const KEY: &str = "packs";
const OSES: [&str; 3] = ["windows", "macos", "linux"];

fn this_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema: u32,
    id: String,
    title: String,
    summary: String,
    #[serde(default)]
    os: Option<Vec<String>>,
    setup: String,
    check: Check,
    /// `name@marketplace`: each is off in the group's settings and on here only while the pack is.
    #[serde(default)]
    plugins: Vec<String>,
    /// Ids of jobs in the group's `schedules.json`; they register only where the pack is on.
    #[serde(default)]
    jobs: Vec<String>,
    /// Group paths (`skills/<name>`, `agents/<name>.md`) placed only where the pack is on.
    #[serde(default)]
    files: Vec<String>,
    /// A `CLAUDE.md` in the pack folder, imported only where the pack is on.
    #[serde(default)]
    claude_md: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Check {
    run: Vec<String>,
    #[serde(default)]
    timeout_seconds: Option<u32>,
}

struct Pack {
    m: Manifest,
    hash: String,
    files: BTreeMap<String, Vec<u8>>,
}

/// What the sync pass needs of a valid pack, kept in the store (`pack-index/<repo>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    id: String,
    plugins: Vec<String>,
    jobs: Vec<String>,
    files: Vec<String>,
    claude_md: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Rec {
    /// When the last check ran (unix seconds).
    #[serde(default)]
    at: i64,
    state: String,
    /// The pack hash the last passing check ran against.
    hash: String,
    tail: String,
}

type State = BTreeMap<String, BTreeMap<String, Rec>>;

fn valid_id(id: &str) -> bool {
    (1..=31).contains(&id.len())
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

impl Core {
    fn pack_state(&self) -> State {
        Store::read(&self.db, KEY)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_pack(&self, repo: &str, id: &str, f: impl FnOnce(&mut Rec)) -> Result<()> {
        let mut s = self.pack_state();
        f(s.entry(repo.into())
            .or_default()
            .entry(id.into())
            .or_default());
        Ok(Store::write(
            &self.db,
            KEY,
            &serde_json::to_vec(&s).unwrap_or_default(),
        )?)
    }

    /// Valid packs of the group's base, and why each other folder was skipped.
    fn catalog(&self, repo: &str) -> Result<Vec<std::result::Result<Pack, (String, String)>>> {
        // packs are repo-only files, not part of what is delivered: read the repo head
        let remote = self.remote(repo)?;
        let head = remote
            .head(None)?
            .ok_or_else(|| Error::new(crate::msg::not_a_group(repo)))?;
        let all = remote.files(&head.sha, &BTreeMap::new())?;
        let ids: std::collections::BTreeSet<&str> = all
            .keys()
            .filter_map(|p| p.strip_prefix("packs/")?.split('/').next())
            .collect();
        let mut out = vec![];
        for id in ids {
            let pre = format!("packs/{id}/");
            let files: BTreeMap<String, Vec<u8>> = all
                .iter()
                .filter_map(|(p, b)| Some((p.strip_prefix(&pre)?.to_string(), b.clone())))
                .collect();
            out.push(parse(id, files).map_err(|e| (id.to_string(), e)));
        }
        Ok(out)
    }

    fn find_pack(&self, repo: &str, id: &str) -> Result<Pack> {
        for p in self.catalog(repo)? {
            match p {
                Ok(p) if p.m.id == id => return Ok(p),
                Err((i, e)) if i == id => {
                    return Err(Error::new(format!("pack {id} is not valid: {e}")))
                }
                _ => {}
            }
        }
        Err(Error::new(format!("{repo} has no pack {id}")))
    }

    pub(crate) fn packs(&mut self, repo: Option<&str>) -> Result<Event> {
        let repos: Vec<String> = match repo {
            Some(r) => vec![r.into()],
            None => self.db.groups()?.into_iter().map(|g| g.repo).collect(),
        };
        let st = self.pack_state();
        let mut out = vec![];
        for r in repos {
            let Ok(cat) = self.catalog(&r) else { continue };
            for p in cat.into_iter().flatten() {
                let rec = st
                    .get(&r)
                    .and_then(|g| g.get(&p.m.id))
                    .cloned()
                    .unwrap_or_default();
                // a pack that changed since it passed is no longer ready
                let state = match rec.state.as_str() {
                    "ready" | "on" if rec.hash != p.hash && rec.state == "ready" => "off",
                    "" => "off",
                    s => s,
                };
                out.push(PackView {
                    repo: r.clone(),
                    id: p.m.id.clone(),
                    title: p.m.title.clone(),
                    summary: p.m.summary.clone(),
                    state: state.into(),
                    unsupported: !os_ok(&p),
                    os: p
                        .m
                        .os
                        .clone()
                        .unwrap_or_else(|| OSES.map(String::from).to_vec()),
                    checked_at: (rec.at > 0).then_some(rec.at),
                    tail: rec.tail,
                });
            }
        }
        out.sort_by(|a, b| (&a.repo, &a.id).cmp(&(&b.repo, &b.id)));
        Ok(Event::Packs(out))
    }

    pub(crate) fn pack_setup(&mut self, repo: &str, id: &str) -> Result<Event> {
        let p = self.find_pack(repo, id)?;
        let text = p
            .files
            .get("setup.md")
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        Ok(Event::Text(text))
    }

    /// The folder, first message and Claude Code for a setup chat (the folder is the placed
    /// pack, so `setup.md` is there).
    pub fn pack_chat(
        &self,
        repo: &str,
        id: &str,
    ) -> Result<(std::path::PathBuf, String, std::path::PathBuf)> {
        let p = self.find_pack(repo, id)?;
        if !os_ok(&p) {
            return Err(Error::new(format!(
                "{} is not for this computer's system",
                p.m.title
            )));
        }
        let dir = self.pack_dir(repo, id);
        for (rel, bytes) in &p.files {
            let to = dir.join(rel);
            if std::fs::read(&to).ok().as_deref() != Some(bytes.as_slice()) {
                claude_sync_fs::atomic::write_atomic(&to, bytes)?;
            }
        }
        let claude = crate::jobs::find_claude(&self.home.home).ok_or_else(|| {
            Error::new("Claude Code isn't installed or signed in on this machine. Install it, then run this again.")
        })?;
        let name = repo.split('/').nth(1).unwrap_or(repo);
        Ok((dir, crate::launch::message(&p.m.title, name), claude))
    }

    pub(crate) fn pack_launch(&mut self, repo: &str, id: &str) -> Result<Event> {
        let (dir, msg, claude) = self.pack_chat(repo, id)?;
        let stem = format!("{}__{id}", repo.replace('/', "__"));
        let out = crate::launch::start(
            &self.home.state.join("packs-setup"),
            &stem,
            &dir,
            &claude,
            &msg,
            this_os(),
        )?;
        Ok(match out {
            crate::launch::Outcome::Started => Event::Message(format!(
                "Claude Code opened in a terminal. When it closes, run the check of {id}."
            )),
            crate::launch::Outcome::NoTerminal { command } => Event::Text(command),
        })
    }

    /// Runs the check (no shell, in the pack folder, with a timeout) and records the result.
    pub(crate) fn pack_check(&mut self, repo: &str, id: &str) -> Result<Event> {
        let p = self.find_pack(repo, id)?;
        if !os_ok(&p) {
            return Err(Error::new(format!(
                "{} is not for this computer's system",
                p.m.title
            )));
        }
        let dir = self
            .home
            .state
            .join("packs")
            .join(repo.replace('/', "__"))
            .join(id);
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, bytes) in &p.files {
            let to = dir.join(rel);
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&to, bytes)?;
        }
        let now = self.now();
        let (code, tail) = run_check(
            &dir,
            &p.m.check.run,
            p.m.check.timeout_seconds.unwrap_or(60).min(120),
        );
        let passed = code == 0;
        self.save_pack(repo, id, |r| {
            // a pack that is already on stays on; only its record of the check moves
            if !(r.state == "on" && passed) {
                r.state = if passed { "ready" } else { "failed" }.into();
            }
            r.hash = if passed {
                p.hash.clone()
            } else {
                String::new()
            };
            r.tail = tail;
            r.at = now;
        })?;
        self.packs_changed();
        Ok(Event::Done)
    }

    pub(crate) fn pack_on(&mut self, repo: &str, id: &str) -> Result<Event> {
        let p = self.find_pack(repo, id)?;
        if !os_ok(&p) {
            return Err(Error::new(format!(
                "{} is not for this computer's system",
                p.m.title
            )));
        }
        let ok = self
            .pack_state()
            .get(repo)
            .and_then(|g| g.get(id))
            .is_some_and(|r| matches!(r.state.as_str(), "ready" | "on") && r.hash == p.hash);
        if !ok {
            return Err(Error::new(format!(
                "the check of {} has not passed against its current version; run it again first",
                p.m.title
            )));
        }
        self.save_pack(repo, id, |r| r.state = "on".into())?;
        self.packs_changed();
        Ok(Event::Done)
    }

    pub(crate) fn pack_off(&mut self, repo: &str, id: &str) -> Result<Event> {
        let p = self.find_pack(repo, id)?;
        let on = self
            .pack_state()
            .get(repo)
            .and_then(|g| g.get(id))
            .is_some_and(|r| r.state == "on");
        if !on {
            return Err(Error::new(format!("{} is not on", p.m.title)));
        }
        self.save_pack(repo, id, |r| r.state = "off".into())?;
        self.unpin_plugins(repo, id);
        self.packs_changed();
        Ok(Event::Done)
    }
}

/// `/enabledPlugins/<name@marketplace>`, RFC 6901 escaped.
fn pointer(plugin: &str) -> String {
    format!(
        "/enabledPlugins/{}",
        plugin.replace('~', "~0").replace('/', "~1")
    )
}

impl Core {
    fn index(&self, repo: &str) -> Vec<Entry> {
        Store::read(&self.db, &format!("pack-index/{repo}"))
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn is_on(&self, repo: &str, id: &str) -> bool {
        self.pack_state()
            .get(repo)
            .and_then(|g| g.get(id))
            .is_some_and(|r| r.state == "on")
    }

    fn pack_dir(&self, repo: &str, id: &str) -> std::path::PathBuf {
        claude_sync_fs::apply::copy_path(&self.home, repo, &format!("packs/{id}"))
    }

    /// The packs of the head just read: keep the index, place each pack folder (the pack's
    /// `CLAUDE.md` is imported from there) and drop the folders of packs that are gone.
    fn refresh_packs(&self, repo: &str, all: &BTreeMap<String, Vec<u8>>) {
        let ids: std::collections::BTreeSet<&str> = all
            .keys()
            .filter_map(|p| p.strip_prefix("packs/")?.split('/').next())
            .collect();
        let mut index = vec![];
        for id in ids {
            let pre = format!("packs/{id}/");
            let files = all
                .iter()
                .filter_map(|(p, b)| Some((p.strip_prefix(&pre)?.to_string(), b.clone())))
                .collect();
            let Ok(p) = parse(id, files) else { continue };
            let dir = self.pack_dir(repo, id);
            for (rel, bytes) in &p.files {
                let to = dir.join(rel);
                if std::fs::read(&to).ok().as_deref() != Some(bytes.as_slice()) {
                    let _ = claude_sync_fs::atomic::write_atomic(&to, bytes);
                }
            }
            index.push(Entry {
                id: p.m.id,
                plugins: p.m.plugins,
                jobs: p.m.jobs,
                files: p.m.files,
                claude_md: p.m.claude_md,
            });
        }
        let root = self.pack_dir(repo, "");
        for d in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            if !index
                .iter()
                .any(|e| d.file_name().to_string_lossy() == e.id)
            {
                let _ = std::fs::remove_dir_all(d.path());
            }
        }
        let _ = Store::write(
            &self.db,
            &format!("pack-index/{repo}"),
            &serde_json::to_vec(&index).unwrap_or_default(),
        );
    }

    /// The sync pass hook (`GroupIn.keep`): group paths of packs that are not on here. Reads the
    /// pack folders from the head's files when it moved, else from the kept index.
    pub(crate) fn pack_skipped(
        &self,
        repo: &str,
        files: Option<&BTreeMap<String, Vec<u8>>>,
    ) -> std::collections::BTreeSet<String> {
        if let Some(f) = files {
            self.refresh_packs(repo, f);
            self.write_imports();
        }
        self.index(repo)
            .into_iter()
            .filter(|e| !self.is_on(repo, &e.id))
            .flat_map(|e| e.files)
            .collect()
    }

    /// Settings pointers of the plugins of packs that are on here (`Pass.pinned`).
    pub(crate) fn pack_pinned(&self) -> std::collections::BTreeSet<String> {
        let Ok(groups) = self.db.groups() else {
            return Default::default();
        };
        groups
            .iter()
            .flat_map(|g| {
                self.index(&g.repo)
                    .into_iter()
                    .filter(|e| self.is_on(&g.repo, &e.id))
                    .flat_map(|e| e.plugins)
            })
            .map(|p| pointer(&p))
            .collect()
    }

    /// What the job reconcile asks: `None` the job is in no pack; `Some(on)` its pack is on here.
    pub(crate) fn pack_gate(&self, repo: &str, job: &str) -> Option<bool> {
        let e = self
            .index(repo)
            .into_iter()
            .find(|e| e.jobs.iter().any(|j| j == job))?;
        Some(self.is_on(repo, &e.id))
    }

    /// `<group copy>/pack-imports`: the `CLAUDE.md` files of the packs that are on, one path a
    /// line, which `update_block` adds after the group's own import.
    fn write_imports(&self) {
        for g in self.db.groups().unwrap_or_default() {
            let lines: Vec<String> = self
                .index(&g.repo)
                .into_iter()
                .filter(|e| self.is_on(&g.repo, &e.id))
                .filter_map(|e| Some(self.pack_dir(&g.repo, &e.id).join(e.claude_md?)))
                .filter(|f| f.is_file())
                .map(|f| f.to_string_lossy().into_owned())
                .collect();
            let f = claude_sync_fs::apply::copy_path(&self.home, &g.repo, "pack-imports");
            if lines.is_empty() {
                let _ = std::fs::remove_file(&f);
            } else {
                let _ = claude_sync_fs::atomic::write_atomic(&f, lines.join("\n").as_bytes());
            }
        }
    }

    /// A pack went off: its plugins get the group's value back in `settings.json` (the machine's
    /// own `true` was only a pin while the pack was on).
    fn unpin_plugins(&self, repo: &str, id: &str) {
        let Some(e) = self.index(repo).into_iter().find(|e| e.id == id) else {
            return;
        };
        let path = self.home.claude.join("settings.json");
        let group: serde_json::Value = std::fs::read(claude_sync_fs::apply::copy_path(
            &self.home,
            repo,
            "settings.shared.json",
        ))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
        let Some(mut local) = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        else {
            return;
        };
        let mut changed = false;
        for p in &e.plugins {
            let Some(map) = local
                .get_mut("enabledPlugins")
                .and_then(serde_json::Value::as_object_mut)
            else {
                return;
            };
            match group.pointer(&pointer(p)).cloned() {
                Some(v) => changed |= map.insert(p.clone(), v).as_ref() != map.get(p),
                None => changed |= map.remove(p).is_some(),
            }
        }
        if changed {
            let text = serde_json::to_string_pretty(&local).unwrap_or_default() + "\n";
            let _ = claude_sync_fs::atomic::write_atomic(&path, text.as_bytes());
        }
    }

    /// A left group: its pack records, index and imports go.
    pub(crate) fn forget_packs(&self, repo: &str) {
        for e in self.index(repo) {
            if self.is_on(repo, &e.id) {
                self.unpin_plugins(repo, &e.id);
            }
        }
        let mut s = self.pack_state();
        if s.remove(repo).is_some() {
            let _ = Store::write(&self.db, KEY, &serde_json::to_vec(&s).unwrap_or_default());
        }
        let _ = Store::write(&self.db, &format!("pack-index/{repo}"), b"");
    }

    /// A pack was turned on or off, or checked: imports, files, plugins and jobs follow now.
    fn packs_changed(&mut self) {
        self.write_imports();
        let order: Vec<(String, bool)> = self
            .db
            .groups()
            .unwrap_or_default()
            .into_iter()
            .map(|g| (g.repo, g.off))
            .collect();
        let _ = claude_sync_fs::apply::update_block(&self.home, &order, self.now());
        let _ = self.run_pass();
        self.jobs_after_pass();
    }
}

fn plugin_ok(p: &str) -> bool {
    let word = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    p.split_once('@').is_some_and(|(n, m)| word(n) && word(m))
}

fn file_path_ok(p: &str) -> bool {
    match p.split('/').collect::<Vec<_>>().as_slice() {
        ["skills", n] => !n.is_empty() && !n.starts_with('.') && !n.contains(':'),
        ["agents", f] => f.ends_with(".md") && f.len() > 3 && !f.starts_with('.'),
        _ => false,
    }
}

fn os_ok(p: &Pack) -> bool {
    p.m.os
        .as_ref()
        .is_none_or(|l| l.iter().any(|o| o == this_os()))
}

fn parse(folder: &str, files: BTreeMap<String, Vec<u8>>) -> std::result::Result<Pack, String> {
    let raw = files.get("pack.json").ok_or("pack.json is missing")?;
    if raw.len() > 64 * 1024 {
        return Err("pack.json is larger than 64 KiB".into());
    }
    let m: Manifest =
        serde_json::from_slice(raw).map_err(|e| format!("pack.json is not valid: {e}"))?;
    if m.schema != 1 {
        return Err(format!(
            "schema {} is not supported (this app reads schema 1)",
            m.schema
        ));
    }
    if !valid_id(&m.id) || m.id != folder {
        return Err(format!(
            "id {:?} must be lowercase letters, digits and dashes and equal the folder name",
            m.id
        ));
    }
    let (t, s) = (
        m.title.trim().chars().count(),
        m.summary.trim().chars().count(),
    );
    if !(1..=40).contains(&t) {
        return Err("title must be 1 to 40 characters".into());
    }
    if !(1..=200).contains(&s) {
        return Err("summary must be 1 to 200 characters".into());
    }
    if let Some(bad) = m.os.iter().flatten().find(|o| !OSES.contains(&o.as_str())) {
        return Err(format!(
            "os {bad:?} must be one of windows, macos, linux (WSL is linux)"
        ));
    }
    if m.setup != "setup.md"
        || files
            .get("setup.md")
            .is_none_or(|b| b.is_empty() || b.len() > 8 * 1024)
    {
        return Err("setup.md must be named in setup and hold 1 byte to 8 KiB".into());
    }
    if m.check.run.is_empty() || m.check.run.len() > 8 {
        return Err("check.run needs a program and at most 7 arguments".into());
    }
    if let Some(bad) = m.plugins.iter().find(|p| !plugin_ok(p)) {
        return Err(format!("plugin {bad:?} must look like name@marketplace"));
    }
    if let Some(bad) = m.files.iter().find(|f| !file_path_ok(f)) {
        return Err(format!(
            "file {bad:?} must be skills/<name> or agents/<name>.md"
        ));
    }
    if let Some(f) = &m.claude_md {
        if f.contains('/') || f.contains("..") || !files.contains_key(f) {
            return Err(format!("claudeMd {f:?} is not a file of the pack folder"));
        }
    }
    let mut all = vec![];
    for (p, b) in &files {
        all.extend_from_slice(p.as_bytes());
        all.extend_from_slice(sha256_hex(b).as_bytes());
    }
    Ok(Pack {
        hash: sha256_hex(&all),
        m,
        files,
    })
}

fn run_check(dir: &std::path::Path, run: &[String], secs: u32) -> (i32, String) {
    let tail = |t: &str| {
        let l = redact(
            t.lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .unwrap_or(""),
        );
        l.chars().take(200).collect::<String>()
    };
    let mut c = Command::new(&run[0]);
    claude_sync_fs::quiet(&mut c);
    c.args(&run[1..])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = match c.spawn() {
        Ok(c) => c,
        Err(e) => return (-1, tail(&format!("could not start {}: {e}", run[0]))),
    };
    let err = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut e, &mut s);
            s
        })
    });
    let start = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code().unwrap_or(-1),
            Ok(None) if start.elapsed() >= Duration::from_secs(secs.into()) => {
                let _ = child.kill();
                let _ = child.wait();
                return (-1, format!("timed out after {secs} s"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return (-1, tail(&e.to_string())),
        }
    };
    (
        code,
        tail(&err.and_then(|h| h.join().ok()).unwrap_or_default()),
    )
}
