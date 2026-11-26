//! The real filesystem behind the engine's `Disk`: where each repo path lives, link checks
//! on every read and write, atomic writes. A group repo and the personal repo each get a
//! `FsDisk` over the same injectable home folder (tests never touch the real `~`).
//!
//! Group paths: `plugins/<mp>/..` -> `<state>/marketplaces/<mp>/..`; `settings.shared.json`
//! -> `<state>/groups/<owner>__<repo>/settings.shared.json`; the other managed folders
//! -> `~/.claude/..`. Personal paths: `CLAUDE.md` (the user's own text only), `settings.json`
//! (shareable part), `memory/<key>/..` -> `~/.claude/projects/<folder>/memory/..`,
//! `state/..` -> `~/.claude/personal-state/..`, `machines/<id>.json` -> `<state>/machines/..`.
//!
//! v2-Parity rows covered: 25, 37, 43.
use crate::atomic::{blocked, read_file, write_atomic};
use claude_sync_engine::claude_md;
use claude_sync_engine::paths::{is_memory_key, is_personal_path, is_planned_path, is_skipped_key};
use claude_sync_engine::plan::FileSet;
use claude_sync_engine::settings::machine_specific;
use claude_sync_engine::traits::{Disk, IoError, IoResult};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The folders claude-sync works in.
#[derive(Debug, Clone)]
pub struct Home {
    pub home: PathBuf,
    pub claude: PathBuf,
    pub state: PathBuf,
    pub ssh: PathBuf,
}

impl Home {
    pub fn under(home: &Path) -> Self {
        Self::with(home, None, None)
    }

    /// `under`, then v1's overrides: `CLAUDE_HOME` for `.claude`, `CLAUDE_SYNC_HOME` for the
    /// state folder (empty counts as unset).
    pub fn from_env(home: &Path) -> Self {
        let var = |k| std::env::var_os(k).filter(|v| !v.is_empty());
        Self::with(home, var("CLAUDE_HOME"), var("CLAUDE_SYNC_HOME"))
    }

    pub fn with(
        home: &Path,
        claude: Option<std::ffi::OsString>,
        state: Option<std::ffi::OsString>,
    ) -> Self {
        Home {
            home: home.into(),
            claude: claude.map_or_else(|| home.join(".claude"), PathBuf::from),
            state: state.map_or_else(|| home.join(".claude-sync"), PathBuf::from),
            ssh: home.join(".ssh"),
        }
    }
    pub fn lock_file(&self) -> PathBuf {
        self.state.join("sync.lock")
    }
}

#[derive(Debug, Clone)]
pub enum Scope {
    /// `owner/repo`.
    Group(String),
    Personal,
}

impl Scope {
    /// The folder-safe name of the repo (`owner__repo`, `personal`).
    pub fn dir(&self) -> String {
        match self {
            Scope::Group(r) => r.replace('/', "__"),
            Scope::Personal => "personal".into(),
        }
    }
}

pub struct FsDisk {
    pub home: Home,
    pub scope: Scope,
}

fn io(e: std::io::Error) -> IoError {
    IoError(e.to_string())
}

/// Claude Code's folder name for a path: `-` for everything but ASCII letters and digits.
pub fn encode_folder(path: &str) -> String {
    path.encode_utf16()
        .map(|u| match char::from_u32(u32::from(u)) {
            Some(c) if c.is_ascii_alphanumeric() => c,
            _ => '-',
        })
        .collect()
}

/// Skipped when walking for new files in a marketplace.
fn walk_skip(name: &str) -> bool {
    matches!(name, "node_modules" | ".git" | ".env" | "COMMIT")
        || name.contains(".claude-sync-tmp-")
}

impl FsDisk {
    pub fn new(home: &Home, scope: Scope) -> Self {
        FsDisk {
            home: home.clone(),
            scope,
        }
    }

    fn enc_home(&self) -> String {
        encode_folder(&self.home.home.to_string_lossy())
    }

