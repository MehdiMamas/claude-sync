//! Personal memory (docs/architecture.md §16): one person's own devices share their `~/.claude/CLAUDE.md` (the
//! part outside the managed block) and Claude Code's auto-memory through a private repo
//! `<login>/claude-sync-personal`. It never touches a group repo, and groups never see it.
//!
//! Repo layout (format 1):
//! - `claude-sync-personal.json`: `{"format": 1, "newDevicesJoin": true}`
//! - `devices.json`: `{"devices": [{"id", "name", "os", "lastSeenUnix"}]}`
//! - `CLAUDE.md`: the user's own text of `~/.claude/CLAUDE.md`
//! - `memory/<key>/**`: `~/.claude/projects/<folder>/memory/**`, keyed by `keys::key_for`
//! - `state/**`: `~/.claude/personal-state/**`, small state that mods save (see `state`)
//! - `settings.json`: your own `~/.claude/settings.json` without what groups set and what
//!   cannot work on another machine (see `settings_file`)
//!
//! Local state lives in `<state_dir>/personal/` (`state.json`, `base/`).
pub(crate) mod claude_md;
pub mod keys;
mod lines;
mod settings_file;
mod state;
#[cfg(test)]
mod tests;

use crate::apply::{blocked_by_user, safe_segment, sha256_hex};
use crate::engine::{
    is_lost_access, join_rel, load_base_dir, only, remove_dir_if_exists, save_base_dir, summarize,
    with_changes, Base, ConflictInfo, MAX_CONFLICTS, PUSH_RETRIES,
};
use crate::merge::{self, MergeResult};
use crate::remote::{Changes, Head, RemoteRepo};
use crate::state::{now_unix, utc_stamp, write_atomic, write_json, AppState};
use crate::{Engine, Error, FileSet, GroupId};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const REPO: &str = "claude-sync-personal";
pub const FILE: &str = "claude-sync-personal.json";
pub const DEVICES: &str = "devices.json";
pub const README: &str = "README.md";
pub const CLAUDE_MD: &str = "CLAUDE.md";
pub const MEMORY: &str = "memory";
pub const FORMAT: u32 = 1;
/// The folder under `~/.claude` that `state/**` maps to.
pub const LOCAL_STATE_DIR: &str = state::LOCAL_DIR;
/// Larger memory files stay on their device.
pub const MAX_FILE: usize = 256 * 1024;
/// All synced files together.
pub const MAX_TOTAL: usize = 16 * 1024 * 1024;
/// Shown once when a new device of the same account turns personal memory on by itself.
pub const JOINED_NOTICE: &str =
    "Personal memory from your other devices is on. Turn off in Settings.";
/// Shown once when the account's other devices share personal memory but new ones don't join.
pub const OFFER_NOTICE: &str =
    "Your other devices share personal memory. Open claude-sync to turn it on here too.";
/// This device's `lastSeenUnix` in `devices.json` is refreshed at most this often.
const SEEN_EVERY_SECS: u64 = 24 * 3600;

/// Paths that may be in the personal repo; everything else in it is ignored.
pub fn is_personal_path(path: &str) -> bool {
    let segs: Vec<&str> = path.split('/').collect();
    match segs.as_slice() {
        [FILE | DEVICES | README | CLAUDE_MD | settings_file::PATH] => true,
        [MEMORY, key, rest @ ..] => {
            !rest.is_empty()
                && keys::is_key(key)
                && rest.iter().all(|s| safe_segment(s) && !s.starts_with('.'))
        }
        [state::DIR, rest @ ..] => {
            !rest.is_empty() && rest.iter().all(|s| safe_segment(s) && !s.starts_with('.'))
        }
        _ => false,
    }
}

/// What syncs. All are on when personal memory is turned on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Kinds {
    /// `~/.claude/CLAUDE.md` outside the managed block.
    pub claude_md: bool,
    /// `~/.claude/projects/*/memory/**`.
    pub memory: bool,
    /// `~/.claude/personal-state/**`.
    pub state: bool,
    /// `~/.claude/settings.json`, without group values and machine-specific values.
    pub settings: bool,
}

impl Default for Kinds {
    fn default() -> Self {
        Self {
            claude_md: true,
            memory: true,
            state: true,
            settings: true,
        }
    }
}

impl Kinds {
    fn syncs(&self, path: &str) -> bool {
        Kind::ALL.into_iter().any(|k| k.covers(path) && k.on(self))
    }
}

/// `claude-sync-personal.json`. Unknown keys are kept.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalFile {
    pub format: u32,
    #[serde(default = "yes")]
    pub new_devices_join: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn yes() -> bool {
    true
}

impl PersonalFile {
    pub fn new() -> Self {
        Self {
            format: FORMAT,
            new_devices_join: true,
            extra: Map::new(),
        }
    }
    pub fn parse(bytes: &[u8]) -> crate::Result<Self> {
        let pf: Self = serde_json::from_slice(bytes)?;
        if pf.format > FORMAT {
            return Err(Error::Invalid(format!(
                "{FILE} has format {}, newer than this app; update the app",
                pf.format
            )));
        }
        Ok(pf)
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = serde_json::to_vec_pretty(self).expect("serializable");
        b.push(b'\n');
        b
    }
}

impl Default for PersonalFile {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub os: String,
    #[serde(default)]
    pub last_seen_unix: u64,
}

/// `devices.json`. Unknown keys are kept.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DevicesFile {
    #[serde(default)]
    devices: Vec<Device>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl DevicesFile {
    fn parse(bytes: Option<&Vec<u8>>) -> Self {
        bytes
            .and_then(|b| serde_json::from_slice(b).ok())
            .unwrap_or_default()
    }
    fn to_bytes(&self) -> Vec<u8> {
        let mut b = serde_json::to_vec_pretty(self).expect("serializable");
        b.push(b'\n');
        b
    }
}

