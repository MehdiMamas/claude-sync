//! Personal memory (Parity 43, 44, 45, 46, 54, 87): the person's own CLAUDE.md, memory, state
//! and settings through the private repo `<login>/claude-sync-personal`. The pass itself is
//! `fs::pass` (the files, MEMORY.md line merge, conflicts); this file owns the switch, the
//! privacy check on every pass, the device list, and the choices that follow the person.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::*;
use crate::msg;
use claude_sync_engine::formats::personal::{
    Device, DevicesFile, PersonalChoice, PersonalFile, DEVICES, FILE, REPO,
};
use claude_sync_engine::formats::Doc;
use claude_sync_engine::merge::merge_choices_bytes;
use claude_sync_engine::traits::{Remote, Store};
use claude_sync_fs::pass::Report;
use claude_sync_github::remote::GithubRemote;
use claude_sync_github::Error as GhError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const STATE: &str = "personal/state";
const KINDS: [&str; 4] = ["claudeMd", "memory", "state", "settings"];
/// A device entry is refreshed at most this often.
const SEEN_EVERY: i64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub(crate) enum Pending {
    Rename { id: String, name: String },
    Remove { id: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct PState {
    enabled: bool,
    /// Turned on or off, or "not now": the new-device check stays quiet.
    decided: bool,
    /// Unix ms of the last turning on: an older "off" choice is not about it.
    enabled_at_ms: u64,
    new_devices_join: Option<bool>,
    device_id: String,
    error: Option<String>,
    last_sync: Option<i64>,
    pending: Vec<Pending>,
    devices: Vec<Device>,
    /// The repo head `devices` was read at: a pass that moved it reads the list again.
    devices_head: String,
}

enum Found {
    Nothing,
    Offer,
    Join,
}

fn gh_missing(e: &GhError) -> bool {
    matches!(e.status(), Some(404 | 409))
}

fn ms(now: i64) -> u64 {
    now.max(0) as u64 * 1000
}

fn pretty<T: Serialize>(v: &T) -> Vec<u8> {
    let mut b = serde_json::to_vec_pretty(v).unwrap_or_default();
    b.push(b'\n');
    b
}

impl Core {
    fn pstate(&self) -> PState {
        if let Some(s) = Store::read(&self.db, STATE)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
        {
            return s;
        }
        // first run after the v1 import: its state file says whether personal memory is on
        let v1 = Store::read(&self.db, "v1/personal/state.json")
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        let Some(v) = v1 else {
            return PState::default();
        };
        PState {
            enabled: v["enabled"].as_bool().unwrap_or(false),
            decided: v["decided"].as_bool().unwrap_or(false),
            enabled_at_ms: v["enabledAtMs"].as_u64().unwrap_or(0),
            new_devices_join: v["newDevicesJoin"].as_bool(),
            device_id: v["deviceId"].as_str().unwrap_or_default().into(),
            ..PState::default()
        }
    }

    fn save_pstate(&self, s: &PState) -> Result<()> {
        Ok(Store::write(
            &self.db,
            STATE,
            &serde_json::to_vec(s).unwrap_or_default(),
        )?)
    }

    pub(crate) fn pchoice(&self) -> PersonalChoice {
        self.choices()
            .rest
            .get("personal")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    fn set_pchoice(&self, f: impl FnOnce(&mut PersonalChoice)) -> Result<()> {
        let mut c = self.choices();
        let mut p = self.pchoice();
        f(&mut p);
        p.ts = ms(self.now()).max(p.ts + 1);
        c.rest.insert(
            "personal".into(),
            serde_json::to_value(&p).unwrap_or_default(),
        );
        Ok(c.save(&self.db)?)
    }

    fn me_repo(&self) -> Result<String> {
        Ok(format!("{}/{REPO}", self.me()?))
    }

    fn sig(&self) -> String {
        self.env.device.clone()
    }

    fn remote_of(&self) -> Result<GithubRemote> {
        Ok(GithubRemote::new(
            self.client.clone(),
            &self.me()?,
            REPO,
            "main",
        ))
    }

    fn this_device(&self, st: &PState) -> Device {
        Device {
            id: st.device_id.clone(),
            name: self
                .env
                .device
                .split_once('@')
                .map_or(self.env.device.as_str(), |x| x.1)
                .to_string(),
            os: std::env::consts::OS.to_string(),
            last_seen_unix: self.now().max(0) as u64,
        }
    }

    /// The repo as it stands: `None` when there is none or it has no commit yet.
    fn read_repo(&self) -> Result<Option<(bool, claude_sync_engine::plan::FileSet, String)>> {
        let me = self.me()?;
        let repo = match self.client.repo(&me, REPO) {
            Ok(r) => r,
            Err(e) if gh_missing(&e) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let head = match self.client.head(&me, REPO, "main", None) {
            Ok(h) => h,
            Err(e) if gh_missing(&e) => {
                return Ok(Some((repo.private, Default::default(), String::new())))
            }
            Err(e) => return Err(e.into()),
        };
        let sha = head.map(|h| h.sha).unwrap_or_default();
        let files = if sha.is_empty() {
            Default::default()
        } else {
            self.remote_of()?.files(&sha, &BTreeMap::new())?
        };
        Ok(Some((repo.private, files, sha)))
    }

    fn clash(&self, files: &claude_sync_engine::plan::FileSet) -> Error {
        let id = self.me_repo().unwrap_or_default();
        let readme = files
            .get("README.md")
            .is_some_and(|r| String::from_utf8_lossy(r).contains("personal-rules mod"));
        let layout = files
            .keys()
            .any(|p| p.starts_with("owners/") || p.starts_with("projects/"));
        Error::new(if readme && layout {
            msg::personal_mod(&id)
        } else {
            msg::personal_foreign(&id)
        })
    }

    /// Mark this device as taking part from now on (an empty base, like the first time).
    fn activate(&self, st: &mut PState) -> Result<()> {
        self.ensure_id(st);
        self.db.save_base(
            claude_sync_store::PERSONAL,
            &claude_sync_store::BaseRec::default(),
            self.now(),
        )?;
        st.enabled = true;
        st.decided = true;
        st.enabled_at_ms = ms(self.now());
        st.error = None;
        self.save_pstate(st)?;
        // an "off" from before this moment is not about this turning on
        self.set_pchoice(|p| p.off = false)
    }

    fn ensure_id(&self, st: &mut PState) {
        if st.device_id.is_empty() {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            use std::hash::{Hash, Hasher};
            (
                self.now(),
                &self.env.device,
                std::process::id(),
                &self.env.home,
            )
                .hash(&mut h);
            st.device_id = format!("{:016x}", h.finish());
        }
    }

    pub(crate) fn personal_on(&mut self) -> Result<Event> {
        let me = self.me()?;
        let mut st = self.pstate();
        let (id, mut fresh) = (self.me_repo()?, false);
        if self.read_repo()?.is_none() {
            self.client
                .create_repo(REPO, "claude-sync personal memory")?;
            fresh = true;
        }
        let Some((private, files, sha)) = self.read_repo()? else {
            return Err(Error::new(msg::not_personal(&id)));
        };
        if !private {
            return Err(Error::new(msg::public_repo(&id)));
        }
        self.ensure_id(&mut st);
        if fresh || sha.is_empty() {
            let dev = DevicesFile {
                devices: vec![self.this_device(&st)],
            };
            let readme = format!("# Personal memory of @{me}\n\nThis private repo is managed by the claude-sync app. It keeps Claude Code's memory, CLAUDE.md and settings the same on your devices. Do not edit it by hand.\n");
            let mut put = BTreeMap::new();
            put.insert(
                FILE.to_string(),
                Some(Doc::new(PersonalFile::new()).to_bytes()?),
            );
            put.insert(DEVICES.to_string(), Some(pretty(&dev)));
            if sha.is_empty() {
                put.insert("README.md".into(), Some(readme.into_bytes()));
            }
            self.remote_of()?.push(
                &sha,
                &put,
                &format!("{}: start personal memory", self.sig()),
            )?;
        } else {
            match files.get(FILE) {
                Some(b) => drop(PersonalFile::open(b)?),
                None => return Err(self.clash(&files)),
            }
        }
        self.activate(&mut st)?;
        self.sync_now()
    }

    pub(crate) fn personal_dismiss(&mut self) -> Result<Event> {
        let mut st = self.pstate();
        st.decided = true;
        self.save_pstate(&st)?;
        Ok(Event::Done)
    }

    /// Off on every device: the choice goes out in one last pass, other devices follow at their
    /// next one, and only after that pass worked new devices stop joining by themselves.
    pub(crate) fn personal_off(&mut self) -> Result<Event> {
        let mut st = self.pstate();
        if st.enabled {
            let floor = st.enabled_at_ms + 1;
            self.set_pchoice(|p| {
                p.off = true;
                p.ts = p.ts.max(floor);
            })?;
            let ok = self.sync_now().is_ok() && self.pstate().error.is_none();
            st = self.pstate();
            if ok {
                let _ = self.set_new_devices(false);
                st.new_devices_join = Some(false);
            }
        }
        st.enabled = false;
        st.decided = true;
        self.save_pstate(&st)?;
        Ok(Event::Done)
    }

    /// Whether this person's next new device turns personal memory on by itself (Parity 43).
    fn set_new_devices(&self, join: bool) -> Result<()> {
        let r = self.remote_of()?;
        let Some(h) = r.head(None)? else {
            return Ok(());
        };
        let files = r.files(&h.sha, &BTreeMap::new())?;
        let Some(b) = files.get(FILE) else {
            return Err(Error::new(msg::not_personal(&self.me_repo()?)));
        };
        let mut doc = PersonalFile::open(b)?;
        doc.value.new_devices_join = join;
        let mut put = BTreeMap::new();
        put.insert(FILE.to_string(), Some(doc.to_bytes()?));
        r.push(
            &h.sha,
            &put,
            &format!("{}: set new devices join", self.sig()),
        )?;
        Ok(())
    }

    pub(crate) fn personal_join_auto(&mut self, on: bool) -> Result<Event> {
        self.set_new_devices(on)?;
        let mut st = self.pstate();
        st.new_devices_join = Some(on);
        self.save_pstate(&st)?;
        Ok(Event::Done)
    }

    pub(crate) fn personal_kind(&mut self, kind: &str, on: bool) -> Result<Event> {
        if !KINDS.contains(&kind) {
            return Err(Error::new(format!(
                "{kind} is not a kind of personal memory"
            )));
        }
        self.set_pchoice(|p| {
            if on {
                p.kinds_off.remove(kind);
            } else {
                p.kinds_off.insert(kind.to_string());
            }
        })?;
        Ok(Event::Done)
    }

    pub(crate) fn device_edit(&mut self, p: Pending) -> Result<Event> {
        let mut st = self.pstate();
        if matches!(&p, Pending::Remove { id } if *id == st.device_id) {
            return Err(Error::new(msg::CANT_REMOVE_SELF));
        }
        match &p {
            Pending::Rename { id, name } => {
                // v1 device-name rule (the name is part of commit signatures and ssh aliases)
                if name.trim().is_empty() || name.len() > 64 || name.contains(['/', '\\', ':', '@'])
                {
                    return Err(Error::new(msg::DEVICE_NAME_INVALID));
                }
                if let Some(d) = st.devices.iter_mut().find(|d| d.id == *id) {
                    d.name = name.clone();
                }
            }
            Pending::Remove { id } => st.devices.retain(|d| d.id != *id),
        }
        st.pending.push(p);
        self.save_pstate(&st)?;
        Ok(Event::Done)
    }

    // ---- the pass ----------------------------------------------------------------------

    /// Before a pass: the new-device check while not decided. It never fails the pass.
    pub(crate) fn personal_discover(&mut self) {
        let mut st = self.pstate();
        if st.enabled || st.decided || self.env.no_auto_personal {
            return;
        }
        let found = (|| -> Result<Found> {
            let Some((private, files, _)) = self.read_repo()? else {
                return Ok(Found::Nothing);
            };
            let Some(b) = files.get(FILE) else {
                return Ok(Found::Nothing);
            };
            let doc = PersonalFile::open(b)?;
            Ok(if !private {
                Found::Nothing
            } else if doc.value.new_devices_join {
                Found::Join
            } else {
                Found::Offer
            })
        })();
        match found {
            Ok(Found::Join) => {
                if self.activate(&mut st).is_ok() {
                    let _ = self.db.add_notification(
                        "personal",
                        msg::PERSONAL_JOINED,
                        None,
                        self.now(),
                    );
                }
            }
            Ok(Found::Offer) if st.new_devices_join != Some(false) => {
                st.new_devices_join = Some(false);
                let _ = self.save_pstate(&st);
                let _ = self
                    .db
                    .add_notification("personal", msg::PERSONAL_OFFER, None, self.now());
            }
            _ => {}
        }
    }

    fn choices_file(&self) -> std::path::PathBuf {
        self.home
            .claude
            .join("personal-state")
            .join("claude-sync")
            .join("choices.json")
    }

    /// The remote to pass to the pass: `None` when personal memory is off, or the repo is not
    /// private (checked on every pass: nothing is pushed or pulled then).
    pub(crate) fn personal_before(&self) -> Option<GithubRemote> {
        let mut st = self.pstate();
        if !st.enabled {
            return None;
        }
        let id = self.me_repo().ok()?;
        match self.client.repo(&self.me().ok()?, REPO) {
            Ok(r) if !r.private => {
                let text = msg::public_repo(&id);
                if st.error.as_deref() != Some(&text) {
                    let _ = self
                        .db
                        .add_notification("personal", &text, None, self.now());
                }
                st.error = Some(text);
                let _ = self.save_pstate(&st);
                return None;
            }
            Err(e) if matches!(e.status(), Some(403 | 404)) => {
                st.error = Some(msg::cant_reach(&id, &e.to_string()));
                let _ = self.save_pstate(&st);
                return None;
            }
            _ => {}
        }
        // the choices go out with the pass (Parity 54, 87)
        let bytes = pretty(&self.choices());
        let f = self.choices_file();
        if std::fs::read(&f).ok().as_deref() != Some(&bytes) {
            if let Some(d) = f.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&f, &bytes);
        }
        self.remote_of().ok()
    }

    /// After a pass: take the choices another device sent, follow its "off", note the result
    /// and keep the device list.
    pub(crate) fn personal_after(&self, rep: &Report, remote: Option<&GithubRemote>) {
        let (Some(res), Some(remote)) = (&rep.personal, remote) else {
            return;
        };
        let mut st = self.pstate();
        let id = self.me_repo().unwrap_or_default();
        match res {
            Ok(_) => {
                st.error = None;
                st.last_sync = Some(self.now());
            }
            Err(e) => {
                st.error = Some(if e.contains("404") || e.contains("403") {
                    msg::cant_reach(&id, e)
                } else {
                    e.clone()
                });
            }
        }
        if res.is_ok() {
            self.take_choices();
            let p = self.pchoice();
            if p.off && p.ts > st.enabled_at_ms {
                st.enabled = false;
                st.decided = true;
                let _ = self.db.add_notification(
                    "personal",
                    msg::PERSONAL_OFF_ELSEWHERE,
                    None,
                    self.now(),
                );
            } else if p.off {
                // a stale off from before this turning on
                let _ = self.set_pchoice(|p| p.off = false);
            }
            if st.enabled {
                if let Err(e) = self.devices_step(remote, &mut st) {
                    st.error = Some(e.to_string());
                }
            }
        }
        let _ = self.save_pstate(&st);
    }

    fn take_choices(&self) {
        let Ok(disk) = std::fs::read(self.choices_file()) else {
            return;
        };
        let mine = pretty(&self.choices());
        if let Some(m) = merge_choices_bytes(&mine, &disk) {
            if m != mine {
                if let Ok(c) = serde_json::from_slice(&m) {
                    let _ = crate::choices::Choices::save(&c, &self.db);
                }
            }
        }
    }

    /// `devices.json`: this device's entry (at most daily), then renames and removals.
    fn devices_step(&self, remote: &GithubRemote, st: &mut PState) -> Result<()> {
        let now = self.now();
        let mine = st.devices.iter().find(|d| d.id == st.device_id);
        let due = mine.is_none_or(|d| d.last_seen_unix as i64 + SEEN_EVERY <= now);
        let at = self
            .db
            .load_base(claude_sync_store::PERSONAL, &self.home.state)?
            .map(|b| b.head_sha)
            .unwrap_or_default();
        if !due && st.pending.is_empty() && at == st.devices_head {
            return Ok(());
        }
        let Some(h) = remote.head(None)? else {
            return Ok(());
        };
        let files = remote.files(&h.sha, &BTreeMap::new())?;
        let old = files.get(DEVICES);
        let mut list: Vec<Device> = old
            .and_then(|b| serde_json::from_slice::<DevicesFile>(b).ok())
            .map(|f| f.devices)
            .unwrap_or_default();
        for p in st.pending.drain(..) {
            match p {
                Pending::Rename { id, name } => {
                    if let Some(d) = list.iter_mut().find(|d| d.id == id) {
                        d.name = name;
                    }
                }
                Pending::Remove { id } => list.retain(|d| d.id != id),
            }
        }
        match list.iter_mut().find(|d| d.id == st.device_id) {
            Some(d) if due => d.last_seen_unix = now.max(0) as u64,
            Some(_) => {}
            None => list.push(self.this_device(st)),
        }
        let bytes = pretty(&DevicesFile {
            devices: list.clone(),
        });
        if old.map(|b| b.as_slice()) != Some(bytes.as_slice()) {
            let mut put = BTreeMap::new();
            put.insert(DEVICES.to_string(), Some(bytes));
            remote.push(&h.sha, &put, &format!("{}: update devices", self.sig()))?;
        }
        st.devices_head = at;
        st.devices = list;
        Ok(())
    }

    pub(crate) fn personal_view(&self) -> PersonalView {
        let st = self.pstate();
        let p = self.pchoice();
        PersonalView {
            on: st.enabled,
            offer: !st.enabled && !st.decided && st.new_devices_join == Some(false),
            repo: self.me_repo().unwrap_or_default(),
            new_devices_join: st.new_devices_join != Some(false),
            kinds_off: p.kinds_off.into_iter().collect(),
            devices: st
                .devices
                .iter()
                .map(|d| DeviceView {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    os: d.os.clone(),
                    last_seen: d.last_seen_unix as i64,
                    this: d.id == st.device_id,
                })
                .collect(),
            stays: Store::read(&self.db, "personal/stays")
                .ok()
                .flatten()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
            error: st.error,
            last_sync: st.last_sync,
        }
    }
}