    /// The `~/.claude/projects` folder holding a key's memory on this device.
    fn memory_folder(&self, key: &str) -> PathBuf {
        let projects = self.home.claude.join("projects");
        let enc = self.enc_home();
        let rest = &key["home".len()..];
        let mut names: Vec<String> = std::fs::read_dir(&projects)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| {
                n.len() == enc.len() + rest.len()
                    && n.is_ascii()
                    && n[..enc.len()].eq_ignore_ascii_case(&enc)
                    && &n[enc.len()..] == rest
            })
            .collect();
        // the folder with a memory/ folder first, else the one spelled like the home folder
        names.sort_by_key(|n| {
            (
                !projects.join(n).join("memory").is_dir(),
                !n.starts_with(&enc),
            )
        });
        match names.first() {
            Some(n) => projects.join(n),
            None => projects.join(format!("{enc}{rest}")),
        }
    }

    /// `(managed root, file)` for a repo path, `None` when the path is not kept on disk.
    pub fn locate(&self, rel: &str) -> Option<(PathBuf, PathBuf)> {
        let h = &self.home;
        match &self.scope {
            Scope::Group(_) => {
                if rel == "settings.shared.json" {
                    let root = h.state.join("groups").join(self.scope.dir());
                    return Some((root.clone(), root.join(rel)));
                }
                if !is_planned_path(rel) {
                    return None;
                }
                match rel.strip_prefix("plugins/") {
                    Some(rest) => {
                        let root = h.state.join("marketplaces");
                        Some((root.clone(), root.join(rest)))
                    }
                    None => Some((h.claude.clone(), h.claude.join(rel))),
                }
            }
            Scope::Personal => {
                if !is_personal_path(rel) {
                    return None;
                }
                let (first, rest) = rel.split_once('/').unwrap_or((rel, ""));
                match first {
                    "CLAUDE.md" | "settings.json" => Some((h.claude.clone(), h.claude.join(rel))),
                    "memory" => {
                        let (key, tail) = rest.split_once('/')?;
                        if !is_memory_key(key) || is_skipped_key(key) {
                            return None;
                        }
                        let root = self.memory_folder(key).join("memory");
                        Some((root.clone(), root.join(tail)))
                    }
                    "state" => {
                        let root = h.claude.join("personal-state");
                        Some((root.clone(), root.join(rest)))
                    }
                    "machines" => {
                        let root = h.state.join("machines");
                        Some((root.clone(), root.join(rest)))
                    }
                    _ => None, // devices.json, README.md, the marker: repo only
                }
            }
        }
    }

    fn checked(&self, rel: &str) -> IoResult<(PathBuf, PathBuf)> {
        let (root, path) = self
            .locate(rel)
            .ok_or_else(|| IoError(format!("'{rel}' is not a managed path")))?;
        if blocked(&root, &path) {
            return Err(IoError(format!(
                "'{rel}' is behind a link or a file in the way"
            )));
        }
        Ok((root, path))
    }

    /// The personal settings.json as shared: what only works on this machine is left out.
    fn settings_view(real: &[u8]) -> Option<Vec<u8>> {
        let v: Value = serde_json::from_slice(real).ok().filter(Value::is_object)?;
        let mut out = serde_json::to_vec_pretty(&machine_specific(&v)).ok()?;
        out.push(b'\n');
        Some(out)
    }

    /// Every repo path with a file on disk that this disk may sync: all of the personal
    /// folders; for a group, only the new files of marketplaces in `known` (the group's other
    /// files are read by name).
    pub fn scan(&self, known: &BTreeSet<String>) -> IoResult<FileSet> {
        let mut paths: BTreeSet<String> = known.clone();
        match self.scope {
            Scope::Group(_) => {
                let mps: BTreeSet<&str> = known
                    .iter()
                    .filter_map(|p| p.strip_prefix("plugins/")?.split('/').next())
                    .collect();
                for mp in mps {
                    let root = self.home.state.join("marketplaces").join(mp);
                    walk(&root, &format!("plugins/{mp}"), &mut paths);
                }
            }
            Scope::Personal => {
                for rel in ["CLAUDE.md", "settings.json"] {
                    paths.insert(rel.into());
                }
                walk(
                    &self.home.claude.join("personal-state"),
                    "state",
                    &mut paths,
                );
                walk(&self.home.state.join("machines"), "machines", &mut paths);
                let projects = self.home.claude.join("projects");
                let enc = self.enc_home();
                for e in std::fs::read_dir(&projects).into_iter().flatten().flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let Some(rest) = name
                        .is_ascii()
                        .then(|| name.get(enc.len()..))
                        .flatten()
                        .filter(|r| {
                            name[..enc.len()].eq_ignore_ascii_case(&enc)
                                && (r.is_empty() || r.starts_with('-'))
                        })
                    else {
                        continue;
                    };
                    let key = format!("home{rest}");
                    if is_memory_key(&key) && !is_skipped_key(&key) {
                        walk(
                            &e.path().join("memory"),
                            &format!("memory/{key}"),
                            &mut paths,
                        );
                    }
                }
            }
        }
        let mut out = FileSet::new();
        for p in paths {
            if self.locate(&p).is_none() {
                continue;
            }
            if let Some(b) = Disk::read(self, &p)? {
                out.insert(p, b);
            }
        }
        Ok(out)
    }
}

