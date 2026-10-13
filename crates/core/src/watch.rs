//! Local edits to managed files (§7 "Local edits", issue #18).
//!
//! [`local_edits`] compares what is on disk with what the app last applied. [`spawn_watcher`]
//! calls back (debounced, 2 s) when a managed path or `settings.json` may have changed, so
//! the app can call `local_edits` again. [`resolve`] runs the user's choice.
use crate::apply::{self, Manifest, CLAUDE_MD, SETTINGS_POINTER_PREFIX};
use crate::{settings, GroupId, Paths};
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEdit {
    pub group: GroupId,
    /// A managed file (`agents/x.md`), `CLAUDE.md` (the group's copy), or one settings
    /// value: `settings.shared.json#/permissions/allow`.
    pub path: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditAction {
    Share,
    KeepLocal,
    Undo,
}

/// Managed files whose content differs from the manifest, edited `CLAUDE.md` copies, and
/// settings values the user changed after the app set them. Sorted by group, then path.
pub fn local_edits(paths: &Paths) -> crate::Result<Vec<LocalEdit>> {
    let m = Manifest::load(paths)?;
    let mut out = Vec::new();
    for (rel, mf) in &m.files {
        let target = rel
            .split('/')
            .fold(paths.claude_home.clone(), |p, s| p.join(s));
        match read_file(&target)? {
            None => out.push(edit(&mf.group, rel, true)),
            Some(b) if apply::sha256_hex(&b) != mf.sha256 => out.push(edit(&mf.group, rel, false)),
            Some(_) => {}
        }
    }
    for (key, entry) in &m.groups {
        let Some(group) = GroupId::parse(key) else {
            continue;
        };
        let Some(sha) = &entry.claude_md else {
            continue;
        };
        if entry.keep_local.contains(CLAUDE_MD) {
            continue;
        }
        match read_file(&paths.group_dir(&group).join(CLAUDE_MD))? {
            None => out.push(edit(&group, CLAUDE_MD, true)),
            Some(b) if apply::sha256_hex(&b) != *sha => out.push(edit(&group, CLAUDE_MD, false)),
            Some(_) => {}
        }
    }
    let owned = apply::load_owned(paths)?;
    if !owned.is_empty() {
        let local = std::fs::read(paths.claude_home.join("settings.json"))
            .ok()
            .and_then(|b| settings::parse_object(&b));
        if let Some(local) = local {
            for (key, o) in &owned {
                let Some(group) = GroupId::parse(key) else {
                    continue;
                };
                for (ptr, deleted) in settings::edited_pointers(&local, o) {
                    out.push(edit(
                        &group,
                        &format!("{SETTINGS_POINTER_PREFIX}{ptr}"),
                        deleted,
                    ));
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.group, &a.path).cmp(&(&b.group, &b.path)));
    Ok(out)
}

fn edit(group: &GroupId, path: &str, deleted: bool) -> LocalEdit {
    LocalEdit {
        group: group.clone(),
        path: path.to_string(),
        deleted,
    }
}

fn read_file(p: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::symlink_metadata(p) {
        Ok(md) if md.is_file() => std::fs::read(p).map(Some),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Run the user's choice for one edit on this device.
/// - `KeepLocal`: stop managing it here (`apply::keep_local`).
/// - `Undo`: put the group's version back (`apply::undo`); `group_version` is the group's
///   current content of `edit.path` (`None` if the group does not have it). Settings
///   values are restored from the ownership record.
/// - `Share` is not a local change: the engine pushes `apply::local_files`, which already
///   carries the edit, and the next apply records the shared version. Returns `Invalid`.
pub fn resolve(
    paths: &Paths,
    edit: &LocalEdit,
    action: EditAction,
    group_version: Option<&[u8]>,
) -> crate::Result<()> {
    match action {
        EditAction::KeepLocal => apply::keep_local(paths, &edit.group, &edit.path),
        EditAction::Undo => apply::undo(paths, &edit.group, &edit.path, group_version),
        EditAction::Share => Err(crate::Error::Invalid(
            "Share is done by the engine: it pushes apply::local_files".into(),
        )),
    }
}

/// How long the watcher waits for changes to settle (§7).
pub const DEBOUNCE: Duration = Duration::from_secs(2);

const TREES: [&str; 4] = ["skills", "agents", "commands", "output-styles"];

/// Keeps the watcher running; dropping it stops it.
pub struct WatchHandle {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    debouncer: Debouncer<RecommendedWatcher>,
    paths: Paths,
    watched: BTreeSet<PathBuf>,
}

impl WatchHandle {
    /// Start watching folders that appeared since the last call. The watcher does this by
    /// itself when it sees one created; calling it after an apply is harmless.
    pub fn refresh(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.refresh();
        }
    }
}

impl Inner {
    fn refresh(&mut self) {
        let home = &self.paths.claude_home;
        let mut want = vec![
            (home.clone(), RecursiveMode::NonRecursive),
            (self.paths.state_dir.clone(), RecursiveMode::NonRecursive),
            (
                self.paths.state_dir.join("groups"),
                RecursiveMode::Recursive,
            ),
        ];
        want.extend(
            TREES
                .iter()
                .map(|t| (home.join(t), RecursiveMode::Recursive)),
        );
        for (dir, mode) in want {
            let is_dir = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir());
            if self.watched.contains(&dir) {
                if !is_dir {
                    // Deleted: the OS watch is gone; watch again when it comes back.
                    let _ = self.debouncer.watcher().unwatch(&dir);
                    self.watched.remove(&dir);
                }
                continue;
            }
            if !is_dir {
                continue;
            }
            match self.debouncer.watcher().watch(&dir, mode) {
                Ok(()) => {
                    self.watched.insert(dir);
                }
                Err(e) => {
                    tracing::warn!(path = %dir.display(), error = %e, "could not watch folder")
                }
            }
        }
    }
}

/// Watch the managed folders, `~/.claude/settings.json`, `~/.claude/CLAUDE.md` and the
/// groups' `CLAUDE.md` copies. `on_change` runs on a background thread at most once per
/// settled batch of changes (2 s). The app's own writes trigger it too; `local_edits` then
/// finds nothing. Folders that do not exist yet are picked up when they are created.
pub fn spawn_watcher(
    paths: &Paths,
    on_change: impl Fn() + Send + 'static,
) -> crate::Result<WatchHandle> {
    spawn_with(paths, DEBOUNCE, on_change)
}

pub(crate) fn spawn_with(
    paths: &Paths,
    debounce: Duration,
    on_change: impl Fn() + Send + 'static,
) -> crate::Result<WatchHandle> {
    let (tx, rx) = mpsc::channel::<DebounceEventResult>();
    let debouncer = new_debouncer(debounce, tx)
        .map_err(|e| crate::Error::Invalid(format!("could not start the file watcher: {e}")))?;
    let inner = Arc::new(Mutex::new(Inner {
        debouncer,
        paths: paths.clone(),
        watched: BTreeSet::new(),
    }));
    inner.lock().expect("new mutex").refresh();
    let weak: Weak<Mutex<Inner>> = Arc::downgrade(&inner);
    let paths = paths.clone();
    std::thread::Builder::new()
        .name("claude-sync-watch".into())
        .spawn(move || {
            // Ends when the handle is dropped: the debouncer and its sender go with it.
            for res in rx {
                let events = match res {
                    Ok(ev) => ev,
                    Err(e) => {
                        tracing::warn!(error = %e, "file watcher error");
                        continue;
                    }
                };
                let refresh = events.iter().any(|e| new_watch_root(&paths, &e.path));
                let changed = events.iter().any(|e| relevant(&paths, &e.path));
                if refresh {
                    if let Some(inner) = weak.upgrade() {
                        if let Ok(mut i) = inner.lock() {
                            i.refresh();
                        }
                    }
                }
                if changed {
                    on_change();
                }
            }
        })
        .map_err(crate::Error::Io)?;
    Ok(WatchHandle { inner })
}

/// A path whose change can mean a local edit.
pub(crate) fn relevant(paths: &Paths, p: &Path) -> bool {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    if name.contains(".claude-sync-tmp-") {
        return false;
    }
    if let Ok(rel) = p.strip_prefix(&paths.claude_home) {
        let segs = segments(rel);
        return match segs.first().map(String::as_str) {
            Some("settings.json") | Some(CLAUDE_MD) => segs.len() == 1,
            Some(t) => TREES.contains(&t),
            None => false,
        };
    }
    if let Ok(rel) = p.strip_prefix(paths.state_dir.join("groups")) {
        let segs = segments(rel);
        return segs.len() == 2 && segs[1] == CLAUDE_MD;
    }
    false
}

/// A folder the watcher should start watching now that it exists.
fn new_watch_root(paths: &Paths, p: &Path) -> bool {
    if let Ok(rel) = p.strip_prefix(&paths.claude_home) {
        let segs = segments(rel);
        return segs.len() == 1 && TREES.contains(&segs[0].as_str());
    }
    p == paths.state_dir.join("groups")
}

fn segments(rel: &Path) -> Vec<String> {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::{apply_group, local_files, SHARED_SETTINGS};
    use crate::FileSet;
    use serde_json::json;
    use std::fs;

    fn env() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            claude_home: tmp.path().join(".claude"),
            state_dir: tmp.path().join(".claude-sync"),
        };
        (tmp, paths)
    }

    fn g(name: &str) -> GroupId {
        GroupId::new("owner", name)
    }

    fn files() -> FileSet {
        [
            ("agents/reviewer.md", &b"reviewer\r\n"[..]),
            ("skills/alpha/SKILL.md", b"alpha\n"),
            (CLAUDE_MD, b"rules\n"),
            (
                SHARED_SETTINGS,
                br#"{"model": "opus", "permissions": {"allow": ["Read"]}}"#,
            ),
        ]
        .iter()
        .map(|(p, b)| (p.to_string(), b.to_vec()))
        .collect()
    }

    fn setup() -> (tempfile::TempDir, Paths, GroupId) {
        let (tmp, paths) = env();
        let a = g("a");
        apply_group(&paths, &a, std::slice::from_ref(&a), &files()).unwrap();
        (tmp, paths, a)
    }

    fn write(p: &Path, b: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b).unwrap();
    }

    fn set_settings(paths: &Paths, v: serde_json::Value) {
        write(
            &paths.claude_home.join("settings.json"),
            &settings::to_bytes(&v),
        );
    }

    #[test]
    fn nothing_edited_right_after_apply() {
        let (_t, paths, _a) = setup();
        assert_eq!(local_edits(&paths).unwrap(), vec![]);
    }

    #[test]
    fn edits_and_deletes_are_listed() {
        let (_t, paths, a) = setup();
        write(
            &paths.claude_home.join("agents").join("reviewer.md"),
            b"reviewer\n",
        );
        fs::remove_file(paths.claude_home.join("skills/alpha/SKILL.md")).unwrap();
        write(&paths.group_dir(&a).join(CLAUDE_MD), b"rules, mine\n");
        set_settings(
            &paths,
            json!({"model": "haiku", "permissions": {"allow": []}}),
        );
        let got = local_edits(&paths).unwrap();
        assert_eq!(
            got,
            vec![
                edit(&a, CLAUDE_MD, false),
                edit(&a, "agents/reviewer.md", false),
                edit(&a, "settings.shared.json#/model", false),
                edit(&a, "settings.shared.json#/permissions/allow", false),
                edit(&a, "skills/alpha/SKILL.md", true),
            ]
        );
        // A CRLF -> LF change counts: content is compared byte for byte.
        assert!(got.iter().any(|e| e.path == "agents/reviewer.md"));
    }

    #[test]
    fn a_deleted_settings_value_is_listed_as_deleted() {
        let (_t, paths, a) = setup();
        set_settings(&paths, json!({"permissions": {"allow": ["Read"]}}));
        assert_eq!(
            local_edits(&paths).unwrap(),
            vec![edit(&a, "settings.shared.json#/model", true)]
        );
    }

    #[test]
    fn keep_local_removes_the_edit_and_keeps_the_content() {
        let (_t, paths, a) = setup();
        let p = paths.claude_home.join("agents/reviewer.md");
        write(&p, b"mine\n");
        set_settings(
            &paths,
            json!({"model": "haiku", "permissions": {"allow": ["Read"]}}),
        );
        for e in local_edits(&paths).unwrap() {
            resolve(&paths, &e, EditAction::KeepLocal, None).unwrap();
        }
        assert_eq!(local_edits(&paths).unwrap(), vec![]);
        assert_eq!(fs::read(&p).unwrap(), b"mine\n");
        // The group's next version does not overwrite it.
        let mut f = files();
        f.insert("agents/reviewer.md".into(), b"v2\n".to_vec());
        f.insert(SHARED_SETTINGS.into(), br#"{"model": "fable"}"#.to_vec());
        apply_group(&paths, &a, std::slice::from_ref(&a), &f).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"mine\n");
        let s: serde_json::Value =
            serde_json::from_slice(&fs::read(paths.claude_home.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(s["model"], json!("haiku"));
        assert_eq!(local_edits(&paths).unwrap(), vec![]);
    }

    #[test]
    fn undo_restores_the_group_version() {
        let (_t, paths, a) = setup();
        let p = paths.claude_home.join("agents/reviewer.md");
        write(&p, b"oops\n");
        fs::remove_file(paths.claude_home.join("skills/alpha/SKILL.md")).unwrap();
        write(&paths.group_dir(&a).join(CLAUDE_MD), b"oops\n");
        set_settings(&paths, json!({"permissions": {"allow": []}, "mine": 1}));
        let group = files();
        for e in local_edits(&paths).unwrap() {
            let key = e.path.split('#').next().unwrap();
            resolve(
                &paths,
                &e,
                EditAction::Undo,
                group.get(key).map(Vec::as_slice),
            )
            .unwrap();
        }
        assert_eq!(local_edits(&paths).unwrap(), vec![]);
        assert_eq!(fs::read(&p).unwrap(), b"reviewer\r\n");
        assert_eq!(
            fs::read(paths.claude_home.join("skills/alpha/SKILL.md")).unwrap(),
            b"alpha\n"
        );
        let s: serde_json::Value =
            serde_json::from_slice(&fs::read(paths.claude_home.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            s,
            json!({"model": "opus", "permissions": {"allow": ["Read"]}, "mine": 1})
        );
    }

    #[test]
    fn share_goes_through_local_files_and_clears_after_the_next_apply() {
        let (_t, paths, a) = setup();
        write(&paths.claude_home.join("agents/reviewer.md"), b"better\n");
        set_settings(
            &paths,
            json!({"model": "haiku", "permissions": {"allow": ["Read"]}}),
        );
        let edits = local_edits(&paths).unwrap();
        assert_eq!(edits.len(), 2);
        assert!(matches!(
            resolve(&paths, &edits[0], EditAction::Share, None),
            Err(crate::Error::Invalid(_))
        ));
        // What the engine pushes carries both edits.
        let local = local_files(&paths, &a).unwrap();
        assert_eq!(local["agents/reviewer.md"], b"better\n");
        let shared: serde_json::Value = serde_json::from_slice(&local[SHARED_SETTINGS]).unwrap();
        assert_eq!(shared["model"], json!("haiku"));
        // The group now ships them: applying records the shared version, nothing is left.
        apply_group(&paths, &a, std::slice::from_ref(&a), &local).unwrap();
        assert_eq!(local_edits(&paths).unwrap(), vec![]);
        assert_eq!(
            fs::read(paths.claude_home.join("agents/reviewer.md")).unwrap(),
            b"better\n"
        );
    }

    #[test]
    fn relevant_paths() {
        let (_t, paths) = env();
        let h = &paths.claude_home;
        assert!(relevant(&paths, &h.join("settings.json")));
        assert!(relevant(&paths, &h.join(CLAUDE_MD)));
        assert!(relevant(
            &paths,
            &h.join("skills").join("a").join("SKILL.md")
        ));
        assert!(relevant(&paths, &h.join("agents")));
        assert!(relevant(&paths, &paths.group_dir(&g("a")).join(CLAUDE_MD)));
        assert!(!relevant(&paths, &h.join("projects").join("x.jsonl")));
        assert!(!relevant(&paths, &h.join("settings.local.json")));
        assert!(!relevant(
            &paths,
            &h.join(".settings.json.claude-sync-tmp-12")
        ));
        assert!(!relevant(&paths, &paths.state_dir.join("manifest.json")));
        assert!(!relevant(
            &paths,
            &paths.group_dir(&g("a")).join("base").join("x.md")
        ));
    }

    fn wait(rx: &mpsc::Receiver<()>) -> bool {
        rx.recv_timeout(Duration::from_secs(15)).is_ok()
    }

    fn drain(rx: &mpsc::Receiver<()>) {
        while rx.recv_timeout(Duration::from_millis(700)).is_ok() {}
    }

    #[test]
    fn watcher_reports_changes_to_managed_paths() {
        let (_t, paths, _a) = setup();
        let (tx, rx) = mpsc::channel();
        let handle = spawn_with(&paths, Duration::from_millis(200), move || {
            let _ = tx.send(());
        })
        .unwrap();
        drain(&rx);
        write(&paths.claude_home.join("agents/reviewer.md"), b"edited\n");
        assert!(wait(&rx), "no callback for a managed file");
        drain(&rx);
        set_settings(&paths, json!({"model": "haiku"}));
        assert!(wait(&rx), "no callback for settings.json");
        drain(&rx);
        // A tree folder created after start is picked up.
        fs::create_dir_all(paths.claude_home.join("commands")).unwrap();
        assert!(wait(&rx), "no callback for a new commands folder");
        drain(&rx);
        handle.refresh();
        write(&paths.claude_home.join("commands/new.md"), b"x\n");
        assert!(wait(&rx), "no callback inside the new folder");
        drop(handle);
    }
}
