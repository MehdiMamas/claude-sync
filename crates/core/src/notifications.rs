//! Notification history (§11): every notification the app shows is also kept in
//! `~/.claude-sync/notifications.json`, so a missed one can be read later in the app.
//!
//! Newest first, at most [`MAX_ENTRIES`]; read entries older than [`READ_KEEP_SECONDS`]
//! are dropped. A missing or unreadable file is an empty history, never an error: the
//! next write replaces it (a corrupt file is first copied to `notifications.json.bad`).
use crate::state::{now_unix, write_json};
use crate::{GroupId, Paths};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

pub const MAX_ENTRIES: usize = 200;
pub const READ_KEEP_SECONDS: u64 = 30 * 86_400;

/// What a notification is about. The UI shows a label and an icon for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// An app update.
    Update,
    /// Membership, invitations, conflicts and access of a group.
    Group,
    /// Plugins changed; open sessions need `/reload-plugins`.
    Plugins,
    /// A shared file was edited on this machine.
    LocalEdit,
    /// A group job ran.
    Job,
    /// Personal sync.
    Personal,
    /// Anything else (sign-in, feedback). Kinds this version does not know read as this.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    /// Unique within this history; never reused, even after `clear`.
    pub id: u64,
    /// When it was recorded, in UTC seconds since the Unix epoch.
    pub time: u64,
    pub title: String,
    pub body: String,
    pub kind: Kind,
    #[serde(default)]
    pub group: Option<GroupId>,
    #[serde(default)]
    pub read: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct File {
    next_id: u64,
    /// Newest first.
    items: Vec<Notification>,
}

/// One read-modify-write at a time within the app (the app is single-instance).
static LOCK: Mutex<()> = Mutex::new(());

fn load(paths: &Paths) -> File {
    let path = paths.notifications_file();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("could not read {}: {e}", path.display());
            }
            return File::default();
        }
    };
    match serde_json::from_slice::<File>(&bytes) {
        Ok(mut f) => {
            let max = f.items.iter().map(|n| n.id).max().unwrap_or(0);
            f.next_id = f.next_id.max(max + 1);
            f
        }
        Err(e) => {
            tracing::warn!(
                "{} is not valid, starting an empty history: {e}",
                path.display()
            );
            let _ = std::fs::copy(&path, path.with_extension("json.bad"));
            File::default()
        }
    }
}

fn prune(f: &mut File, now: u64) {
    f.items
        .retain(|n| !(n.read && now.saturating_sub(n.time) > READ_KEEP_SECONDS));
    f.items.truncate(MAX_ENTRIES);
}

/// Load, change, prune and save under the lock. Returns what `change` returned.
fn update<T>(paths: &Paths, change: impl FnOnce(&mut File) -> T) -> crate::Result<T> {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = load(paths);
    let out = change(&mut f);
    prune(&mut f, now_unix());
    write_json(&paths.notifications_file(), &f)?;
    Ok(out)
}

/// The group moved to a new `owner/repo`: its history entries follow.
pub fn rename_group(paths: &Paths, old: &GroupId, new: &GroupId) -> crate::Result<()> {
    update(paths, |f| {
        for n in f.items.iter_mut().filter(|n| n.group.as_ref() == Some(old)) {
            n.group = Some(new.clone());
        }
    })
}

/// Add a notification (unread, now) and return it.
pub fn record(
    paths: &Paths,
    kind: Kind,
    group: Option<GroupId>,
    title: &str,
    body: &str,
) -> crate::Result<Notification> {
    record_at(paths, now_unix(), kind, group, title, body)
}

fn record_at(
    paths: &Paths,
    time: u64,
    kind: Kind,
    group: Option<GroupId>,
    title: &str,
    body: &str,
) -> crate::Result<Notification> {
    update(paths, |f| {
        let n = Notification {
            id: f.next_id.max(1),
            time,
            title: title.into(),
            body: body.into(),
            kind,
            group,
            read: false,
        };
        f.next_id = n.id + 1;
        f.items.insert(0, n.clone());
        n
    })
}

/// The history, newest first.
pub fn list(paths: &Paths) -> Vec<Notification> {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = load(paths);
    prune(&mut f, now_unix());
    f.items
}

pub fn unread_count(paths: &Paths) -> usize {
    list(paths).iter().filter(|n| !n.read).count()
}

/// Returns false when there is no such notification.
pub fn mark_read(paths: &Paths, id: u64) -> crate::Result<bool> {
    update(paths, |f| match f.items.iter_mut().find(|n| n.id == id) {
        Some(n) => {
            n.read = true;
            true
        }
        None => false,
    })
}

