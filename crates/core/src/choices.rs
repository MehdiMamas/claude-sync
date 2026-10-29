//! The app's choices (docs/architecture.md §16a). Every choice is for the group, for you
//! (personal) or for one project; nothing is per device. The choices a person makes in the
//! app live in one personal file, `~/.claude/personal-state/claude-sync/choices.json`, which
//! personal sync carries to every device of the account (`state/claude-sync/choices.json`):
//!
//! ```json
//! { "groups": { "owner/repo": { "off": true, "reviewClaudeMd": false, "autoRunJobs": true, "ts": 1760000000000 } },
//!   "personal": { "off": true, "ts": 1760000000000 } }
//! ```
//!
//! `ts` is Unix milliseconds. Two devices' files merge per key: the newer `ts` wins (see
//! [`merge`]). Without personal sync the file is simply the local store.
//!
//! Older versions kept these per device (`state.json` `disabled`, `jobs.json` `autoAccept`,
//! `claude-md-review.json` `review`). The first [`load`] with no file copies those values in;
//! after that the old fields are never read.
use crate::jobs::JobsState;
use crate::review::{self, ReviewState};
use crate::state::{now_unix, write_json, AppState};
use crate::{GroupId, Paths};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Path inside the personal repo.
pub const REPO_PATH: &str = "state/claude-sync/choices.json";

