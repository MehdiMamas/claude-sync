//! Plugin marketplaces bundled in a group repo (§4, issue #30).
//!
//! `plugins/<name>/` in the repo is placed at `<state_dir>/marketplaces/<name>/` (the files
//! go through the normal managed-file path in `apply_group`) and registered in
//! `~/.claude/settings.json` as `extraKnownMarketplaces.<name>`, a directory source. That
//! value is the app's: it replaces whatever was there (settings.json is backed up first),
//! wins over the same key in a group's `settings.shared.json`, and is removed again when
//! the marketplace leaves the group or the group is removed.
use super::{
    blocked_by_user, exists_any, is_managed_target, load_owned, push_unique, read_file,
    read_settings_file, remove_counted, save_owned, write_atomic, ApplyReport, Backups, Collision,
    ManagedMarketplace, Manifest,
};
use crate::paths::MARKETPLACES;
use crate::remote::Head;
use crate::{group, settings, FileSet, GroupId, Paths};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// `/extraKnownMarketplaces/<name>`.
pub(super) fn pointer(name: &str) -> String {
    settings::join("/extraKnownMarketplaces", name)
}

/// The `extraKnownMarketplaces.<name>` value for the local copy of a bundled marketplace.
pub(super) fn source(paths: &Paths, name: &str) -> Value {
    let dir = paths.marketplaces_dir().join(name);
    let dir = std::path::absolute(&dir).unwrap_or(dir);
    json!({ "source": { "source": "directory", "path": dir.to_string_lossy() } })
}

/// The marketplaces in `files` this group places here: those with a
/// `.claude-plugin/marketplace.json` whose `name` is the folder name, and that no other
/// joined group placed first (reported as a collision on `plugins/<name>`).
pub(super) fn plan(
    m: &Manifest,
    group: &GroupId,
    order: &[GroupId],
    files: &FileSet,
    report: &mut ApplyReport,
) -> BTreeSet<String> {
    let names: BTreeSet<&str> = files
        .keys()
        .filter_map(|p| group::marketplace_of(p))
        .collect();
    let mut placed = BTreeSet::new();
    for name in names {
        let owner = m.marketplaces.get(name).map(|o| &o.group);
        // The owner keeps it while it ranks above this group; a group that ranks above the
        // owner takes it over (after a reorder, §7).
        let rank = |g: &GroupId| order.iter().position(|x| x == g);
        let held = owner.filter(|g| {
            **g != *group
                && match (rank(g), rank(group)) {
                    (Some(o), Some(me)) => o < me,
                    (Some(_), None) => true,
                    _ => false,
                }
        });
        if let Some(owner) = held {
            report.collisions.push(Collision::OtherGroup {
                path: format!("{}/{name}", group::PLUGINS),
                owner: owner.clone(),
            });
            continue;
        }
        let named = files
            .get(&group::marketplace_manifest(name))
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .is_some_and(|v| v.get("name").and_then(Value::as_str) == Some(name));
        if !named {
            report.skipped.push(format!(
                "plugins/{name}: no .claude-plugin/marketplace.json with \"name\": \"{name}\", so the marketplace is not installed"
            ));
            continue;
        }
        placed.insert(name.to_string());
    }
    placed
}