/// `<state_dir>/personal/state.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct State {
    /// The account this state belongs to; another account starts over.
    login: Option<String>,
    enabled: bool,
    /// The user turned it on or off, or said "not now": the new-device check stays quiet.
    decided: bool,
    kinds: Kinds,
    device_id: String,
    last_sync_unix: Option<u64>,
    last_error: Option<String>,
    conflicts: Vec<ConflictInfo>,
    skipped: Vec<String>,
    devices: Vec<Device>,
    new_devices_join: Option<bool>,
    /// When it was last turned on here (Unix ms): a "personal sync is off" choice older than
    /// this is not about this turning on.
    #[serde(default)]
    enabled_at_ms: u64,
    /// Hash of the local files after the last sync: unchanged files and remote skip a pass.
    fingerprint: String,
    /// When this device's entry in `devices.json` was last pushed.
    seen_pushed_unix: u64,
}

/// What Settings shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalStatus {
    pub enabled: bool,
    pub decided: bool,
    /// The account's other devices share personal memory and this one was not asked yet.
    pub offer: bool,
    pub kinds: Kinds,
    /// `owner/repo`.
    pub repo: String,
    pub repo_url: String,
    pub device_id: String,
    pub devices: Vec<DeviceStatus>,
    /// `None` until the repo was read once.
    pub new_devices_join: Option<bool>,
    pub last_sync_unix: Option<u64>,
    pub last_error: Option<String>,
    pub conflicts: Vec<ConflictInfo>,
    /// Files left out of the last sync and why.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    pub id: String,
    pub name: String,
    pub os: String,
    pub last_seen_unix: u64,
    pub this_device: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalSyncReport {
    pub pulled: bool,
    pub pushed: bool,
    /// Repo paths written on this device.
    pub written: Vec<String>,
    pub conflicts: Vec<ConflictInfo>,
    pub error: Option<String>,
}

/// Outcome of the check a device runs after sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NewDevice {
    /// No personal repo, or the user already decided on this device.
    Nothing,
    /// The repo says new devices join: personal memory is now on (show `JOINED_NOTICE`).
    Joined,
    /// The repo exists but new devices don't join by themselves: offer it.
    Offer,
}

/// With a git-clone `personal-state`, `state/**` outside `state/claude-sync/` is not synced.
fn state_skipped(path: &str) -> bool {
    path.starts_with("state/") && !path.starts_with("state/claude-sync/")
}

/// The files on this device, and what is held back.
#[derive(Debug, Default)]
struct LocalScan {
    files: FileSet,
    /// Paths never written or pushed this pass.
    held: BTreeSet<String>,
    /// Keys whose folder is a link: everything under them is held.
    held_keys: BTreeSet<String>,
    /// `~/.claude/personal-state` is a link: everything under `state/` is held.
    held_state: bool,
    notes: Vec<String>,
    /// Key -> the folder under `projects/` it uses on this device.
    folders: BTreeMap<String, String>,
    /// What groups set in `settings.json`, when that kind is on.
    settings_skip: Option<settings_file::Skip>,
}

impl LocalScan {
    fn is_held(&self, path: &str) -> bool {
        self.held.contains(path)
            || (self.held_state && path.starts_with("state/"))
            || path
                .strip_prefix("memory/")
                .and_then(|r| r.split('/').next())
                .is_some_and(|k| self.held_keys.contains(k))
    }
}

impl Engine {
    /// Settings' view, from local state only.
    pub fn personal_status(&self) -> PersonalStatus {
        self.status_of(&self.load_personal())
    }

    /// Turn personal memory on: create `<login>/claude-sync-personal` when it is missing,
    /// then sync. The first sync merges with an empty base, so nothing local is lost:
    /// files only one side has are kept, `MEMORY.md` becomes the union, and a file both
    /// sides have with different content is a conflict (remote wins, local copy saved).
    pub async fn personal_enable(&self) -> crate::Result<PersonalStatus> {
        let _busy = self.busy.lock().await;
        self.enable_locked().await
    }

    /// Stop syncing, on every device: the choice is saved with the others and goes out in a
    /// last sync, and the other devices turn it off at their next one. New devices don't
    /// join by themselves any more. Local files and the repo stay as they are.
    pub async fn personal_disable(&self) -> crate::Result<PersonalStatus> {
        let _busy = self.busy.lock().await;
        let mut st = self.load_personal();
        if st.enabled {
            crate::choices::set_personal_off(&self.paths, true)?;
            st.fingerprint.clear();
            let remote = self.remotes.open(&self.personal_id());
            let report = self.sync_locked(remote.as_ref(), None, &mut st).await;
            if report.error.is_none() {
                let _ = self
                    .edit_repo_file(FILE, "stop new devices joining", |bytes| {
                        let mut pf = match bytes {
                            Some(b) => PersonalFile::parse(b)?,
                            None => return Err(not_personal(&self.personal_id())),
                        };
                        pf.new_devices_join = false;
                        Ok(pf.to_bytes())
                    })
                    .await;
                st.new_devices_join = Some(false);
            }
        }
        st.enabled = false;
        st.decided = true;
        self.save_personal(&st)?;
        Ok(self.status_of(&st))
    }

    /// Choose what syncs. Takes effect on the next sync.
    pub async fn personal_set_kinds(&self, kinds: Kinds) -> crate::Result<PersonalStatus> {
        let _busy = self.busy.lock().await;
        let mut st = self.load_personal();
        // A kind turned off keeps what this device last synced of it, so turning it back on
        // merges against that instead of mistaking changes made meanwhile for conflicts.
        let base = load_base_dir(&self.personal_dir().join("base"))?;
        for kind in Kind::ALL {
            let dir = self.personal_dir().join(kind.off_dir());
            if kind.on(&st.kinds) && !kind.on(&kinds) && !dir.join("head.json").is_file() {
                let files = only(&base.files, |p| kind.covers(p));
                save_base_dir(
                    &dir,
                    &Base {
                        head: base.head.clone(),
                        files,
                    },
                )?;
            }
        }
        st.kinds = kinds;
        st.fingerprint.clear();
        self.save_personal(&st)?;
        Ok(self.status_of(&st))
    }

    /// Whether this person's next new device turns personal memory on by itself.
    pub async fn personal_set_new_devices_join(&self, join: bool) -> crate::Result<PersonalStatus> {
        let _busy = self.busy.lock().await;
        self.edit_repo_file(FILE, "set new devices join", |bytes| {
            let mut pf = match bytes {
                Some(b) => PersonalFile::parse(b)?,
                None => return Err(not_personal(&self.personal_id())),
            };
            pf.new_devices_join = join;
            Ok(pf.to_bytes())
        })
        .await?;
        let mut st = self.load_personal();
        st.new_devices_join = Some(join);
        self.save_personal(&st)?;
        Ok(self.status_of(&st))
    }