/// Returns how many were unread.
pub fn mark_all_read(paths: &Paths) -> crate::Result<usize> {
    update(paths, |f| {
        let mut changed = 0;
        for n in f.items.iter_mut().filter(|n| !n.read) {
            n.read = true;
            changed += 1;
        }
        changed
    })
}

/// Remove every notification.
pub fn clear(paths: &Paths) -> crate::Result<()> {
    update(paths, |f| f.items.clear())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            claude_home: dir.path().join("c"),
            state_dir: dir.path().join("s"),
        };
        (dir, paths)
    }

    #[test]
    fn missing_file_is_empty() {
        let (_d, p) = temp();
        assert!(list(&p).is_empty());
        assert_eq!(unread_count(&p), 0);
        assert!(!mark_read(&p, 1).unwrap());
    }

    #[test]
    fn records_newest_first_and_round_trips() {
        let (_d, p) = temp();
        let g = GroupId::new("o", "r");
        let a = record(&p, Kind::Update, None, "Update ready", "v2").unwrap();
        let b = record(&p, Kind::Group, Some(g.clone()), "Access ended", "o/r").unwrap();
        assert!(b.id > a.id);
        let items = list(&p);
        assert_eq!(items, vec![b.clone(), a.clone()]);
        assert_eq!(items[0].group, Some(g));
        assert_eq!(unread_count(&p), 2);
        let json = std::fs::read_to_string(p.notifications_file()).unwrap();
        assert!(json.contains("\"kind\": \"group\""), "{json}");
        let leftovers: Vec<_> = std::fs::read_dir(&p.state_dir).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temp file left behind");
    }

    #[test]
    fn mark_read_and_mark_all() {
        let (_d, p) = temp();
        let a = record(&p, Kind::Other, None, "a", "").unwrap();
        record(&p, Kind::Other, None, "b", "").unwrap();
        record(&p, Kind::Other, None, "c", "").unwrap();
        assert!(mark_read(&p, a.id).unwrap());
        assert!(list(&p).iter().find(|n| n.id == a.id).unwrap().read);
        assert_eq!(unread_count(&p), 2);
        assert_eq!(mark_all_read(&p).unwrap(), 2);
        assert_eq!(unread_count(&p), 0);
        assert_eq!(mark_all_read(&p).unwrap(), 0);
    }

    #[test]
    fn clear_keeps_ids_unique() {
        let (_d, p) = temp();
        let a = record(&p, Kind::Other, None, "a", "").unwrap();
        clear(&p).unwrap();
        assert!(list(&p).is_empty());
        let b = record(&p, Kind::Other, None, "b", "").unwrap();
        assert!(b.id > a.id);
    }

    #[test]
    fn capped_and_old_read_entries_dropped() {
        let (_d, p) = temp();
        let now = now_unix();
        let old = record_at(
            &p,
            now - READ_KEEP_SECONDS - 10,
            Kind::Other,
            None,
            "old",
            "",
        )
        .unwrap();
        let old_unread = record_at(
            &p,
            now - READ_KEEP_SECONDS - 5,
            Kind::Other,
            None,
            "kept",
            "",
        )
        .unwrap();
        mark_read(&p, old.id).unwrap();
        let ids: Vec<_> = list(&p).iter().map(|n| n.id).collect();
        assert_eq!(
            ids,
            vec![old_unread.id],
            "old read entry dropped, old unread kept"
        );

        for i in 0..MAX_ENTRIES + 5 {
            record(&p, Kind::Other, None, &format!("n{i}"), "").unwrap();
        }
        let items = list(&p);
        assert_eq!(items.len(), MAX_ENTRIES);
        assert_eq!(items[0].title, format!("n{}", MAX_ENTRIES + 4));
    }

    #[test]
    fn corrupt_file_is_empty_and_kept_aside() {
        let (_d, p) = temp();
        std::fs::create_dir_all(&p.state_dir).unwrap();
        std::fs::write(p.notifications_file(), b"{ not json").unwrap();
        assert!(list(&p).is_empty());
        let n = record(&p, Kind::Plugins, None, "Plugins changed", "").unwrap();
        assert_eq!(list(&p), vec![n]);
        assert!(p.notifications_file().with_extension("json.bad").exists());
    }

    #[test]
    fn unknown_kind_reads_as_other() {
        let (_d, p) = temp();
        std::fs::create_dir_all(&p.state_dir).unwrap();
        let json = r#"{"nextId": 0, "items": [{"id": 4, "time": 1, "title": "t", "body": "b", "kind": "from-the-future"}]}"#;
        std::fs::write(p.notifications_file(), json).unwrap();
        let items = list(&p);
        assert_eq!(items[0].kind, Kind::Other);
        assert!(!items[0].read);
        let n = record(&p, Kind::Job, None, "j", "").unwrap();
        assert_eq!(n.id, 5, "ids continue past the ones on disk");
    }
}
