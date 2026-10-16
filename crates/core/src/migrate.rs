//! One-time move off v0 (§10): `~/.claude-sync/` was a git clone of the personal repo.
//!
//! Steps, in an order that is safe to stop at any point: the clone's `.git` folder is
//! moved last, and it is what marks the migration as not done. A rerun picks up where the
//! last run stopped. Nothing is ever deleted: the v0 files go to
//! `~/.claude-sync-v0-backup-<UTC time>/` and `settings.json` is copied there first.
use serde_json::Value;
use std::path::{Path, PathBuf};

const BACKUP_PREFIX: &str = ".claude-sync-v0-backup-";
/// Present in a backup folder while its migration is unfinished, so a rerun reuses it.
const MARKER: &str = ".migrating";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migrated {
    pub backup: PathBuf,
}

impl Migrated {
    pub fn message(&self) -> String {
        format!(
            "Moved your old claude-sync setup to {}. Turn on Personal memory in Settings to sync your CLAUDE.md and memories.",
            self.backup.display()
        )
    }
}

/// Migrate when `<home>/.claude-sync/.git` exists. `Ok(None)` when there is nothing to do.
pub fn migrate_v0(home: &Path, claude_home: &Path) -> crate::Result<Option<Migrated>> {
    let sync = home.join(".claude-sync");
    if !sync.join(".git").exists() {
        return Ok(None);
    }
    let backup = backup_dir(home)?;
    std::fs::create_dir_all(&backup)?;
    // The names the app keeps in `app/`, written once: after a stop half way, files already
    // moved up look like v0's, and this list tells them apart. A name v0 also has is not in
    // it, so v0's copy goes to the backup and the app's moves up.
    let marker = backup.join(MARKER);
    if !marker.exists() {
        let mut names = Vec::new();
        if let Ok(rd) = std::fs::read_dir(sync.join("app")) {
            for e in rd {
                let name = e?.file_name();
                if !sync.join(&name).exists() {
                    names.push(name.to_string_lossy().into_owned());
                }
            }
        }
        std::fs::write(
            &marker,
            names.join(
                "
",
            ),
        )?;
    }
    let app_names: Vec<String> = std::fs::read_to_string(&marker)?
        .lines()
        .map(String::from)
        .collect();

    remove_v0_hooks(&claude_home.join("settings.json"), &backup)?;

    // Everything of v0's except `app/` (the app's own state) and `.git` (moved last).
    let moved = backup.join("claude-sync");
    std::fs::create_dir_all(&moved)?;
    for entry in std::fs::read_dir(&sync)? {
        let name = entry?.file_name();
        if name == "app"
            || name == ".git"
            || app_names
                .iter()
                .any(|n| name.to_string_lossy() == n.as_str())
        {
            continue;
        }
        move_entry(&sync.join(&name), &moved.join(&name))?;
    }

    let app = sync.join("app");
    if app.is_dir() {
        for entry in std::fs::read_dir(&app)? {
            let name = entry?.file_name();
            let to = sync.join(&name);
            if to.exists() {
                // A name v0 and the app share; keep the app's copy with the backup.
                move_entry(&app.join(&name), &backup.join("app").join(&name))?;
            } else {
                move_entry(&app.join(&name), &to)?;
            }
        }
        // Only an empty folder is removed; anything left stays.
        let _ = std::fs::remove_dir(&app);
    }

    move_entry(&sync.join(".git"), &moved.join(".git"))?;
    let _ = std::fs::remove_file(backup.join(MARKER));
    Ok(Some(Migrated { backup }))
}

/// An unfinished backup folder from an earlier run, else a new one named by UTC time.
fn backup_dir(home: &Path) -> crate::Result<PathBuf> {
    let mut names: Vec<_> = std::fs::read_dir(home)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with(BACKUP_PREFIX)
                && e.path().join(MARKER).exists()
        })
        .map(|e| e.path())
        .collect();
    names.sort();
    if let Some(p) = names.pop() {
        return Ok(p);
    }
    let base = format!("{BACKUP_PREFIX}{}", crate::apply::utc_stamp());
    let mut p = home.join(&base);
    let mut n = 1;
    while p.exists() {
        n += 1;
        p = home.join(format!("{base}-{n}"));
    }
    Ok(p)
}

