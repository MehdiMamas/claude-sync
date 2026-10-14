//! Apply layer: group files -> ~/.claude, with a manifest (§7, issue #16).
//! Settings go through `crate::settings` (§8).
//!
//! On disk (all under `Paths::state_dir`):
//! - `manifest.json`: every file the app wrote into `~/.claude` (path -> group, SHA-256),
//!   plus per group: the hash of its `CLAUDE.md` copy and `settings.shared.json` copy,
//!   the paths kept local on this device, and the paths it ships that are not applied here.
//! - `settings-owned.json`: per group, the settings values and array items the app set.
//! - `groups/<owner>__<repo>/CLAUDE.md` and `.../settings.shared.json`: the group's files
//!   as last applied, byte for byte.
//! - `backups/<UTC time>/`: copies of user files taken before the app changes them.
//! - `marketplaces/<name>/`: plugin marketplaces a group bundles under `plugins/<name>/`,
//!   registered in `settings.json` as `extraKnownMarketplaces.<name>` (a directory source).
//!   The manifest tracks these files like the ones in `~/.claude` (see [`target_of`]).
//!
//! A managed file the user edited (or deleted) is not reverted while the group's version
//! stays the same; it shows up in `watch::local_edits` and in [`local_files`] instead. When
//! the group ships a new version, the apply writes it: by then the three-way merge
//! upstream has seen both sides and saved any losing local copy.
use crate::group;
use crate::paths::MARKETPLACES;
use crate::settings::{self, Owned};
use crate::{FileSet, GroupId, Paths};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const SETTINGS_OWNED_FILE: &str = "settings-owned.json";
pub const CLAUDE_MD: &str = "CLAUDE.md";
pub const SHARED_SETTINGS: &str = "settings.shared.json";
/// `keep_local` / `undo` / `LocalEdit` path for one settings value:
/// `settings.shared.json#/permissions/allow` (a JSON pointer after the `#`).
pub const SETTINGS_POINTER_PREFIX: &str = "settings.shared.json#";
pub const BLOCK_BEGIN: &str = "<!-- claude-sync:begin (managed, do not edit inside) -->";
pub const BLOCK_END: &str = "<!-- claude-sync:end -->";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Collision {
    /// A file the user owns is in the way.
    UserFile { path: String },
    /// Another group, earlier in the order, already ships this path.
    OtherGroup { path: String, owner: GroupId },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub written: Vec<String>,
    pub removed: Vec<String>,
    pub collisions: Vec<Collision>,
    /// Settings keys not applied and why (§8).
    pub settings_skipped: Vec<String>,
    /// Managed files left in place because they were edited locally (the group dropped
    /// them, or the group was removed). They are the user's now, or a local edit to share.
    pub kept: Vec<String>,
    /// Group files not placed and why: oversize plugin files, invalid marketplaces (§4).
    pub skipped: Vec<String>,
    /// A plugin file or a bundled marketplace's `settings.json` entry changed: open Claude
    /// Code sessions need `/reload-plugins` (or a restart) to see it.
    pub plugins_changed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Repo path (forward slashes) -> owner. The file is at that path under `~/.claude`,
    /// except `plugins/<name>/...`, which is under `<state_dir>/marketplaces/<name>/...`.
    #[serde(default)]
    pub files: BTreeMap<String, ManagedFile>,
    /// Bundled plugin marketplaces placed on this machine, by name. The first group that
    /// placed one keeps it; another group shipping the same name gets a collision.
    #[serde(default)]
    pub marketplaces: BTreeMap<String, ManagedMarketplace>,
    /// Keyed by `owner/repo`.
    #[serde(default)]
    pub groups: BTreeMap<String, GroupEntry>,
    /// The joined-group order last passed to `apply_group` (import line order).
    #[serde(default)]
    pub order: Vec<GroupId>,
    /// Folders under `~/.claude` the app created (removed again when empty). Entries under
    /// `marketplaces/` are under `state_dir`.
    #[serde(default)]
    pub created_dirs: BTreeSet<String>,
    /// The app created `~/.claude/CLAUDE.md`; it is deleted again if only the block was in it.
    #[serde(default)]
    pub created_claude_md: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedFile {
    pub group: GroupId,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedMarketplace {
    pub group: GroupId,
    /// The `extraKnownMarketplaces.<name>` value the app set in `settings.json`; `None`
    /// when it could not be set (settings.json unreadable).
    #[serde(default)]
    pub setting: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupEntry {
    /// SHA-256 of the group's `CLAUDE.md` as the app last wrote the copy.
    #[serde(default)]
    pub claude_md: Option<String>,
    /// SHA-256 of the group's `settings.shared.json` as last applied.
    #[serde(default)]
    pub settings: Option<String>,
    /// Paths this device stopped managing ("Keep local"): file paths, `CLAUDE.md`,
    /// `settings.shared.json`, or `settings.shared.json#<pointer>`.
    #[serde(default)]
    pub keep_local: BTreeSet<String>,
    /// Paths the group ships that the last apply did not put on this machine
    /// (collisions, unsafe or unknown paths).
    #[serde(default)]
    pub excluded: BTreeSet<String>,
}

impl Manifest {
    pub fn load(paths: &Paths) -> crate::Result<Self> {
        Ok(read_json(&paths.state_dir.join(MANIFEST_FILE))?.unwrap_or_default())
    }
    fn save(&self, paths: &Paths) -> crate::Result<()> {
        write_json(&paths.state_dir.join(MANIFEST_FILE), self)
    }
}

/// `settings-owned.json`: `owner/repo` -> what that group set.
pub fn load_owned(paths: &Paths) -> crate::Result<BTreeMap<String, Owned>> {
    Ok(read_json(&paths.state_dir.join(SETTINGS_OWNED_FILE))?.unwrap_or_default())
}

fn save_owned(paths: &Paths, owned: &BTreeMap<String, Owned>) -> crate::Result<()> {
    write_json(&paths.state_dir.join(SETTINGS_OWNED_FILE), owned)
}

/// Make `~/.claude` reflect `files` for `group`. `order` is the joined-group order (§7 collisions).
/// Idempotent: a second call with the same input writes nothing.
pub fn apply_group(
    paths: &Paths,
    group: &GroupId,
    order: &[GroupId],
    files: &FileSet,
) -> crate::Result<ApplyReport> {
    let mut m = Manifest::load(paths)?;
    let before = m.clone();
    let gkey = group.to_string();
    let mut entry = m.groups.get(&gkey).cloned().unwrap_or_default();
    let mut report = ApplyReport::default();
    let mut backups = Backups::new(paths);
    m.order = order.to_vec();
    entry.excluded.clear();
    // A kept path the group no longer ships is forgotten; settings pointers stay, and so do
    // new plugin files kept on this device while they exist.
    entry.keep_local.retain(|k| {
        k.starts_with(SETTINGS_POINTER_PREFIX)
            || files.contains_key(k)
            || (group::marketplace_of(k).is_some() && exists_any(&target_of(paths, k)))
    });
    let rank = |g: &GroupId| order.iter().position(|x| x == g);
    let my_rank = rank(group);
    let placed = marketplaces::plan(&m, group, order, files, &mut report);

    // 1. Managed tree files, and the files of the marketplaces placed here.
    let mut desired = BTreeSet::new();
    for (rel, bytes) in files {
        if rel == CLAUDE_MD || rel == SHARED_SETTINGS || entry.keep_local.contains(rel) {
            continue;
        }
        let plugin = group::marketplace_of(rel);
        if let Some(mp) = plugin {
            if !placed.contains(mp) {
                entry.excluded.insert(rel.clone());
                continue;
            }
            if group::too_large(rel, bytes.len()) {
                report.skipped.push(group::too_large_note(rel));
                entry.excluded.insert(rel.clone());
                continue;
            }
        }
        if !is_managed_target(rel) {
            if is_tree_path(rel) {
                tracing::warn!(group = %gkey, path = %rel, "unsafe path in group; not applied");
            }
            entry.excluded.insert(rel.clone());
            continue;
        }
        let (root, sub) = locate(paths, rel);
        let target = target_path(root, &sub);
        let sha = sha256_hex(bytes);
        match m.files.get(rel).cloned() {
            Some(mf) if mf.group == *group => {
                desired.insert(rel.clone());
                if blocked_by_user(root, &sub) || not_a_file(&target) {
                    report
                        .collisions
                        .push(Collision::UserFile { path: rel.clone() });
                    entry.excluded.insert(rel.clone());
                    continue;
                }
                let local_sha = read_file(&target)?.map(|b| sha256_hex(&b));
                if local_sha.as_deref() == Some(sha.as_str()) {
                    // Already there.
                } else if mf.sha256 == sha && local_sha.as_deref() != Some(mf.sha256.as_str()) {
                    // The group's version did not change and the user edited or deleted
                    // the file: leave it, it is a local edit.
                    continue;
                } else {
                    write_tree_file(root, &sub, bytes, &mut m.created_dirs)?;
                    push_unique(&mut report.written, rel);
                }
                m.files.insert(
                    rel.clone(),
                    ManagedFile {
                        group: group.clone(),
                        sha256: sha,
                    },
                );
            }
            Some(mf) => {
                // A marketplace's files follow the marketplace, which this group holds.
                let we_win = plugin.is_some()
                    || match (my_rank, rank(&mf.group)) {
                        (Some(a), Some(b)) => a < b,
                        (Some(_), None) => true,
                        _ => false,
                    };
                if !we_win || blocked_by_user(root, &sub) || not_a_file(&target) {
                    report.collisions.push(Collision::OtherGroup {
                        path: rel.clone(),
                        owner: mf.group,
                    });
                    entry.excluded.insert(rel.clone());
                    continue;
                }
                // An earlier group takes the path over from a later one.
                desired.insert(rel.clone());
                let cur = read_file(&target)?;
                if let Some(cur) = cur.as_ref().filter(|c| sha256_hex(c) != mf.sha256) {
                    backups.save(rel, cur)?;
                }
                if cur.as_deref() != Some(bytes.as_slice()) {
                    write_tree_file(root, &sub, bytes, &mut m.created_dirs)?;
                    push_unique(&mut report.written, rel);
                }
                m.files.insert(
                    rel.clone(),
                    ManagedFile {
                        group: group.clone(),
                        sha256: sha,
                    },
                );
            }
            None => {
                let in_the_way = if plugin.is_some() {
                    // The marketplace folder is the app's: a stray file there is backed
                    // up and replaced, only a folder or link in the way blocks.
                    not_a_file(&target)
                } else {
                    exists_any(&target)
                };
                if in_the_way || blocked_by_user(root, &sub) {
                    report
                        .collisions
                        .push(Collision::UserFile { path: rel.clone() });
                    entry.excluded.insert(rel.clone());
                    continue;
                }
                desired.insert(rel.clone());
                let cur = read_file(&target)?;
                if cur.as_deref() != Some(bytes.as_slice()) {
                    if let Some(cur) = &cur {
                        backups.save(rel, cur)?;
                    }
                    write_tree_file(root, &sub, bytes, &mut m.created_dirs)?;
                    push_unique(&mut report.written, rel);
                }
                m.files.insert(
                    rel.clone(),
                    ManagedFile {
                        group: group.clone(),
                        sha256: sha,
                    },
                );
            }
        }
    }

    // 2. Files the group dropped: deleted when unchanged since the app wrote them.
    let dropped: Vec<(String, ManagedFile)> = m
        .files
        .iter()
        .filter(|(p, f)| f.group == *group && !desired.contains(*p))
        .map(|(p, f)| (p.clone(), f.clone()))
        .collect();
    for (rel, mf) in dropped {
        if entry.keep_local.contains(&rel) {
            m.files.remove(&rel);
            continue;
        }
        let (root, sub) = locate(paths, &rel);
        match delete_if_unchanged(root, &sub, &mf.sha256)? {
            Deleted::Yes => {
                m.files.remove(&rel);
                push_unique(&mut report.removed, &rel);
            }
            Deleted::Gone => {
                m.files.remove(&rel);
            }
            // Edited locally: stays managed, so it shows as a local edit.
            Deleted::Edited => push_unique(&mut report.kept, &rel),
        }
    }
    prune_dirs(paths, &mut m)?;

    // 3. The group's CLAUDE.md copy.
    if !entry.keep_local.contains(CLAUDE_MD) {
        let copy = paths.group_dir(group).join(CLAUDE_MD);
        match files.get(CLAUDE_MD) {
            Some(bytes) => {
                let sha = sha256_hex(bytes);
                let cur = read_file(&copy)?.map(|b| sha256_hex(&b));
                let edited = entry.claude_md.as_deref() == Some(sha.as_str())
                    && cur.as_deref() != Some(sha.as_str());
                if cur.as_deref() != Some(sha.as_str()) && !edited {
                    write_atomic(&copy, bytes)?;
                    push_unique(&mut report.written, CLAUDE_MD);
                }
                entry.claude_md = Some(sha);
            }
            None => {
                if let Some(old) = entry.claude_md.take() {
                    match read_file(&copy)? {
                        Some(cur) if sha256_hex(&cur) != old => {
                            entry.claude_md = Some(old);
                            push_unique(&mut report.kept, CLAUDE_MD);
                        }
                        Some(_) => {
                            remove_counted(&copy)?;
                            push_unique(&mut report.removed, CLAUDE_MD);
                        }
                        None => {}
                    }
                }
            }
        }
    }
    m.groups.insert(gkey.clone(), entry.clone());
    update_claude_md_block(paths, &mut m, &mut backups, &mut report)?;

    // 4. settings.shared.json -> settings.json. A bundled marketplace's entry is the app's.
    if !entry.keep_local.contains(SHARED_SETTINGS) {
        let bundled: BTreeSet<String> = m.marketplaces.keys().chain(&placed).cloned().collect();
        let entry = m.groups.get_mut(&gkey).expect("inserted above");
        apply_settings(
            paths,
            group,
            order,
            files.get(SHARED_SETTINGS),
            &bundled,
            entry,
            &mut backups,
            &mut report,
        )?;
    }

    // 5. extraKnownMarketplaces entries for the marketplaces placed here.
    marketplaces::register(paths, group, &placed, &mut m, &mut backups, &mut report)?;
    report.plugins_changed |= touches_plugins(&report);

    if m != before {
        m.save(paths)?;
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn apply_settings(
    paths: &Paths,
    group: &GroupId,
    order: &[GroupId],
    shared_bytes: Option<&Vec<u8>>,
    bundled: &BTreeSet<String>,
    entry: &mut GroupEntry,
    backups: &mut Backups,
    report: &mut ApplyReport,
) -> crate::Result<()> {
    let gkey = group.to_string();
    let copy = paths.group_dir(group).join(SHARED_SETTINGS);
    let mut owned_all = load_owned(paths)?;
    let owned_before = owned_all.clone();
    let prev = owned_all.get(&gkey).cloned().unwrap_or_default();
    let shared = match shared_bytes {
        Some(bytes) => {
            if read_file(&copy)?.as_deref() != Some(bytes.as_slice()) {
                write_atomic(&copy, bytes)?;
            }
            entry.settings = Some(sha256_hex(bytes));
            match settings::parse_object(bytes) {
                Some(v) => v,
                None => {
                    report.settings_skipped.push(
                        "settings.shared.json is not one JSON object, so nothing was merged; settings.json was not changed"
                            .into(),
                    );
                    return Ok(());
                }
            }
        }
        None => {
            if entry.settings.take().is_some() {
                remove_counted(&copy)?;
            }
            if prev.is_empty() {
                return Ok(());
            }
            Value::Object(Default::default())
        }
    };

    // Pointers this group must not set: kept local here, or owned by an earlier group.
    let mut filters: Vec<(String, String)> = entry
        .keep_local
        .iter()
        .filter_map(|k| k.strip_prefix(SETTINGS_POINTER_PREFIX))
        .map(|p| {
            (
                p.to_string(),
                "kept local on this device; not applied".to_string(),
            )
        })
        .collect();
    let rank = |g: &str| order.iter().position(|x| x.to_string() == g);
    let my_rank = rank(&gkey);
    for (g, o) in &owned_all {
        let earlier = match (rank(g), my_rank) {
            (Some(a), Some(b)) => a < b,
            (Some(_), None) => true,
            _ => false,
        };
        if g != &gkey && earlier {
            filters.extend(
                o.values
                    .keys()
                    .map(|p| (p.clone(), format!("set by group {g}; not applied"))),
            );
        }
    }
    filters.extend(bundled.iter().map(|mp| {
        (
            marketplaces::pointer(mp),
            format!("the group repo bundles this marketplace under plugins/{mp}; the directory source is used"),
        )
    }));
    let (shared, mut skipped) = settings::without(&shared, &filters);

    let file = paths.claude_home.join("settings.json");
    let (local, existing) = match read_settings_file(&file) {
        Ok(v) => v,
        Err(msg) => {
            report.settings_skipped.extend(skipped);
            report.settings_skipped.push(msg);
            return Ok(());
        }
    };
    let merged = settings::merge(&local, &shared, &prev)?;
    skipped.extend(merged.skipped);
    if merged.changed {
        if let Some(old) = &existing {
            backups.save("settings.json", old)?;
        }
        write_atomic(&file, &settings::to_bytes(&merged.settings))?;
        push_unique(&mut report.written, "settings.json");
    }
    if merged.owned.is_empty() {
        owned_all.remove(&gkey);
    } else {
        owned_all.insert(gkey, merged.owned);
    }
    if owned_all != owned_before {
        save_owned(paths, &owned_all)?;
    }
    report.settings_skipped.extend(skipped);
    Ok(())
}

/// `~/.claude/settings.json` as `(object, raw bytes)`; a missing file is `{}`.
/// `Err` is the message to report when it must not be touched.
fn read_settings_file(file: &Path) -> Result<(Value, Option<Vec<u8>>), String> {
    match fs::symlink_metadata(file) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Value::Object(Default::default()), None)),
        Err(e) => return Err(format!("could not read settings.json ({e}); settings.json was not changed")),
        Ok(md) if md.file_type().is_symlink() || !md.is_file() => {
            return Err("settings.json is a link or not a regular file, so nothing was merged; settings.json was not changed".into())
        }
        Ok(_) => {}
    }
    let raw = fs::read(file).map_err(|e| {
        format!("could not read settings.json ({e}); settings.json was not changed")
    })?;
    match settings::parse_object(&raw) {
        Some(v) => Ok((v, Some(raw))),
        None => Err("settings.json is not one JSON object, so nothing was merged; settings.json was not changed".into()),
    }
}

/// The group's files as they are on this machine right now, read back from `~/.claude`
/// (the "local" side of the three-way merge). Paths set to "keep local" are left out.
///
/// - Managed files: their current bytes; a managed file the user deleted is missing.
/// - New files the user added inside a marketplace folder this group placed.
/// - `CLAUDE.md`: the group's copy under `groups/<owner>__<repo>/`.
/// - `settings.shared.json`: rebuilt from the copy the app last applied, with every
///   value the app set replaced by the local one where the user changed it, and dropped
///   where the user deleted it (`settings::reconstruct`). With no such edit it is the
///   applied copy byte for byte, so the merge sees "unchanged".
///
/// Paths in [`excluded_paths`] (kept local, collisions, unsafe) are never returned; the
/// engine must treat them as unchanged (local = base) so they are not pushed as deletes.
pub fn local_files(paths: &Paths, group: &GroupId) -> crate::Result<FileSet> {
    let m = Manifest::load(paths)?;
    let entry = m
        .groups
        .get(&group.to_string())
        .cloned()
        .unwrap_or_default();
    let mut out = FileSet::new();
    for (rel, mf) in &m.files {
        let (root, sub) = locate(paths, rel);
        if mf.group != *group || entry.keep_local.contains(rel) || blocked_by_user(root, &sub) {
            continue;
        }
        if let Some(b) = read_file(&target_path(root, &sub))? {
            out.insert(rel.clone(), b);
        }
    }
    for rel in marketplaces::new_files(paths, &m, group)? {
        if let Some(b) = read_file(&target_of(paths, &rel))? {
            out.insert(rel, b);
        }
    }
    if entry.claude_md.is_some() && !entry.keep_local.contains(CLAUDE_MD) {
        if let Some(b) = read_file(&paths.group_dir(group).join(CLAUDE_MD))? {
            out.insert(CLAUDE_MD.into(), b);
        }
    }
    if entry.settings.is_some() && !entry.keep_local.contains(SHARED_SETTINGS) {
        if let Some(raw) = read_file(&paths.group_dir(group).join(SHARED_SETTINGS))? {
            let owned = load_owned(paths)?
                .remove(&group.to_string())
                .unwrap_or_default();
            let local = read_settings_file(&paths.claude_home.join("settings.json"))
                .ok()
                .map(|(v, _)| v);
            let rebuilt = match (settings::parse_object(&raw), local) {
                (Some(copy), Some(local)) => settings::reconstruct(&copy, &local, &owned),
                _ => None,
            };
            out.insert(
                SHARED_SETTINGS.into(),
                rebuilt.map(|v| settings::to_bytes(&v)).unwrap_or(raw),
            );
        }
    }
    Ok(out)
}

/// Files the user added on this device inside a marketplace folder `group` placed. They
/// count as local edits ([`local_files`] carries them; Share pushes them to the group).
pub fn added_plugin_files(
    paths: &Paths,
    m: &Manifest,
    group: &GroupId,
) -> crate::Result<Vec<String>> {
    marketplaces::new_files(paths, m, group)
}

/// Paths the group ships that this device does not manage: kept local, collisions, and
/// paths the apply layer does not place (unsafe or unknown). [`local_files`] leaves them
/// out; the engine should use the base version for them as the local side.
pub fn excluded_paths(paths: &Paths, group: &GroupId) -> crate::Result<BTreeSet<String>> {
    let m = Manifest::load(paths)?;
    let entry = m
        .groups
        .get(&group.to_string())
        .cloned()
        .unwrap_or_default();
    let mut out = entry.excluded;
    out.extend(
        entry
            .keep_local
            .into_iter()
            .filter(|k| !k.starts_with(SETTINGS_POINTER_PREFIX)),
    );
    Ok(out)
}

/// Remove everything this group put on the machine (files, import line, owned settings), with a backup.
/// Files the user edited are backed up and left in place (reported in `kept`); settings
/// values the user changed stay and become the user's.
pub fn remove_group(paths: &Paths, group: &GroupId) -> crate::Result<ApplyReport> {
    let mut m = Manifest::load(paths)?;
    let before = m.clone();
    let gkey = group.to_string();
    let mut report = ApplyReport::default();
    let mut backups = Backups::new(paths);
    let dir = group.dir_name();

    let ours: Vec<(String, ManagedFile)> = m
        .files
        .iter()
        .filter(|(_, f)| f.group == *group)
        .map(|(p, f)| (p.clone(), f.clone()))
        .collect();
    for (rel, mf) in ours {
        m.files.remove(&rel);
        let (root, sub) = locate(paths, &rel);
        if blocked_by_user(root, &sub) {
            continue;
        }
        let target = target_path(root, &sub);
        let Some(cur) = read_file(&target)? else {
            continue;
        };
        backups.save(&format!("{dir}/{rel}"), &cur)?;
        if sha256_hex(&cur) == mf.sha256 {
            remove_counted(&target)?;
            push_unique(&mut report.removed, &rel);
        } else {
            push_unique(&mut report.kept, &rel);
        }
    }
    prune_dirs(paths, &mut m)?;

    let gdir = paths.group_dir(group);
    let copy = gdir.join(CLAUDE_MD);
    if let Some(cur) = read_file(&copy)? {
        backups.save(&format!("{dir}/{CLAUDE_MD}"), &cur)?;
        remove_counted(&copy)?;
        push_unique(&mut report.removed, CLAUDE_MD);
    }
    m.groups.remove(&gkey);
    m.order.retain(|g| g != group);
    update_claude_md_block(paths, &mut m, &mut backups, &mut report)?;
    marketplaces::register(
        paths,
        group,
        &BTreeSet::new(),
        &mut m,
        &mut backups,
        &mut report,
    )?;
    report.plugins_changed |= touches_plugins(&report);

    let mut owned_all = load_owned(paths)?;
    if let Some(prev) = owned_all.remove(&gkey) {
        let file = paths.claude_home.join("settings.json");
        match read_settings_file(&file) {
            Ok((local, existing)) => {
                let merged = settings::merge(&local, &Value::Object(Default::default()), &prev)?;
                if merged.changed {
                    if let Some(old) = &existing {
                        backups.save("settings.json", old)?;
                    }
                    write_atomic(&file, &settings::to_bytes(&merged.settings))?;
                    push_unique(&mut report.written, "settings.json");
                }
            }
            Err(msg) => report.settings_skipped.push(msg),
        }
        save_owned(paths, &owned_all)?;
    }
    let settings_copy = gdir.join(SHARED_SETTINGS);
    if exists_any(&settings_copy) {
        remove_counted(&settings_copy)?;
    }
    // The folder goes too if nothing else (the engine's base snapshot) is in it.
    let _ = fs::remove_dir(&gdir);

    if m != before {
        m.save(paths)?;
    }
    Ok(report)
}

/// Stop managing `path` on this device (the "Keep local" action, §7).
/// `path` is a managed file, `CLAUDE.md`, `settings.shared.json` (all of this group's
/// settings) or `settings.shared.json#<pointer>` (one value). The local content stays as
/// it is and becomes the user's; the group's version is no longer applied here.
pub fn keep_local(paths: &Paths, group: &GroupId, path: &str) -> crate::Result<()> {
    let mut m = Manifest::load(paths)?;
    let gkey = group.to_string();
    let mut entry = m.groups.get(&gkey).cloned().unwrap_or_default();
    if let Some(ptr) = path.strip_prefix(SETTINGS_POINTER_PREFIX) {
        let mut owned_all = load_owned(paths)?;
        if let Some(o) = owned_all.get_mut(&gkey) {
            settings::forget(o, ptr);
            if o.is_empty() {
                owned_all.remove(&gkey);
            }
        }
        save_owned(paths, &owned_all)?;
    } else if path == SHARED_SETTINGS {
        let mut owned_all = load_owned(paths)?;
        if owned_all.remove(&gkey).is_some() {
            save_owned(paths, &owned_all)?;
        }
    } else if path == CLAUDE_MD {
        // The copy and the import line stay; the copy is no longer updated.
    } else {
        match m.files.get(path) {
            Some(mf) if mf.group == *group => {
                m.files.remove(path);
            }
            None if marketplaces::is_new_file(paths, &m, group, path) => {}
            _ => {
                return Err(crate::Error::Invalid(format!(
                    "{path} is not managed for group {group}"
                )))
            }
        }
    }
    entry.keep_local.insert(path.to_string());
    m.groups.insert(gkey, entry);
    m.save(paths)
}

/// Put the group's version back (the "Undo" action, §7). For a file or `CLAUDE.md`,
/// `group_version` is the group's content (`None` = the group does not have it: delete
/// it). For `settings.shared.json` or `settings.shared.json#<pointer>` the values the app
/// set are restored from the ownership record and `group_version` is ignored.
pub fn undo(
    paths: &Paths,
    group: &GroupId,
    path: &str,
    group_version: Option<&[u8]>,
) -> crate::Result<()> {
    let mut m = Manifest::load(paths)?;
    let gkey = group.to_string();
    let mut backups = Backups::new(paths);
    let under = if path == SHARED_SETTINGS {
        Some("")
    } else {
        path.strip_prefix(SETTINGS_POINTER_PREFIX)
    };
    if let Some(under) = under {
        let owned = load_owned(paths)?.remove(&gkey).unwrap_or_default();
        let file = paths.claude_home.join("settings.json");
        let (local, existing) = read_settings_file(&file).map_err(crate::Error::Invalid)?;
        let restored = settings::restore(&local, &owned, under);
        if restored != local {
            if let Some(old) = &existing {
                backups.save("settings.json", old)?;
            }
            write_atomic(&file, &settings::to_bytes(&restored))?;
        }
        return Ok(());
    }
    if path == CLAUDE_MD {
        let copy = paths.group_dir(group).join(CLAUDE_MD);
        let entry = m.groups.entry(gkey).or_default();
        match group_version {
            Some(b) => {
                write_atomic(&copy, b)?;
                entry.claude_md = Some(sha256_hex(b));
            }
            None => {
                if exists_any(&copy) {
                    remove_counted(&copy)?;
                }
                entry.claude_md = None;
            }
        }
        let mut report = ApplyReport::default();
        update_claude_md_block(paths, &mut m, &mut backups, &mut report)?;
        return m.save(paths);
    }
    let added = match m.files.get(path) {
        Some(mf) if mf.group == *group => false,
        None if marketplaces::is_new_file(paths, &m, group, path) => true,
        _ => {
            return Err(crate::Error::Invalid(format!(
                "{path} is not managed for group {group}"
            )))
        }
    };
    let (root, sub) = locate(paths, path);
    if blocked_by_user(root, &sub) {
        return Err(crate::Error::Invalid(format!(
            "{path} is behind a link or a file the user owns"
        )));
    }
    if added {
        // A file added on this device and never shared: keep a copy before removing it.
        if let Some(cur) = read_file(&target_path(root, &sub))? {
            backups.save(path, &cur)?;
        }
    }
    match group_version {
        Some(b) => {
            write_tree_file(root, &sub, b, &mut m.created_dirs)?;
            m.files.insert(
                path.to_string(),
                ManagedFile {
                    group: group.clone(),
                    sha256: sha256_hex(b),
                },
            );
        }
        None => {
            let target = target_path(root, &sub);
            if exists_any(&target) {
                remove_counted(&target)?;
            }
            m.files.remove(path);
            prune_dirs(paths, &mut m)?;
        }
    }
    m.save(paths)
}

// ---------------------------------------------------------------- CLAUDE.md block

/// Rewrite the managed block in `~/.claude/CLAUDE.md` from the manifest: one import line
/// per group with a `CLAUDE.md` copy, in join order. Writes only when it changes.
fn update_claude_md_block(
    paths: &Paths,
    m: &mut Manifest,
    backups: &mut Backups,
    report: &mut ApplyReport,
) -> crate::Result<()> {
    let mut groups: Vec<GroupId> = m.order.clone();
    for k in m.groups.keys() {
        if let Some(g) = GroupId::parse(k) {
            if !groups.contains(&g) {
                groups.push(g);
            }
        }
    }
    let lines: Vec<String> = groups
        .iter()
        .filter(|g| {
            m.groups
                .get(&g.to_string())
                .is_some_and(|e| e.claude_md.is_some())
        })
        .filter(|g| exists_any(&paths.group_dir(g).join(CLAUDE_MD)))
        .map(|g| import_line(&paths.group_dir(g).join(CLAUDE_MD), home_dir().as_deref()))
        .collect();
    let file = paths.claude_home.join(CLAUDE_MD);
    let existing = read_file(&file)?;
    let new = render_claude_md(existing.as_deref().unwrap_or_default(), &lines);
    match existing {
        None if lines.is_empty() => {}
        None => {
            write_atomic(&file, &new)?;
            m.created_claude_md = true;
            push_unique(&mut report.written, CLAUDE_MD);
        }
        Some(old) if old == new => {}
        Some(old) => {
            backups.save(CLAUDE_MD, &old)?;
            if new.is_empty() && m.created_claude_md {
                remove_counted(&file)?;
                m.created_claude_md = false;
            } else {
                write_atomic(&file, &new)?;
            }
            push_unique(&mut report.written, CLAUDE_MD);
        }
    }
    // No block left: whatever remains is the user's file.
    if lines.is_empty() {
        m.created_claude_md = false;
    }
    Ok(())
}

/// `@~/...` (forward slashes) when `file` is under `home`, else `@<absolute path>`.
pub fn import_line(file: &Path, home: Option<&Path>) -> String {
    let slashes = |p: &Path| {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    if let Some(rel) = home.and_then(|h| file.strip_prefix(h).ok()) {
        return format!("@~/{}", slashes(rel).join("/"));
    }
    format!("@{}", file.to_string_lossy().replace('\\', "/"))
}

fn home_dir() -> Option<PathBuf> {
    let vars: &[&str] = if cfg!(windows) {
        &["USERPROFILE", "HOME"]
    } else {
        &["HOME"]
    };
    vars.iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

fn eol_len_at(text: &[u8], at: usize) -> usize {
    match text.get(at..) {
        Some([b'\r', b'\n', ..]) => 2,
        Some([b'\n', ..]) => 1,
        _ => 0,
    }
}

/// Byte range of the managed block, including the line end after the end marker.
fn find_block(text: &[u8]) -> Option<(usize, usize)> {
    let mut from = 0;
    loop {
        let s = find(text, BLOCK_BEGIN.as_bytes(), from)?;
        if s == 0 || text[s - 1] == b'\n' {
            let e = find(text, BLOCK_END.as_bytes(), s)? + BLOCK_END.len();
            return Some((s, e + eol_len_at(text, e)));
        }
        from = s + 1;
    }
}

/// The lines inside the managed block of `text` (none without a block), line ends trimmed.
pub fn block_lines(text: &[u8]) -> Vec<String> {
    let Some((s, e)) = find_block(text) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&text[s..e])
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| *l != BLOCK_BEGIN && *l != BLOCK_END)
        .map(str::to_string)
        .collect()
}

/// `existing` with the block set to `lines` (no lines: block removed). Everything outside
/// the block is kept byte for byte. A new block goes at the top, followed by one blank
/// line when the file has other content; removing the block removes that line again.
/// The block uses the file's line ends (CRLF if its first line ends with CRLF).
pub fn render_claude_md(existing: &[u8], lines: &[String]) -> Vec<u8> {
    let eol: &[u8] = match find(existing, b"\n", 0) {
        Some(i) if i > 0 && existing[i - 1] == b'\r' => b"\r\n",
        _ => b"\n",
    };
    let mut block = Vec::new();
    if !lines.is_empty() {
        block.extend_from_slice(BLOCK_BEGIN.as_bytes());
        block.extend_from_slice(eol);
        for l in lines {
            block.extend_from_slice(l.as_bytes());
            block.extend_from_slice(eol);
        }
        block.extend_from_slice(BLOCK_END.as_bytes());
        block.extend_from_slice(eol);
    }
    let mut out = Vec::with_capacity(existing.len() + block.len() + 2);
    match find_block(existing) {
        Some((s, e)) => {
            out.extend_from_slice(&existing[..s]);
            let mut after = &existing[e..];
            if block.is_empty() && s == 0 {
                after = &after[eol_len_at(after, 0)..];
            }
            out.extend_from_slice(&block);
            out.extend_from_slice(after);
        }
        None if block.is_empty() => out.extend_from_slice(existing),
        None => {
            out.extend_from_slice(&block);
            if !existing.is_empty() {
                out.extend_from_slice(eol);
                out.extend_from_slice(existing);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- paths and files

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let d = sha2::Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn is_tree_path(rel: &str) -> bool {
    [
        "skills/",
        "agents/",
        "commands/",
        "output-styles/",
        "plugins/",
    ]
    .iter()
    .any(|p| rel.starts_with(p))
}

/// A path the apply layer places: `skills/<name>/**`, a `.md` file under `agents/`,
/// `commands/` or `output-styles/`, or `plugins/<marketplace>/**`, with no segment that
/// could escape the folder or mean something special on Windows.
pub fn is_managed_target(rel: &str) -> bool {
    let segs: Vec<&str> = rel.split('/').collect();
    if !segs.iter().all(|s| safe_segment(s)) {
        return false;
    }
    match segs[0] {
        "skills" => segs.len() >= 3,
        "agents" | "commands" | "output-styles" => segs.len() >= 2 && rel.ends_with(".md"),
        group::PLUGINS => group::marketplace_of(rel).is_some(),
        _ => false,
    }
}

/// Where a managed path lives: `plugins/<name>/...` under `<state_dir>/marketplaces/<name>/...`,
/// everything else under `~/.claude`. Returns the root and the path below it.
fn locate<'a>(paths: &'a Paths, rel: &str) -> (&'a Path, String) {
    match rel.strip_prefix("plugins/") {
        Some(rest) => (&paths.state_dir, format!("{MARKETPLACES}/{rest}")),
        None => (&paths.claude_home, rel.to_string()),
    }
}

/// The file on this machine for a managed path (see [`locate`]).
pub fn target_of(paths: &Paths, rel: &str) -> PathBuf {
    let (root, sub) = locate(paths, rel);
    target_path(root, &sub)
}

/// The root a `created_dirs` entry is relative to.
fn dir_root<'a>(paths: &'a Paths, dir: &str) -> &'a Path {
    if dir.starts_with(&format!("{MARKETPLACES}/")) {
        &paths.state_dir
    } else {
        &paths.claude_home
    }
}

/// The report wrote or removed a plugin file.
fn touches_plugins(report: &ApplyReport) -> bool {
    report
        .written
        .iter()
        .chain(&report.removed)
        .any(|p| group::marketplace_of(p).is_some())
}

pub(crate) fn safe_segment(s: &str) -> bool {
    if s.is_empty() || s == "." || s == ".." || s.ends_with('.') || s.ends_with(' ') {
        return false;
    }
    if s.chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return false;
    }
    let stem = s.split('.').next().unwrap_or(s).to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    !reserved
}

fn target_path(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s))
}