    /// Take an old device off the list. This device can't remove itself (turn it off).
    pub async fn personal_remove_device(&self, device_id: &str) -> crate::Result<PersonalStatus> {
        let _busy = self.busy.lock().await;
        let mut st = self.load_personal();
        if device_id == st.device_id {
            return Err(Error::Invalid(
                "this device can't remove itself; turn personal memory off instead".into(),
            ));
        }
        let mut devices = Vec::new();
        self.edit_repo_file(DEVICES, "remove a device", |bytes| {
            let mut df = DevicesFile::parse(bytes.map(|b| b.to_vec()).as_ref());
            df.devices.retain(|d| d.id != device_id);
            devices = df.devices.clone();
            Ok(df.to_bytes())
        })
        .await?;
        st.devices = devices;
        self.save_personal(&st)?;
        Ok(self.status_of(&st))
    }

    /// Run once after sign-in. On a device that never decided, a personal repo of this
    /// account turns personal memory on (`Joined`) or is offered (`Offer`), as the repo's
    /// `newDevicesJoin` says.
    pub async fn personal_check_new_device(&self) -> crate::Result<NewDevice> {
        let _busy = self.busy.lock().await;
        let mut st = self.load_personal();
        if st.decided || st.enabled {
            return Ok(NewDevice::Nothing);
        }
        let remote = self.remotes.open(&self.personal_id());
        let head = match remote.head().await {
            Err(Error::GitHub { status: 404, .. }) => return Ok(NewDevice::Nothing),
            r => r?,
        };
        if head.is_empty() {
            return Ok(NewDevice::Nothing);
        }
        let files = remote.files_matching(&head.commit, |p| p == FILE).await?;
        let Some(pf) = files
            .get(FILE)
            .map(|b| PersonalFile::parse(b))
            .transpose()?
        else {
            return Ok(NewDevice::Nothing);
        };
        if !remote.is_private().await? {
            return Ok(NewDevice::Nothing);
        }
        if !pf.new_devices_join {
            // Kept so Settings shows the offer until the user answers.
            st.new_devices_join = Some(false);
            self.save_personal(&st)?;
            return Ok(NewDevice::Offer);
        }
        self.enable_locked().await?;
        Ok(NewDevice::Joined)
    }

    /// "Not now" on the offer: don't ask again on this device.
    pub async fn personal_dismiss_offer(&self) -> crate::Result<()> {
        let _busy = self.busy.lock().await;
        let mut st = self.load_personal();
        st.decided = true;
        self.save_personal(&st)
    }

    /// One pass: pull, merge, write, push. `None` when personal memory is off (or paused,
    /// or nothing changed, with `only_if_changed`). Errors go in the report and the status.
    pub async fn personal_sync(
        &self,
        only_if_changed: bool,
    ) -> crate::Result<Option<PersonalSyncReport>> {
        let _busy = self.busy.lock().await;
        let mut st = self.load_personal();
        if !st.enabled {
            return Ok(None);
        }
        let remote = self.remotes.open(&self.personal_id());
        let mut head = None;
        if only_if_changed {
            if AppState::load(&self.paths)?.paused {
                return Ok(None);
            }
            match remote.head_if_changed().await {
                Ok(Some(h)) => head = Some(h),
                Ok(None) => {
                    let quiet = st.last_error.is_none()
                        && now_unix().saturating_sub(st.seen_pushed_unix) < SEEN_EVERY_SECS
                        // A rename goes out on the next pass.
                        && st
                            .devices
                            .iter()
                            .any(|d| d.id == st.device_id && d.name == self.device)
                        && self
                            .scan_local(&st.kinds)
                            .is_ok_and(|s| fingerprint(&s.files) == st.fingerprint);
                    if quiet {
                        return Ok(None);
                    }
                }
                Err(_) => {}
            }
        }
        Ok(Some(self.sync_locked(remote.as_ref(), head, &mut st).await))
    }

    async fn enable_locked(&self) -> crate::Result<PersonalStatus> {
        let id = self.personal_id();
        let mut remote = self.remotes.open(&id);
        let mut head = match remote.head().await {
            Err(Error::GitHub { status: 404, .. }) => {
                self.remotes.create_personal_repo(REPO).await?;
                remote = self.remotes.open(&id);
                remote.head().await?
            }
            r => r?,
        };
        if !remote.is_private().await? {
            return Err(Error::PublicRepo(id.to_string()));
        }
        let mut st = self.load_personal();
        if head.is_empty() {
            let devices = DevicesFile {
                devices: vec![self.this_device(&st)],
                extra: Map::new(),
            };
            let changes = Changes::from([
                (FILE.to_string(), Some(PersonalFile::new().to_bytes())),
                (README.to_string(), Some(readme(&self.login).into_bytes())),
                (DEVICES.to_string(), Some(devices.to_bytes())),
            ]);
            head = remote
                .commit(
                    &head,
                    &changes,
                    &self.personal_message("start personal memory"),
                )
                .await?;
            st.seen_pushed_unix = now_unix();
        } else {
            let files = remote
                .files_matching(&head.commit, |p| {
                    p == FILE
                        || p == "README.md"
                        || p.starts_with("owners/")
                        || p.starts_with("projects/")
                })
                .await?;
            match files.get(FILE) {
                Some(f) => {
                    PersonalFile::parse(f)?;
                }
                None => return Err(clash(&id, &files)),
            }
        }
        // Start from an empty base, like the first time: nothing that changed while it was
        // off on this device is mistaken for a deletion.
        self.clear_bases()?;
        st.enabled = true;
        // An "off" from before this moment is not about this turning on.
        st.enabled_at_ms = crate::choices::now_ms();
        st.decided = true;
        st.fingerprint.clear();
        // An error from before turning it on is old news; a failing sync below writes its own.
        st.last_error = None;
        self.save_personal(&st)?;
        let report = self.sync_locked(remote.as_ref(), Some(head), &mut st).await;
        if let Some(e) = report.error {
            tracing::warn!(error = %e, "first personal sync failed");
        }
        Ok(self.status_of(&st))
    }

