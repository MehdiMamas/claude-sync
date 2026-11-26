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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Rec {
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
        })?;
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
        Ok(Event::Done)
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