/// Make `settings.json` register exactly the `placed` marketplaces of `group`: set their
/// entries, and remove the entries of marketplaces the group placed before but not now
/// (only while they still hold the value the app set). Records ownership in `m`.
pub(super) fn register(
    paths: &Paths,
    group: &GroupId,
    placed: &BTreeSet<String>,
    m: &mut Manifest,
    backups: &mut Backups,
    report: &mut ApplyReport,
) -> crate::Result<()> {
    let dropped: Vec<String> = m
        .marketplaces
        .iter()
        .filter(|(name, o)| o.group == *group && !placed.contains(*name))
        .map(|(name, _)| name.clone())
        .collect();
    if placed.is_empty() && dropped.is_empty() {
        return Ok(());
    }
    if !dropped.is_empty() {
        report.plugins_changed = true;
    }
    let file = paths.claude_home.join("settings.json");
    let (mut local, existing) = match read_settings_file(&file) {
        Ok(v) => v,
        Err(msg) => {
            // Keep the claim on the marketplaces (and on the dropped ones, to clean up later).
            push_unique(&mut report.settings_skipped, &msg);
            for name in placed {
                m.marketplaces
                    .entry(name.clone())
                    .or_insert_with(|| ManagedMarketplace {
                        group: group.clone(),
                        setting: None,
                    });
            }
            return Ok(());
        }
    };
    let before = local.clone();
    for name in placed {
        let ptr = pointer(name);
        let want = source(paths, name);
        let mine = m.marketplaces.get(name).and_then(|o| o.setting.clone());
        let mut setting = Some(want.clone());
        match local.pointer(&ptr) {
            Some(cur) if *cur == want => {}
            cur => {
                if cur.is_some() && cur != mine.as_ref() {
                    report.skipped.push(format!(
                        "extraKnownMarketplaces.{name}: replaced by the marketplace bundled under plugins/{name} (settings.json was backed up)"
                    ));
                }
                if !settings::set_at(&mut local, &ptr, want) {
                    report.settings_skipped.push(format!(
                        "extraKnownMarketplaces in settings.json is not an object, so the marketplace {name} is not registered"
                    ));
                    setting = None;
                }
            }
        }
        m.marketplaces.insert(
            name.clone(),
            ManagedMarketplace {
                group: group.clone(),
                setting,
            },
        );
    }
    for name in &dropped {
        let ptr = pointer(name);
        let mine = m.marketplaces.remove(name).and_then(|o| o.setting);
        if mine.is_some() && local.pointer(&ptr) == mine.as_ref() {
            settings::remove_at(&mut local, &ptr);
        }
    }
    if local != before {
        if let Some(old) = &existing {
            backups.save("settings.json", old)?;
        }
        write_atomic(&file, &settings::to_bytes(&local))?;
        push_unique(&mut report.written, "settings.json");
        report.plugins_changed = true;
    }

    // No group owns a bundled marketplace's entry through settings.shared.json: it is
    // neither a local edit to share nor a value to restore.
    let mut owned_all = load_owned(paths)?;
    let owned_before = owned_all.clone();
    for name in placed {
        for o in owned_all.values_mut() {
            settings::forget(o, &pointer(name));
        }
    }
    owned_all.retain(|_, o| !o.is_empty());
    if owned_all != owned_before {
        save_owned(paths, &owned_all)?;
    }
    Ok(())
}

/// Files the user added on this device inside a marketplace folder `group` placed: not in
/// the manifest, allowlisted, not excluded. They show up as local edits.
pub(super) fn new_files(
    paths: &Paths,
    m: &Manifest,
    group: &GroupId,
) -> crate::Result<Vec<String>> {
    let entry = m.groups.get(&group.to_string());
    let mut out = Vec::new();
    for (name, o) in &m.marketplaces {
        if o.group != *group {
            continue;
        }
        let prefix = format!("{}/{name}", group::PLUGINS);
        let dir = paths.marketplaces_dir().join(name);
        if blocked_by_user(&paths.state_dir, &format!("{MARKETPLACES}/{name}/x")) {
            continue;
        }
        walk(&dir, &prefix, &mut Vec::new(), &mut out)?;
    }
    out.retain(|rel| !m.files.contains_key(rel) && entry.is_none_or(|e| !e.excluded.contains(rel)));
    Ok(out)
}

/// `path` is a file the user added inside a marketplace folder `group` placed.
pub(super) fn is_new_file(paths: &Paths, m: &Manifest, group: &GroupId, path: &str) -> bool {
    group::marketplace_of(path)
        .and_then(|name| m.marketplaces.get(name))
        .is_some_and(|o| o.group == *group)
        && !m.files.contains_key(path)
        && !is_generated(path)
        && group::is_allowlisted(path)
        && is_managed_target(path)
        && exists_any(&super::target_of(paths, path))
}