    async fn sync_locked(
        &self,
        remote: &dyn RemoteRepo,
        head: Option<Head>,
        st: &mut State,
    ) -> PersonalSyncReport {
        let mut report = PersonalSyncReport::default();
        let result = async {
            // Someone made it public: nothing more goes in, or comes out.
            if !remote.is_private().await? {
                return Err(Error::PublicRepo(self.personal_id().to_string()));
            }
            let head = match head {
                Some(h) => h,
                None => remote.head().await?,
            };
            self.personal_pull_push(remote, head, st, &mut report).await
        }
        .await;
        if self.state_is_git_clone() {
            // Entries recorded before these paths stopped syncing are no longer true.
            st.conflicts.retain(|c| !state_skipped(&c.path));
        }
        st.conflicts.extend(report.conflicts.iter().cloned());
        let excess = st.conflicts.len().saturating_sub(MAX_CONFLICTS);
        st.conflicts.drain(..excess);
        match result {
            Ok(()) => {
                st.last_sync_unix = Some(now_unix());
                st.last_error = None;
                // Choices made on other devices take effect here.
                match crate::choices::load(&self.paths) {
                    Ok(c) if c.personal.off && c.personal.ts > st.enabled_at_ms => {
                        st.enabled = false;
                        st.decided = true;
                    }
                    Ok(c) => {
                        if c.personal.off {
                            // Older than turning it on here: no longer true.
                            let _ = crate::choices::set_personal_off(&self.paths, false);
                        }
                        self.apply_group_choices().await;
                    }
                    Err(_) => {}
                }
            }
            Err(e) => {
                let e = if is_lost_access(&e) {
                    format!(
                        "can't reach {}: {e}; turn personal memory off and on to start it again",
                        self.personal_id()
                    )
                } else {
                    e.to_string()
                };
                tracing::warn!(error = %e, "personal sync failed");
                st.last_error = Some(e.clone());
                report.error = Some(e);
            }
        }
        if let Err(e) = self.save_personal(st) {
            tracing::warn!(error = %e, "could not save the personal state");
        }
        report
    }

    async fn personal_pull_push(
        &self,
        remote: &dyn RemoteRepo,
        mut head: Head,
        st: &mut State,
        report: &mut PersonalSyncReport,
    ) -> crate::Result<()> {
        let base_dir = self.personal_dir().join("base");
        let mut base = load_base_dir(&base_dir)?;
        let kinds = st.kinds;
        let git_state = self.state_is_git_clone();
        let keep = |p: &str| kinds.syncs(p) && !(git_state && state_skipped(p));
        let mut attempt = 0;
        let notes = loop {
            if head.is_empty() {
                return Err(not_personal(&self.personal_id()));
            }
            let remote_files = if head == base.head {
                base.files.clone()
            } else {
                report.pulled = true;
                remote
                    .files_matching(&head.commit, is_personal_path)
                    .await?
            };
            PersonalFile::parse(
                remote_files
                    .get(FILE)
                    .ok_or_else(|| not_personal(&self.personal_id()))?,
            )
            .map(|pf| st.new_devices_join = Some(pf.new_devices_join))?;

            let mut base_v = only(&base.files, keep);
            let mut resumed = Vec::new();
            for kind in Kind::ALL {
                let dir = self.personal_dir().join(kind.off_dir());
                if kind.on(&kinds) && dir.join("head.json").is_file() {
                    base_v.retain(|p, _| !kind.covers(p));
                    base_v.extend(load_base_dir(&dir)?.files);
                    resumed.push(dir);
                }
            }
            let remote_v = only(&remote_files, keep);
            let mut scan = self.scan_local(&kinds)?;
            self.hold_secrets(&mut scan, &base_v);
            let mut local = scan.files.clone();
            if let Some(skip) = &scan.settings_skip {
                settings_file::keep_skipped(&mut local, &base_v, skip);
            }
            let held: Vec<String> = base_v
                .keys()
                .chain(local.keys())
                .filter(|p| scan.is_held(p))
                .cloned()
                .collect();
            for p in held {
                match base_v.get(&p) {
                    Some(b) => local.insert(p, b.clone()),
                    None => local.remove(&p),
                };
            }

            let merged = merge_personal(&base_v, &local, &remote_v);
            report
                .conflicts
                .extend(self.save_personal_conflicts(&merged.conflicts)?);
            let written = self.write_local(&local, &merged.merged, &mut scan)?;
            report.written.extend(written);

            // The new base is the remote, minus held files this device never had: they
            // merge as new once they are no longer held.
            let files: FileSet = remote_files
                .iter()
                .filter(|(p, _)| !scan.is_held(p) || base_v.contains_key(*p))
                .map(|(p, b)| (p.clone(), b.clone()))
                .collect();
            base = Base {
                head: head.clone(),
                files,
            };
            save_base_dir(&base_dir, &base)?;
            for dir in resumed.drain(..) {
                remove_dir_if_exists(&dir)?;
            }
            st.devices = DevicesFile::parse(remote_files.get(DEVICES)).devices;

            let mut push: Changes = merged
                .to_push
                .into_iter()
                .filter(|(p, _)| !scan.is_held(p))
                .collect();
            let devices = self.device_update(&remote_files, st);
            if let Some(d) = &devices {
                push.insert(DEVICES.to_string(), Some(d.to_bytes()));
            }
            if push.is_empty() {
                break scan.notes;
            }
            self.check_secrets(&push)?;
            let message = self.personal_message(&summarize(&push));
            match remote.commit(&head, &push, &message).await {
                Ok(new) => {
                    base = Base {
                        head: new,
                        files: with_changes(base.files, &push),
                    };
                    save_base_dir(&base_dir, &base)?;
                    if let Some(d) = devices {
                        st.devices = d.devices;
                        st.seen_pushed_unix = now_unix();
                    }
                    report.pushed = true;
                    break scan.notes;
                }
                Err(Error::PushRace) if attempt < PUSH_RETRIES => {
                    attempt += 1;
                    head = remote.head().await?;
                }
                Err(e) => return Err(e),
            }
        };
        st.skipped = notes;
        st.fingerprint = fingerprint(&self.scan_local(&kinds)?.files);
        Ok(())
    }

