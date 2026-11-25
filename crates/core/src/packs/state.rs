//! `<state>/packs.json`: what this machine decided for each pack. Never synced: a pack needs
//! an app on that machine, and `choices.json` is per account.
use crate::Paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

pub const FORMAT: u32 = 1;
const FILE: &str = "packs.json";

/// One read-change-write at a time inside a process.
static LOCK: Mutex<()> = Mutex::new(());

/// A pack with no record is "available": never set up here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackState {
    /// Set up was started, or a check failed. Nothing is applied.
    Tried,
    /// The check passed; waiting for the member's Turn on.
    Ready,
    On,
    /// On, but its check fails now (the pack changed). Stays applied.
    Failing,
    /// Turned off by the member, or off on a host machine before packs.
    Off,
}

impl PackState {
    /// The plugin, jobs, rule line and files of the pack are in effect.
    pub fn applied(self) -> bool {
        matches!(self, Self::On | Self::Failing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum By {
    Setup,
    CarryOver,
    HostAuto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub at: u64,
    pub exit: i32,
    /// The last stderr line, redacted, at most 200 characters.
    pub tail: String,
    /// The pack hash the check ran against.
    #[serde(default)]
    pub hash: String,
}

impl CheckResult {
    pub fn passed(&self) -> bool {
        self.exit == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub state: PackState,
    pub by: By,
    /// Unix milliseconds.
    pub since: u64,
    /// The pack as this machine last accepted it (set when it was turned on or re-checked).
    pub pack_hash: String,
    #[serde(default)]
    pub last_check: Option<CheckResult>,
    /// Job id -> consent hash accepted at Turn on.
    #[serde(default)]
    pub job_hashes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacksFile {
    #[serde(default = "format")]
    pub format: u32,
    /// `owner/repo` -> pack id -> record.
    #[serde(default)]
    pub groups: BTreeMap<String, BTreeMap<String, Record>>,
}

fn format() -> u32 {
    FORMAT
}

impl Default for PacksFile {
    fn default() -> Self {
        Self {
            format: FORMAT,
            groups: BTreeMap::new(),
        }
    }
}

pub fn file(paths: &Paths) -> std::path::PathBuf {
    paths.state_dir.join(FILE)
}

/// A missing file is empty. A file from a newer app is an error, so it is never overwritten.
pub fn load(paths: &Paths) -> crate::Result<PacksFile> {
    match std::fs::read(file(paths)) {
        Ok(b) => {
            let f: PacksFile = serde_json::from_slice(&b)?;
            if f.format > FORMAT {
                return Err(crate::Error::Invalid(format!(
                    "packs.json is format {}; update the app",
                    f.format
                )));
            }
            Ok(f)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PacksFile::default()),
        Err(e) => Err(e.into()),
    }
}

/// Written atomically (temp file, then rename).
pub fn save(paths: &Paths, f: &PacksFile) -> crate::Result<()> {
    crate::state::write_json(&file(paths), f)
}

/// Read, change and write `packs.json`.
pub fn update<T>(
    paths: &Paths,
    change: impl FnOnce(&mut PacksFile) -> crate::Result<T>,
) -> crate::Result<T> {
    let _one = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = load(paths)?;
    let out = change(&mut f)?;
    save(paths, &f)?;
    Ok(out)
}

pub fn record(paths: &Paths, group: &crate::GroupId, id: &str) -> crate::Result<Option<Record>> {
    Ok(load(paths)?
        .groups
        .get(&group.to_string())
        .and_then(|g| g.get(id))
        .cloned())
}