/// True when a folder on the way to `rel` is a link (symlink or junction) or a file: the
/// app never writes through it.
pub(crate) fn blocked_by_user(root: &Path, rel: &str) -> bool {
    let segs: Vec<&str> = rel.split('/').collect();
    let mut cur = root.to_path_buf();
    for s in &segs[..segs.len().saturating_sub(1)] {
        cur.push(s);
        match fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_symlink() || !md.is_dir() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// Something other than a regular file (a folder, a link) is at `p`.
fn not_a_file(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|md| !md.is_file())
}

fn exists_any(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// File contents, `None` when missing. A folder or link in its place counts as missing
/// for reading; callers check `exists_any` before writing.
fn read_file(p: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(p) {
        Ok(md) if md.is_file() => fs::read(p).map(Some),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(p: &Path) -> crate::Result<Option<T>> {
    match read_file(p)? {
        Some(b) => Ok(Some(serde_json::from_slice(&b)?)),
        None => Ok(None),
    }
}

fn write_json<T: Serialize>(p: &Path, v: &T) -> crate::Result<()> {
    let mut b = serde_json::to_vec_pretty(v)?;
    b.push(b'\n');
    write_atomic(p, &b)?;
    Ok(())
}

fn write_tree_file(
    root: &Path,
    rel: &str,
    bytes: &[u8],
    created: &mut BTreeSet<String>,
) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let segs: Vec<&str> = rel.split('/').collect();
    let mut cur = root.to_path_buf();
    for i in 0..segs.len() - 1 {
        cur.push(segs[i]);
        if !cur.exists() {
            fs::create_dir(&cur)?;
            // Top-level folders (`skills`, `agents`, ...) are left in place later.
            if i > 0 {
                created.insert(segs[..=i].join("/"));
            }
        }
    }
    write_atomic(&cur.join(segs[segs.len() - 1]), bytes)
}

enum Deleted {
    Yes,
    Gone,
    Edited,
}

fn delete_if_unchanged(root: &Path, rel: &str, sha: &str) -> io::Result<Deleted> {
    if blocked_by_user(root, rel) {
        return Ok(Deleted::Gone);
    }
    let target = target_path(root, rel);
    match read_file(&target)? {
        None => Ok(Deleted::Gone),
        Some(cur) if sha256_hex(&cur) == sha => {
            remove_counted(&target)?;
            Ok(Deleted::Yes)
        }
        Some(_) => Ok(Deleted::Edited),
    }
}

/// Remove folders the app created that are empty now, deepest first.
fn prune_dirs(paths: &Paths, m: &mut Manifest) -> io::Result<()> {
    let mut dirs: Vec<String> = m.created_dirs.iter().cloned().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
    for d in dirs {
        let p = target_path(dir_root(paths, &d), &d);
        match fs::symlink_metadata(&p) {
            Ok(md) if md.is_dir() && !md.file_type().is_symlink() => {
                if fs::read_dir(&p)?.next().is_none() {
                    fs::remove_dir(&p)?;
                    count_write();
                    m.created_dirs.remove(&d);
                }
            }
            // Gone, or replaced by something the user owns: stop tracking it.
            _ => {
                m.created_dirs.remove(&d);
            }
        }
    }
    Ok(())
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|x| x == s) {
        v.push(s.to_string());
    }
}

/// Write to a temp file in the same folder, then rename over the target.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.claude-sync-tmp-{}", std::process::id()));
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        // Windows refuses the rename while another process has the target open
        // without delete sharing; that clears quickly.
        let mut tries = 0;
        loop {
            match fs::rename(&tmp, path) {
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied && tries < 5 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                r => return r,
            }
        }
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    count_write();
    res
}

fn remove_counted(p: &Path) -> io::Result<()> {
    count_write();
    fs::remove_file(p)
}

#[cfg(test)]
thread_local! {
    static WRITES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn count_write() {
    #[cfg(test)]
    WRITES.with(|w| w.set(w.get() + 1));
}

/// Writes and deletes done by this thread so far (tests only).
#[cfg(test)]
pub(crate) fn writes() -> usize {
    WRITES.with(|w| w.get())
}

/// UTC time as `20261013T120000Z` (no `:`, which Windows forbids in names).
pub(crate) fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!(
        "{y:04}{mo:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// One `backups/<UTC time>/` folder per operation, created on the first save.
/// A path saved twice in one operation keeps its first (original) copy.
struct Backups {
    root: PathBuf,
    dir: Option<PathBuf>,
    saved: BTreeSet<String>,
}

impl Backups {
    fn new(paths: &Paths) -> Self {
        Self {
            root: paths.state_dir.join("backups"),
            dir: None,
            saved: BTreeSet::new(),
        }
    }
    fn save(&mut self, rel: &str, bytes: &[u8]) -> io::Result<()> {
        if !self.saved.insert(rel.to_string()) {
            return Ok(());
        }
        if self.dir.is_none() {
            let stamp = utc_stamp();
            let mut dir = self.root.join(&stamp);
            let mut n = 0;
            while dir.exists() {
                n += 1;
                dir = self.root.join(format!("{stamp}-{n}"));
            }
            fs::create_dir_all(&dir)?;
            self.dir = Some(dir);
        }
        let dir = self.dir.as_ref().expect("set above");
        write_atomic(&target_path(dir, rel), bytes)
    }
}

mod marketplaces;

#[cfg(test)]
mod tests;