    /// This device's entry when `devices.json` needs it: missing, renamed, or not seen for a day.
    fn device_update(&self, remote_files: &FileSet, st: &State) -> Option<DevicesFile> {
        let mut df = DevicesFile::parse(remote_files.get(DEVICES));
        let me = self.this_device(st);
        match df.devices.iter_mut().find(|d| d.id == me.id) {
            Some(d)
                if d.name == me.name
                    && d.os == me.os
                    && me.last_seen_unix.saturating_sub(d.last_seen_unix) < SEEN_EVERY_SECS =>
            {
                None
            }
            Some(d) => {
                *d = me;
                Some(df)
            }
            None => {
                df.devices.push(me);
                Some(df)
            }
        }
    }

    /// Change one repo file with `edit` and commit it, retrying when someone pushed first.
    async fn edit_repo_file(
        &self,
        path: &str,
        summary: &str,
        mut edit: impl FnMut(Option<&[u8]>) -> crate::Result<Vec<u8>>,
    ) -> crate::Result<()> {
        let id = self.personal_id();
        let remote = self.remotes.open(&id);
        let mut attempt = 0;
        loop {
            let head = remote.head().await?;
            if head.is_empty() {
                return Err(not_personal(&id));
            }
            let files = remote
                .files_matching(&head.commit, is_personal_path)
                .await?;
            if !files.contains_key(FILE) {
                return Err(not_personal(&id));
            }
            let old = files.get(path);
            let new = edit(old.map(Vec::as_slice))?;
            if old == Some(&new) {
                return Ok(());
            }
            let changes = Changes::from([(path.to_string(), Some(new))]);
            match remote
                .commit(&head, &changes, &self.personal_message(summary))
                .await
            {
                Ok(_) => return Ok(()),
                Err(Error::PushRace) if attempt < PUSH_RETRIES => attempt += 1,
                Err(e) => return Err(e),
            }
        }
    }