/// Repo paths of the plain files under `dir` (not through links), named from `prefix`.
fn walk(dir: &Path, prefix: &str, out: &mut BTreeSet<String>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if walk_skip(&name) {
            continue;
        }
        let rel = format!("{prefix}/{name}");
        match e.file_type() {
            Ok(t) if t.is_file() => {
                out.insert(rel);
            }
            Ok(t) if t.is_dir() => walk(&e.path(), &rel, out),
            _ => {}
        }
    }
}

/// Repo paths of links found under `dir`.
fn walk_links(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    if std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
        out.push(prefix.to_string()); // the managed folder itself is a link
        return;
    }
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let rel = format!("{prefix}/{}", e.file_name().to_string_lossy());
        match e.file_type() {
            Ok(t) if t.is_symlink() => out.push(rel),
            Ok(t) if t.is_dir() => walk_links(&e.path(), &rel, out),
            _ => {}
        }
    }
}

impl Disk for FsDisk {
    fn read(&self, rel: &str) -> IoResult<Option<Vec<u8>>> {
        let (root, path) = self
            .locate(rel)
            .ok_or_else(|| IoError(format!("'{rel}' is not a managed path")))?;
        if blocked(&root, &path) {
            return Ok(None); // a link in the way reads as absent; links() lists it
        }
        let bytes = read_file(&path).map_err(io)?;
        Ok(match (&self.scope, rel, bytes) {
            (Scope::Personal, "CLAUDE.md", Some(b)) => claude_md::strip(&b),
            (Scope::Personal, "settings.json", Some(b)) => Self::settings_view(&b),
            (_, _, b) => b,
        })
    }

    fn write(&self, rel: &str, bytes: &[u8]) -> IoResult<()> {
        let (_, path) = self.checked(rel)?;
        let bytes: Vec<u8> = match (&self.scope, rel) {
            (Scope::Personal, "CLAUDE.md") => {
                let have = read_file(&path).map_err(io)?;
                match claude_md::compose(have.as_deref(), Some(bytes)) {
                    Some(b) => b,
                    None => return self.delete(rel),
                }
            }
            (Scope::Personal, "settings.json") => {
                let have = read_file(&path).map_err(io)?;
                merge_personal_settings(have.as_deref(), bytes)?
            }
            _ => bytes.to_vec(),
        };
        write_atomic(&path, &bytes).map_err(io)
    }

    fn delete(&self, rel: &str) -> IoResult<()> {
        let (root, path) = self.checked(rel)?;
        if rel == "CLAUDE.md" && matches!(self.scope, Scope::Personal) {
            // only the user's own text goes; the managed block stays
            if let Some(have) = read_file(&path).map_err(io)? {
                return match claude_md::compose(Some(&have), None) {
                    Some(b) => write_atomic(&path, &b).map_err(io),
                    None => std::fs::remove_file(&path).map_err(io),
                };
            }
            return Ok(());
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io(e)),
        }
        // empty folders this file leaves behind go, never the managed root itself
        let mut dir = path.parent().map(Path::to_path_buf);
        while let Some(d) = dir.filter(|d| d != &root && d.starts_with(&root)) {
            if std::fs::remove_dir(&d).is_err() {
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
        Ok(())
    }

    fn links(&self) -> IoResult<Vec<String>> {
        let h = &self.home;
        let mut out = vec![];
        match self.scope {
            Scope::Group(_) => {
                for d in ["skills", "agents", "commands", "output-styles"] {
                    walk_links(&h.claude.join(d), d, &mut out);
                }
                walk_links(&h.state.join("marketplaces"), "plugins", &mut out);
            }
            Scope::Personal => {
                walk_links(&h.claude.join("personal-state"), "state", &mut out);
            }
        }
        Ok(out)
    }
}

/// Synced shareable settings laid over the real file: keys that only work on this machine
/// stay as they are. shortcut: top-level keys, group-owned values inside them are re-set by
/// the group pass that follows every personal pass.
fn merge_personal_settings(have: Option<&[u8]>, synced: &[u8]) -> IoResult<Vec<u8>> {
    let new: Value = serde_json::from_slice(synced)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| {
            IoError("settings.json from the personal repo is not one JSON object".into())
        })?;
    let real: Value = match have {
        Some(b) => serde_json::from_slice(b)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| IoError(crate::NOT_OBJECT_LOCAL.into()))?,
        None => Value::Object(Default::default()),
    };
    let shareable = machine_specific(&real);
    let mut out = new;
    if let (Some(o), Some(r)) = (out.as_object_mut(), real.as_object()) {
        for (k, v) in r {
            if shareable.get(k) != Some(v) && !o.contains_key(k) {
                o.insert(k.clone(), v.clone()); // machine-only key
            }
        }
    }
    let mut b = serde_json::to_vec_pretty(&out).map_err(|e| IoError(e.to_string()))?;
    b.push(b'\n');
    Ok(b)
}