/// The marker the app writes at the root of each delivered marketplace folder: the group's
/// commit sha, and its date when known. Never a local edit.
pub(crate) const COMMIT: &str = "COMMIT";

/// Write `<marketplace>/COMMIT` for each placed marketplace whose folder exists, only when
/// the content changes. Marketplaces `group` holds but no longer places lose theirs.
pub(super) fn write_commit_markers(
    paths: &Paths,
    group: &GroupId,
    m: &Manifest,
    placed: &BTreeSet<String>,
    head: Option<&Head>,
) -> crate::Result<()> {
    remove_commit_markers(paths, group, m, placed)?;
    let Some(head) = head else {
        return Ok(());
    };
    let mut text = format!("{}\n", head.commit);
    if let Some(date) = &head.date {
        text.push_str(&format!("{date}\n"));
    }
    for name in placed {
        let dir = paths.marketplaces_dir().join(name);
        if !dir.is_dir() {
            continue;
        }
        let file = dir.join(COMMIT);
        if read_file(&file)?.as_deref() != Some(text.as_bytes()) {
            write_atomic(&file, text.as_bytes())?;
        }
    }
    Ok(())
}

/// Remove the `COMMIT` marker of the marketplaces `group` holds that are not in `keep`.
pub(super) fn remove_commit_markers(
    paths: &Paths,
    group: &GroupId,
    m: &Manifest,
    keep: &BTreeSet<String>,
) -> crate::Result<()> {
    for (name, o) in &m.marketplaces {
        if o.group == *group && !keep.contains(name) {
            let file = paths.marketplaces_dir().join(name).join(COMMIT);
            if exists_any(&file) {
                remove_counted(&file)?;
            }
        }
    }
    Ok(())
}

/// Claude Code writes these into a mod folder itself: `.claude-plugin/types/**` and
/// `mods/<name>/tsconfig.json`. `rel` is `plugins/<marketplace>/<path>`.
fn is_generated(rel: &str) -> bool {
    let r: Vec<&str> = rel.split('/').skip(2).collect();
    r == [COMMIT]
        || r.windows(2).any(|w| w == [".claude-plugin", "types"])
        || (r.len() == 3 && r[0] == "mods" && r[2] == "tsconfig.json")
}

/// Regular files under `dir` as `<prefix>/<relative path>`, skipping links, temp files of
/// an atomic write, anything the allowlist refuses (`.git`, `node_modules`, ...), files
/// Claude Code generates, and paths a `.gitignore` in the tree (`ignores`) matches.
fn walk(
    dir: &Path,
    prefix: &str,
    ignores: &mut Vec<Gitignore>,
    out: &mut Vec<String>,
) -> crate::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let own = dir.join(".gitignore");
    let mut b = GitignoreBuilder::new(dir);
    let pushed = own.is_file() && b.add(&own).is_none();
    if pushed {
        if let Ok(g) = b.build() {
            ignores.push(g);
        }
    }
    let result = walk_entries(entries, prefix, ignores, out);
    if pushed {
        ignores.pop();
    }
    result
}

fn walk_entries(
    entries: fs::ReadDir,
    prefix: &str,
    ignores: &mut Vec<Gitignore>,
    out: &mut Vec<String>,
) -> crate::Result<()> {
    for e in entries {
        let e = e?;
        let Some(name) = e.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let rel = format!("{prefix}/{name}");
        let ft = e.file_type()?;
        if ft.is_symlink() || is_generated(&rel) {
            continue;
        }
        let path = e.path();
        let is_dir = ft.is_dir();
        if ignores.iter().any(|g| {
            path.starts_with(g.path()) && g.matched_path_or_any_parents(&path, is_dir).is_ignore()
        }) {
            continue;
        }
        if is_dir {
            // A folder whose files can never sync is not worth walking.
            if group::is_allowlisted(&format!("{rel}/x")) {
                walk(&path, &rel, ignores, out)?;
            }
        } else if ft.is_file()
            && !name.contains(".claude-sync-tmp-")
            && group::is_allowlisted(&rel)
            && is_managed_target(&rel)
        {
            out.push(rel);
        }
    }
    Ok(())
}