pub fn file(paths: &Paths) -> PathBuf {
    paths
        .claude_home
        .join(crate::personal::LOCAL_STATE_DIR)
        .join("claude-sync")
        .join("choices.json")
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupChoice {
    /// The group is turned off: nothing of it is applied or synced.
    pub off: bool,
    /// Review changes others make to the group's CLAUDE.md. `None` follows the group.
    pub review_claude_md: Option<bool>,
    /// Run the group's jobs without asking.
    pub auto_run_jobs: bool,
    /// Unix milliseconds of the last change.
    pub ts: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PersonalChoice {
    /// Personal sync is turned off on every device.
    pub off: bool,
    pub ts: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choices {
    pub groups: BTreeMap<String, GroupChoice>,
    pub personal: PersonalChoice,
    /// Keys a newer version wrote are kept.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Choices {
    /// The group's choices; the defaults when it has none.
    pub fn group(&self, id: &GroupId) -> GroupChoice {
        self.groups
            .get(&id.to_string())
            .cloned()
            .unwrap_or_default()
    }
}

/// Unix milliseconds, and always later than `after` so a change made in the same
/// millisecond as the last one still wins.
fn stamp(after: u64) -> u64 {
    now_ms().max(after + 1)
}

/// Unix milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(now_unix() * 1000, |d| d.as_millis() as u64)
}

fn read(paths: &Paths) -> crate::Result<Option<Choices>> {
    match std::fs::read(file(paths)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The choices. With no file yet, the per-device values older versions kept are copied in
/// (and saved, when there are any).
pub fn load(paths: &Paths) -> crate::Result<Choices> {
    if let Some(c) = read(paths)? {
        return Ok(c);
    }
    let c = legacy(paths)?;
    if !c.groups.is_empty() {
        write_json(&file(paths), &c)?;
    }
    Ok(c)
}

/// What the old per-device files said. `ts` 0: any real choice made elsewhere is newer.
fn legacy(paths: &Paths) -> crate::Result<Choices> {
    let state = AppState::load(paths)?;
    let jobs = JobsState::load(paths).unwrap_or_default();
    let mut c = Choices::default();
    for id in &state.groups {
        let g = GroupChoice {
            off: state.disabled.contains(id),
            review_claude_md: ReviewState::load(&paths.group_dir(id).join(review::FILE))
                .ok()
                .flatten()
                .and_then(|r| r.review),
            auto_run_jobs: jobs.auto_accept.contains(id),
            ts: 0,
        };
        if g != GroupChoice::default() {
            c.groups.insert(id.to_string(), g);
        }
    }
    Ok(c)
}

/// Change one group's choices and save.
pub fn set_group(
    paths: &Paths,
    id: &GroupId,
    change: impl FnOnce(&mut GroupChoice),
) -> crate::Result<Choices> {
    let mut c = load(paths)?;
    let g = c.groups.entry(id.to_string()).or_default();
    let before = g.ts;
    change(g);
    g.ts = stamp(before);
    write_json(&file(paths), &c)?;
    Ok(c)
}

/// Turn personal sync off (or on again) for every device.
pub fn set_personal_off(paths: &Paths, off: bool) -> crate::Result<Choices> {
    let mut c = load(paths)?;
    c.personal = PersonalChoice {
        off,
        ts: stamp(c.personal.ts),
    };
    write_json(&file(paths), &c)?;
    Ok(c)
}

/// A group moved to a new `owner/repo`: its choices move with it.
pub fn rename_group(paths: &Paths, old: &GroupId, new: &GroupId) -> crate::Result<()> {
    let mut c = load(paths)?;
    if let Some(mut g) = c.groups.remove(&old.to_string()) {
        g.ts = stamp(g.ts);
        c.groups.entry(new.to_string()).or_insert(g);
        write_json(&file(paths), &c)?;
    }
    Ok(())
}

/// Two devices' files merged key by key: the entry with the newer `ts` wins, the remote's
/// on a tie. `None` when either side is not a choices file (then the normal merge decides).
pub fn merge(local: &[u8], remote: &[u8]) -> Option<Vec<u8>> {
    let l: Choices = serde_json::from_slice(local).ok()?;
    let mut out: Choices = serde_json::from_slice(remote).ok()?;
    for (k, lg) in l.groups {
        match out.groups.get(&k) {
            Some(rg) if rg.ts >= lg.ts => {}
            _ => {
                out.groups.insert(k, lg);
            }
        }
    }
    if l.personal.ts > out.personal.ts {
        out.personal = l.personal;
    }
    let mut bytes = serde_json::to_vec_pretty(&out).ok()?;
    bytes.push(b'\n');
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().unwrap();
        let p = Paths {
            claude_home: tmp.path().join(".claude"),
            state_dir: tmp.path().join(".claude-sync"),
        };
        (tmp, p)
    }

    fn id(s: &str) -> GroupId {
        GroupId::parse(s).unwrap()
    }

    #[test]
    fn set_and_load_round_trip() {
        let (_t, p) = paths();
        assert_eq!(load(&p).unwrap(), Choices::default());
        assert!(!file(&p).exists(), "nothing to save yet");
        let g = id("a/b");
        set_group(&p, &g, |c| c.off = true).unwrap();
        set_group(&p, &g, |c| c.review_claude_md = Some(false)).unwrap();
        set_group(&p, &g, |c| c.auto_run_jobs = true).unwrap();
        let c = load(&p).unwrap().group(&g);
        assert!(c.off && c.auto_run_jobs && c.review_claude_md == Some(false) && c.ts > 0);
        assert_eq!(
            load(&p).unwrap().group(&id("a/other")),
            GroupChoice::default()
        );
        assert!(file(&p).ends_with(
            std::path::Path::new("personal-state")
                .join("claude-sync")
                .join(file(&p).file_name().unwrap())
        ));
    }

    #[test]
    fn migrates_each_old_per_device_file_once() {
        let (_t, p) = paths();
        let (g1, g2, g3) = (id("a/one"), id("a/two"), id("a/three"));
        let mut st = AppState {
            groups: vec![g1.clone(), g2.clone(), g3.clone()],
            ..AppState::default()
        };
        st.disabled.insert(g1.clone());
        st.save(&p).unwrap();
        let mut jobs = JobsState::default();
        jobs.auto_accept.insert(g2.clone());
        jobs.save(&p).unwrap();
        let mut rs = ReviewState::new("x");
        rs.review = Some(false);
        std::fs::create_dir_all(p.group_dir(&g3)).unwrap();
        rs.save(&p.group_dir(&g3).join(review::FILE)).unwrap();

        let c = load(&p).unwrap();
        assert!(c.group(&g1).off && !c.group(&g1).auto_run_jobs);
        assert!(c.group(&g2).auto_run_jobs && !c.group(&g2).off);
        assert_eq!(c.group(&g3).review_claude_md, Some(false));
        assert!(file(&p).exists());

        // The old files are not read again.
        st.disabled.clear();
        st.save(&p).unwrap();
        assert!(load(&p).unwrap().group(&g1).off);
    }

    #[test]
    fn merge_keeps_the_newer_entry_per_key() {
        let mk = |entries: &[(&str, bool, u64)], off: bool, pts: u64| {
            let mut c = Choices::default();
            for (k, o, ts) in entries {
                c.groups.insert(
                    k.to_string(),
                    GroupChoice {
                        off: *o,
                        ts: *ts,
                        ..Default::default()
                    },
                );
            }
            c.personal = PersonalChoice { off, ts: pts };
            serde_json::to_vec(&c).unwrap()
        };
        let local = mk(&[("a/x", true, 5), ("a/y", false, 1)], false, 0);
        let remote = mk(
            &[("a/x", false, 3), ("a/y", true, 9), ("a/z", true, 2)],
            true,
            4,
        );
        let out: Choices = serde_json::from_slice(&merge(&local, &remote).unwrap()).unwrap();
        assert!(out.groups["a/x"].off, "local is newer");
        assert!(out.groups["a/y"].off, "remote is newer");
        assert!(out.groups["a/z"].off, "only remote has it");
        assert!(out.personal.off, "remote is newer");
        assert!(merge(b"nope", &remote).is_none());
    }
}