fn move_entry(from: &Path, to: &Path) -> crate::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Different drive or a locked file: copy, then remove the source.
    copy_all(from, to)?;
    if from.is_dir() {
        std::fs::remove_dir_all(from)?;
    } else {
        std::fs::remove_file(from)?;
    }
    Ok(())
}

fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_all(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Is this hook command v0's `claude-sync pull|push --hook`?
fn is_v0_hook(command: &str) -> bool {
    command.contains("claude-sync") && command.contains("--hook")
}

/// Remove v0's hook entries from `settings.json`, after copying the file into `backup`.
/// Other hooks and every other key stay as they are.
fn remove_v0_hooks(settings: &Path, backup: &Path) -> crate::Result<()> {
    let Ok(bytes) = std::fs::read(settings) else {
        return Ok(());
    };
    let Some(mut root) = crate::settings::parse_object(&bytes) else {
        return Ok(());
    };
    let mut changed = false;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        for groups in hooks.values_mut() {
            let Some(groups) = groups.as_array_mut() else {
                continue;
            };
            for g in groups.iter_mut() {
                if let Some(list) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                    let before = list.len();
                    list.retain(|h| {
                        !h.get("command")
                            .and_then(Value::as_str)
                            .is_some_and(is_v0_hook)
                    });
                    changed |= list.len() != before;
                }
            }
            let before = groups.len();
            groups.retain(|g| {
                g.get("hooks")
                    .and_then(Value::as_array)
                    .is_none_or(|l| !l.is_empty())
            });
            changed |= groups.len() != before;
        }
        hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    }
    if !changed {
        return Ok(());
    }
    if root
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|h| h.is_empty())
    {
        if let Some(o) = root.as_object_mut() {
            o.remove("hooks");
        }
    }
    std::fs::copy(settings, backup.join("settings.json.before-migration"))?;
    std::fs::write(settings, crate::settings::to_bytes(&root))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Env {
        home: tempfile::TempDir,
    }
    impl Env {
        fn new() -> Self {
            let home = tempfile::tempdir().unwrap();
            let s = home.path().join(".claude-sync");
            std::fs::create_dir_all(s.join(".git")).unwrap();
            std::fs::write(s.join(".git/HEAD"), "ref").unwrap();
            std::fs::create_dir_all(s.join("skills")).unwrap();
            std::fs::write(s.join("skills/a.md"), "skill").unwrap();
            std::fs::write(s.join("CLAUDE.md"), "mine").unwrap();
            std::fs::create_dir_all(s.join("app/groups")).unwrap();
            std::fs::write(s.join("app/state.json"), "{}").unwrap();
            std::fs::write(s.join("app/groups/g"), "g").unwrap();
            let c = home.path().join(".claude");
            std::fs::create_dir_all(&c).unwrap();
            std::fs::write(c.join("CLAUDE.md"), "user file").unwrap();
            let settings = json!({
              "model": "x",
              "hooks": {
                "SessionStart": [{"hooks": [
                  {"type":"command","command":"bash /h/.claude-sync/bin/claude-sync pull --hook"},
                  {"type":"command","command":"echo keep"}]}],
                "Stop": [{"hooks": [
                  {"type":"command","command":"bash /h/.claude-sync/bin/claude-sync push --hook"}]}]
              }
            });
            std::fs::write(c.join("settings.json"), settings.to_string()).unwrap();
            Self { home }
        }
        fn run(&self) -> Option<Migrated> {
            migrate_v0(self.home.path(), &self.home.path().join(".claude")).unwrap()
        }
        fn sync(&self) -> PathBuf {
            self.home.path().join(".claude-sync")
        }
        fn settings(&self) -> Value {
            serde_json::from_slice(
                &std::fs::read(self.home.path().join(".claude/settings.json")).unwrap(),
            )
            .unwrap()
        }
        fn backups(&self) -> Vec<PathBuf> {
            std::fs::read_dir(self.home.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with(BACKUP_PREFIX))
                .map(|e| e.path())
                .collect()
        }
    }

    #[test]
    fn fresh_install_does_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert!(migrate_v0(home.path(), &home.path().join(".claude"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn migrates_a_v0_clone() {
        let e = Env::new();
        let m = e.run().expect("migrated");
        assert!(m.message().contains("Turn on Personal memory"));
        // The app's state is now where the normal layout wants it.
        assert!(e.sync().join("state.json").is_file());
        assert!(e.sync().join("groups/g").is_file());
        assert!(!e.sync().join(".git").exists());
        assert!(!e.sync().join("app").exists());
        assert!(!e.sync().join("CLAUDE.md").exists());
        // v0's files are kept, not deleted.
        assert!(m.backup.join("claude-sync/.git/HEAD").is_file());
        assert!(m.backup.join("claude-sync/CLAUDE.md").is_file());
        assert!(m.backup.join("claude-sync/skills/a.md").is_file());
        assert!(!m.backup.join(MARKER).exists());
        // ~/.claude is left alone; only v0's hooks are gone, and the old file is saved.
        assert_eq!(
            std::fs::read_to_string(e.home.path().join(".claude/CLAUDE.md")).unwrap(),
            "user file"
        );
        let s = e.settings();
        assert_eq!(s["model"], "x");
        assert_eq!(
            s["hooks"],
            json!({"SessionStart": [{"hooks": [{"type":"command","command":"echo keep"}]}]})
        );
        assert!(m.backup.join("settings.json.before-migration").is_file());
        // The normal layout now applies.
        let p = crate::paths::Paths::resolve(None, None, Some(e.home.path())).unwrap();
        assert_eq!(p.state_dir, e.sync());
    }

    #[test]
    fn already_migrated_is_a_no_op() {
        let e = Env::new();
        assert!(e.run().is_some());
        let before = e.backups();
        assert!(e.run().is_none());
        assert_eq!(e.backups(), before);
        assert!(e.sync().join("state.json").is_file());
    }

    #[test]
    fn interrupted_run_is_finished_by_the_next() {
        let e = Env::new();
        // A first run that died after hooks and the v0 files, with one app file moved.
        let backup = e
            .home
            .path()
            .join(format!("{BACKUP_PREFIX}20260101T000000Z"));
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(
            backup.join(MARKER),
            "groups
state.json",
        )
        .unwrap();
        remove_v0_hooks(&e.home.path().join(".claude/settings.json"), &backup).unwrap();
        move_entry(
            &e.sync().join("CLAUDE.md"),
            &backup.join("claude-sync/CLAUDE.md"),
        )
        .unwrap();
        move_entry(
            &e.sync().join("app/state.json"),
            &e.sync().join("state.json"),
        )
        .unwrap();
        assert!(e.sync().join(".git").exists());

        let m = e.run().expect("migrated");
        assert_eq!(m.backup, backup, "the unfinished backup is reused");
        assert_eq!(e.backups().len(), 1);
        assert!(m.backup.join("claude-sync/CLAUDE.md").is_file());
        assert!(m.backup.join("claude-sync/skills/a.md").is_file());
        assert!(m.backup.join("claude-sync/.git/HEAD").is_file());
        assert!(e.sync().join("state.json").is_file());
        assert!(e.sync().join("groups/g").is_file());
        assert!(!e.sync().join("app").exists());
        assert!(!e.sync().join(".git").exists());
        assert!(m.backup.join("settings.json.before-migration").is_file());
        assert!(e.run().is_none());
    }

    #[test]
    fn invalid_settings_json_is_left_alone() {
        let e = Env::new();
        let f = e.home.path().join(".claude/settings.json");
        std::fs::write(&f, "{ not json").unwrap();
        assert!(e.run().is_some());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "{ not json");
    }
}