    /// `CLAUDE.md` (stripped) and memory files, as repo paths, with what is held back.
    fn scan_local(&self, kinds: &Kinds) -> crate::Result<LocalScan> {
        let mut out = LocalScan::default();
        if kinds.claude_md {
            let file = self.paths.claude_home.join(CLAUDE_MD);
            match std::fs::symlink_metadata(&file) {
                Ok(md) if !md.is_file() => {
                    out.held.insert(CLAUDE_MD.into());
                    out.notes
                        .push(format!("{CLAUDE_MD}: not synced, it is a link or a folder"));
                }
                Ok(md) if md.len() as usize > MAX_FILE => {
                    out.held.insert(CLAUDE_MD.into());
                    out.notes.push(too_large(CLAUDE_MD));
                }
                Ok(_) => {
                    if let Some(text) = claude_md::strip(&std::fs::read(&file)?) {
                        out.files.insert(CLAUDE_MD.into(), text);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        if kinds.state {
            self.scan_state(&mut out)?;
        }
        if kinds.settings {
            self.scan_settings(&mut out)?;
        }
        if !kinds.memory {
            return Ok(out);
        }
        let mut total: usize = out.files.values().map(Vec::len).sum();
        let home_enc = self.home_enc()?;
        let projects = self.paths.claude_home.join("projects");
        let entries = match std::fs::read_dir(&projects) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        let mut by_key: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for entry in entries {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if let Some(key) = keys::key_for(&name, &home_enc) {
                by_key.entry(key).or_default().push(name);
            }
        }
        for (key, names) in by_key {
            let folder = keys::choose(&projects, names, &home_enc);
            out.folders.insert(key.clone(), folder.clone());
            let mem = projects.join(&folder).join(MEMORY);
            if blocked_by_user(
                &self.paths.claude_home,
                &format!("projects/{folder}/memory/x"),
            ) || std::fs::symlink_metadata(&mem).is_ok_and(|m| m.file_type().is_symlink())
            {
                out.held_keys.insert(key.clone());
                out.notes
                    .push(format!("memory/{key}: not synced, its folder is a link"));
                continue;
            }
            let mut dirs = vec![mem.clone()];
            while let Some(dir) = dirs.pop() {
                let entries = match std::fs::read_dir(&dir) {
                    Ok(e) => e,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e.into()),
                };
                for entry in entries {
                    let entry = entry?;
                    let path = entry.path();
                    let rel: Vec<String> = path
                        .strip_prefix(&mem)
                        .expect("walked from mem")
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect();
                    let repo_path = format!("memory/{key}/{}", rel.join("/"));
                    let kind = entry.file_type()?;
                    if kind.is_symlink() {
                        out.held.insert(repo_path.clone());
                        out.notes
                            .push(format!("{repo_path}: not synced, it is a link"));
                    } else if kind.is_dir() {
                        dirs.push(path);
                    } else if kind.is_file() && is_personal_path(&repo_path) {
                        let len = entry.metadata()?.len() as usize;
                        if len > MAX_FILE {
                            out.held.insert(repo_path.clone());
                            out.notes.push(too_large(&repo_path));
                        } else if total + len > MAX_TOTAL {
                            out.held.insert(repo_path.clone());
                            out.notes.push(format!(
                                "{repo_path}: not synced, personal memory is over {} MiB",
                                MAX_TOTAL >> 20
                            ));
                        } else {
                            total += len;
                            out.files.insert(repo_path, std::fs::read(&path)?);
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// `~/.claude/settings.json` as `settings.json`, without what groups set or what is machine-specific.
    fn scan_settings(&self, out: &mut LocalScan) -> crate::Result<()> {
        let skip = settings_file::skip_set(&self.paths)?;
        out.settings_skip = Some(skip.clone());
        let file = self.paths.claude_home.join("settings.json");
        let path = settings_file::PATH;
        match std::fs::symlink_metadata(&file) {
            Ok(md) if !md.is_file() => {
                out.held.insert(path.into());
                out.notes
                    .push(format!("{path}: not synced, it is a link or a folder"));
            }
            Ok(md) if md.len() as usize > MAX_FILE => {
                out.held.insert(path.into());
                out.notes.push(too_large(path));
            }
            Ok(_) => match crate::settings::parse_object(&std::fs::read(&file)?) {
                Some(real) => {
                    if let Some(bytes) = settings_file::bytes_of(&settings_file::view(&real, &skip))
                    {
                        out.files.insert(path.into(), bytes);
                    }
                }
                None => {
                    out.held.insert(path.into());
                    out.notes
                        .push(format!("{path}: not synced, it is not one JSON object"));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// `~/.claude/personal-state` is a git clone of its own: only `state/claude-sync/` syncs.
    fn state_is_git_clone(&self) -> bool {
        self.paths
            .claude_home
            .join(state::LOCAL_DIR)
            .join(".git")
            .exists()
    }

    /// `~/.claude/personal-state/**` as `state/**`, held back by the same rules as memory.
    fn scan_state(&self, out: &mut LocalScan) -> crate::Result<()> {
        let root = self.paths.claude_home.join(state::LOCAL_DIR);
        if blocked_by_user(&self.paths.claude_home, &format!("{}/x", state::LOCAL_DIR)) {
            out.held_state = true;
            out.notes
                .push(format!("{}/: not synced, its folder is a link", state::DIR));
            return Ok(());
        }
        let git_state = self.state_is_git_clone();
        let mut total: usize = out.files.values().map(Vec::len).sum();
        let mut dirs = vec![root.clone()];
        while let Some(dir) = dirs.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                let rel: Vec<String> = path
                    .strip_prefix(&root)
                    .expect("walked from root")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                let repo_path = format!("{}/{}", state::DIR, rel.join("/"));
                if git_state && rel[0] != "claude-sync" {
                    continue;
                }
                let kind = entry.file_type()?;
                if kind.is_symlink() {
                    out.held.insert(repo_path.clone());
                    out.notes
                        .push(format!("{repo_path}: not synced, it is a link"));
                } else if kind.is_dir() {
                    dirs.push(path);
                } else if kind.is_file() && is_personal_path(&repo_path) {
                    let len = entry.metadata()?.len() as usize;
                    if len > MAX_FILE {
                        out.held.insert(repo_path.clone());
                        out.notes.push(too_large(&repo_path));
                    } else if total + len > MAX_TOTAL {
                        out.held.insert(repo_path.clone());
                        out.notes.push(format!(
                            "{repo_path}: not synced, personal memory is over {} MiB",
                            MAX_TOTAL >> 20
                        ));
                    } else {
                        total += len;
                        out.files.insert(repo_path, std::fs::read(&path)?);
                    }
                }
            }
        }
        Ok(())
    }

    /// Changed files that look like they hold a secret stay on this device.
    fn hold_secrets(&self, scan: &mut LocalScan, base: &FileSet) {
        let mut found = Vec::new();
        for (path, bytes) in &scan.files {
            if base.get(path) == Some(bytes) {
                continue;
            }
            if let Some(hit) = (self.scanner)(&String::from_utf8_lossy(bytes))
                .into_iter()
                .next()
            {
                found.push((path.clone(), hit));
            }
        }
        for (path, hit) in found {
            scan.notes.push(format!(
                "{path}: not synced, line {} looks like a secret ({})",
                hit.line, hit.pattern
            ));
            scan.held.insert(path);
        }
    }

    fn check_secrets(&self, changes: &Changes) -> crate::Result<()> {
        for (path, content) in changes {
            let Some(bytes) = content else { continue };
            if let Some(hit) = (self.scanner)(&String::from_utf8_lossy(bytes))
                .into_iter()
                .next()
            {
                return Err(Error::SecretFound {
                    path: path.clone(),
                    line: hit.line,
                    pattern: hit.pattern.into(),
                });
            }
        }
        Ok(())
    }

    /// Make this device match `merged` where it differs from `local`. Held paths are never
    /// written. Returns the repo paths written.
    fn write_local(
        &self,
        local: &FileSet,
        merged: &FileSet,
        scan: &mut LocalScan,
    ) -> crate::Result<Vec<String>> {
        let paths: BTreeSet<&String> = local.keys().chain(merged.keys()).collect();
        let mut written = Vec::new();
        let backup_root = self
            .paths
            .backups_dir()
            .join("personal")
            .join(utc_stamp(now_unix()));
        for path in paths {
            let want = merged.get(path);
            if local.get(path) == want || scan.is_held(path) {
                continue;
            }
            if path == CLAUDE_MD {
                self.write_claude_md(want.map(Vec::as_slice))?;
                written.push(path.clone());
                continue;
            }
            if path == settings_file::PATH {
                if self.write_settings(local.get(path), want, &backup_root, scan)? {
                    written.push(path.clone());
                }
                continue;
            }
            let (under_home, stop, folder) = if let Some(rel) = path.strip_prefix("state/") {
                (
                    format!("{}/{rel}", state::LOCAL_DIR),
                    state::LOCAL_DIR.to_string(),
                    None,
                )
            } else {
                let Some((key, rel)) = path.strip_prefix("memory/").and_then(|r| r.split_once('/'))
                else {
                    continue;
                };
                let folder = match scan.folders.get(key) {
                    Some(f) => f.clone(),
                    None => match keys::folder_for(key, &self.home_enc()?) {
                        Some(f) => f,
                        None => continue,
                    },
                };
                (
                    format!("projects/{folder}/memory/{rel}"),
                    format!("projects/{folder}/memory"),
                    Some((key.to_string(), folder)),
                )
            };
            if blocked_by_user(&self.paths.claude_home, &under_home) {
                scan.notes.push(format!(
                    "{path}: not written, a folder on the way is a link"
                ));
                continue;
            }
            let file = join_rel(&self.paths.claude_home, &under_home);
            // The file being overwritten or deleted is kept, like CLAUDE.md.
            if let Some(old) = local.get(path) {
                write_atomic(&join_rel(&backup_root, path), old)?;
            }
            match want {
                Some(bytes) => write_atomic(&file, bytes)?,
                None => {
                    match std::fs::remove_file(&file) {
                        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                        _ => {}
                    }
                    prune_empty(&file, &join_rel(&self.paths.claude_home, &stop));
                }
            }
            if let Some((key, folder)) = folder {
                scan.folders.entry(key).or_insert(folder);
            }
            written.push(path.clone());
        }
        Ok(written)
    }

    /// Move the real `settings.json` from the view it had to the merged one, with a backup.
    fn write_settings(
        &self,
        old: Option<&Vec<u8>>,
        new: Option<&Vec<u8>>,
        backup_root: &Path,
        scan: &mut LocalScan,
    ) -> crate::Result<bool> {
        let file = self.paths.claude_home.join("settings.json");
        let existing = match std::fs::read(&file) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let mut real = match &existing {
            Some(b) => match crate::settings::parse_object(b) {
                Some(v) => v,
                None => return Ok(false),
            },
            None => Value::Object(Map::new()),
        };
        let skip = match scan.settings_skip.clone() {
            Some(s) => s,
            None => settings_file::skip_set(&self.paths)?,
        };
        let before = real.clone();
        settings_file::apply(&mut real, old, new, &skip);
        if real == before {
            return Ok(false);
        }
        if let Some(old) = &existing {
            write_atomic(&backup_root.join(settings_file::PATH), old)?;
        }
        write_atomic(&file, &crate::settings::to_bytes(&real))?;
        Ok(true)
    }

    /// Put `synced` under this device's managed block, with a backup of the old file.
    fn write_claude_md(&self, synced: Option<&[u8]>) -> crate::Result<()> {
        let file = self.paths.claude_home.join(CLAUDE_MD);
        let existing = match std::fs::read(&file) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let new = claude_md::compose(existing.as_deref(), synced);
        if new == existing {
            return Ok(());
        }
        if let Some(old) = &existing {
            let backup = self
                .paths
                .backups_dir()
                .join("personal")
                .join(utc_stamp(now_unix()))
                .join(CLAUDE_MD);
            write_atomic(&backup, old)?;
        }
        match new {
            Some(bytes) => write_atomic(&file, &bytes),
            None => Ok(std::fs::remove_file(&file)?),
        }
    }

    fn save_personal_conflicts(
        &self,
        conflicts: &[merge::Conflict],
    ) -> crate::Result<Vec<ConflictInfo>> {
        let at_unix = now_unix();
        let dir = self
            .paths
            .state_dir
            .join("conflicts")
            .join("personal")
            .join(utc_stamp(at_unix));
        conflicts
            .iter()
            .map(|c| {
                let saved_to = match &c.local {
                    Some(bytes) => {
                        let path = join_rel(&dir, &c.path);
                        write_atomic(&path, bytes)?;
                        path.display().to_string()
                    }
                    None => String::new(),
                };
                Ok(ConflictInfo {
                    path: c.path.clone(),
                    saved_to,
                    at_unix,
                })
            })
            .collect()
    }

    fn personal_id(&self) -> GroupId {
        GroupId::new(&self.login, REPO)
    }

    fn personal_dir(&self) -> PathBuf {
        self.paths.state_dir.join("personal")
    }

    /// Forget what was last synced: the next sync merges like the first one.
    fn clear_bases(&self) -> crate::Result<()> {
        let dir = self.personal_dir();
        for name in [
            "base",
            "base.old",
            Kind::ClaudeMd.off_dir(),
            Kind::Memory.off_dir(),
            Kind::State.off_dir(),
            Kind::Settings.off_dir(),
        ] {
            remove_dir_if_exists(&dir.join(name))?;
        }
        Ok(())
    }

    fn personal_message(&self, summary: &str) -> String {
        format!("{}@{}: {summary}", self.login, self.device)
    }

    fn home_enc(&self) -> crate::Result<String> {
        let home = self
            .user_home
            .as_ref()
            .ok_or_else(|| Error::Invalid("cannot find the home directory".into()))?;
        Ok(keys::encode(&home.to_string_lossy()))
    }

    fn this_device(&self, st: &State) -> Device {
        Device {
            id: st.device_id.clone(),
            name: self.device.clone(),
            os: std::env::consts::OS.into(),
            last_seen_unix: now_unix(),
        }
    }

    fn load_personal(&self) -> State {
        let file = self.personal_dir().join("state.json");
        let mut st: State = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if st
            .login
            .as_deref()
            .is_some_and(|l| !l.eq_ignore_ascii_case(&self.login))
        {
            // Another account signed in: its personal memory is another repo.
            st = State {
                device_id: st.device_id,
                ..State::default()
            };
            if let Err(e) = self.clear_bases() {
                tracing::warn!(error = %e, "could not clear the personal base");
            }
        }
        st.login = Some(self.login.clone());
        if st.device_id.is_empty() {
            st.device_id = new_device_id(&self.paths.state_dir, &self.device);
        }
        st
    }

    fn save_personal(&self, st: &State) -> crate::Result<()> {
        write_json(&self.personal_dir().join("state.json"), st)
    }

    fn status_of(&self, st: &State) -> PersonalStatus {
        let id = self.personal_id();
        let mut devices: Vec<DeviceStatus> = st
            .devices
            .iter()
            .map(|d| DeviceStatus {
                id: d.id.clone(),
                name: d.name.clone(),
                os: d.os.clone(),
                last_seen_unix: d.last_seen_unix,
                this_device: d.id == st.device_id,
            })
            .collect();
        if st.enabled && !devices.iter().any(|d| d.this_device) {
            let me = self.this_device(st);
            devices.push(DeviceStatus {
                id: me.id,
                name: me.name,
                os: me.os,
                last_seen_unix: me.last_seen_unix,
                this_device: true,
            });
        }
        PersonalStatus {
            enabled: st.enabled,
            decided: st.decided,
            offer: !st.enabled && !st.decided && st.new_devices_join == Some(false),
            kinds: st.kinds,
            repo: id.to_string(),
            repo_url: format!("https://github.com/{}/{}", id.owner, id.repo),
            device_id: st.device_id.clone(),
            devices,
            new_devices_join: st.new_devices_join,
            last_sync_unix: st.last_sync_unix,
            last_error: st.last_error.clone(),
            conflicts: st.conflicts.clone(),
            skipped: st.skipped.clone(),
        }
    }
}

/// One of the things that can be turned on and off.
#[derive(Clone, Copy)]
enum Kind {
    ClaudeMd,
    Memory,
    State,
    Settings,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::ClaudeMd, Kind::Memory, Kind::State, Kind::Settings];

    fn on(self, kinds: &Kinds) -> bool {
        match self {
            Kind::ClaudeMd => kinds.claude_md,
            Kind::Memory => kinds.memory,
            Kind::State => kinds.state,
            Kind::Settings => kinds.settings,
        }
    }

    fn covers(self, path: &str) -> bool {
        match self {
            Kind::ClaudeMd => path == CLAUDE_MD,
            Kind::Memory => path.starts_with("memory/"),
            Kind::State => path.starts_with("state/"),
            Kind::Settings => path == settings_file::PATH,
        }
    }

    /// Where the last synced files of a kind wait while it is off.
    fn off_dir(self) -> &'static str {
        match self {
            Kind::ClaudeMd => "off-claude-md",
            Kind::Memory => "off-memory",
            Kind::State => "off-state",
            Kind::Settings => "off-settings",
        }
    }
}

/// Three-way merge, except `MEMORY.md` indexes, which merge as line sets, `state/**.jsonl`
/// logs, which merge as sorted line sets, and `state/**.json` objects that both sides have
/// with a `ts`, where the newer one wins.
fn merge_personal<'a>(base: &'a FileSet, local: &'a FileSet, remote: &'a FileSet) -> MergeResult {
    let choices: BTreeMap<&String, Vec<u8>> = local
        .iter()
        .filter(|(p, _)| p.as_str() == crate::choices::REPO_PATH)
        .filter_map(|(p, l)| Some((p, crate::choices::merge(l, remote.get(p)?)?)))
        .collect();
    let newer: BTreeMap<&String, &[u8]> = local
        .iter()
        .filter(|(p, _)| state::is_json(p) && !choices.contains_key(p))
        .filter_map(|(p, l)| Some((p, state::newer(l, remote.get(p)?)?)))
        .collect();
    let by_lines = |p: &str| lines::is_index(p) || state::is_jsonl(p);
    let three = |p: &str| {
        !by_lines(p)
            && p != settings_file::PATH
            && !newer.keys().any(|k| k.as_str() == p)
            && !choices.keys().any(|k| k.as_str() == p)
    };
    let mut out = merge::three_way(
        &only(base, three),
        &only(local, three),
        &only(remote, three),
    );
    for (path, bytes) in &newer {
        if Some(*bytes) != remote.get(*path).map(Vec::as_slice) {
            out.to_push.insert((*path).clone(), Some(bytes.to_vec()));
        }
        out.merged.insert((*path).clone(), bytes.to_vec());
    }
    for (path, bytes) in &choices {
        if Some(bytes.as_slice()) != remote.get(*path).map(Vec::as_slice) {
            out.to_push.insert((*path).clone(), Some(bytes.clone()));
        }
        out.merged.insert((*path).clone(), bytes.clone());
    }
    let get_settings = |f: &'a FileSet| f.get(settings_file::PATH);
    let settings = settings_file::merge(
        get_settings(base),
        get_settings(local),
        get_settings(remote),
    );
    if settings.as_ref() != get_settings(remote) {
        out.to_push
            .insert(settings_file::PATH.into(), settings.clone());
    }
    if let Some(bytes) = settings {
        out.merged.insert(settings_file::PATH.into(), bytes);
    }
    let indexes: BTreeSet<&String> = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .filter(|p| by_lines(p))
        .collect();
    for path in indexes {
        let get = |f: &'a FileSet| f.get(path).map(Vec::as_slice);
        let merged = if state::is_jsonl(path) {
            state::merge_log(get(base), get(local), get(remote))
        } else {
            lines::merge(get(base), get(local), get(remote))
        };
        if merged.as_deref() != get(remote) {
            out.to_push.insert(path.clone(), merged.clone());
        }
        if let Some(bytes) = merged {
            out.merged.insert(path.clone(), bytes);
        }
    }
    out
}

/// Hash of a set of files, to tell whether anything changed since the last sync.
fn fingerprint(files: &FileSet) -> String {
    let mut all = Vec::new();
    for (path, bytes) in files {
        all.extend_from_slice(path.as_bytes());
        all.push(0);
        all.extend_from_slice(sha256_hex(bytes).as_bytes());
        all.push(b'\n');
    }
    sha256_hex(&all)
}

fn new_device_id(state_dir: &Path, device: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seed = format!(
        "{nanos}:{}:{device}:{}",
        std::process::id(),
        state_dir.display()
    );
    sha256_hex(seed.as_bytes())[..16].to_string()
}

/// Remove empty folders from `file`'s folder up to, not including, `stop`.
fn prune_empty(file: &Path, stop: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == stop || !d.starts_with(stop) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

fn too_large(path: &str) -> String {
    format!("{path}: not synced, it is over {} KiB", MAX_FILE >> 10)
}

fn not_personal(id: &GroupId) -> Error {
    Error::Invalid(format!(
        "{id} exists but is not a claude-sync personal memory repo; rename or delete it on GitHub"
    ))
}

/// The error for a repo with our name but no marker: the personal-rules mod's own repo (its
/// layout is `owners/` and `projects/`, with a README naming the mod) or some other repo.
fn clash(id: &GroupId, files: &crate::FileSet) -> Error {
    let readme = files
        .get("README.md")
        .is_some_and(|r| String::from_utf8_lossy(r).contains("personal-rules mod"));
    let layout = files
        .keys()
        .any(|p| p.starts_with("owners/") || p.starts_with("projects/"));
    if readme && layout {
        Error::Invalid(format!(
            "{id} is used by the personal-rules mod, not by personal memory. Personal memory \
             can't start until that mod moves to its own repo (an update is on the way)."
        ))
    } else {
        Error::Invalid(format!(
            "{id} already exists on GitHub and wasn't made by claude-sync, so personal memory \
             won't use it. Rename or delete that repo on GitHub, then turn this on again."
        ))
    }
}

fn readme(login: &str) -> String {
    format!(
        "# Personal memory of @{login}\n\n\
         This private repo is managed by the claude-sync app. It keeps Claude Code's memory in \
         step between @{login}'s own devices. It is never shared with a group.\n\n\
         - `CLAUDE.md`: your own instructions from `~/.claude/CLAUDE.md` (without the part \
         the app manages for groups)\n\
         - `memory/<project>/`: Claude Code's auto-memory of each project, keyed by its path \
         under your home folder\n\
         - `state/`: small settings your mods save in `~/.claude/personal-state/`, such as \
         opus-only mode\n\
         - `settings.json`: your own `~/.claude/settings.json` without what groups set and \
         what cannot work on another machine\n\
         - `devices.json`: the devices that sync this repo\n\n\
         Keep this repo private. Turn syncing off in the app's Settings.\n"
    )
}
