//! Applying a group to the machine: the CLAUDE.md copy and the managed block in
//! `~/.claude/CLAUDE.md`, the settings merge into `~/.claude/settings.json`, and the
//! marketplace `COMMIT` marker. Group files themselves go through `FsDisk` and the executor.
//!
//! v2-Parity rows covered: 7, 25, 37, 43.
use crate::atomic::{blocked, read_file, sha256_hex, write_atomic};
use crate::disk::{Home, Scope};
use crate::exec::utc_stamp;
use claude_sync_engine::claude_md::{compose_block, GroupCopy};
use claude_sync_engine::formats::text::CommitMarker;
use claude_sync_engine::settings::{merge, unapply, Owned};
use claude_sync_engine::settings_edits::{edited_pointers, keep_overrides, restore};
use claude_sync_engine::traits::Store;
use claude_sync_store::Db;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub fn copy_path(home: &Home, repo: &str, file: &str) -> PathBuf {
    home.state
        .join("groups")
        .join(Scope::Group(repo.into()).dir())
        .join(file)
}

/// The group's CLAUDE.md copy: written when it differs, removed when the group dropped it.
pub fn write_claude_md_copy(home: &Home, repo: &str, text: Option<&[u8]>) -> std::io::Result<()> {
    let p = copy_path(home, repo, "CLAUDE.md");
    match text {
        Some(t) if read_file(&p)?.as_deref() != Some(t) => write_atomic(&p, t),
        None if p.exists() => std::fs::remove_file(&p),
        _ => Ok(()),
    }
}

fn backup(home: &Home, now: i64, rel: &str, bytes: &[u8]) -> std::io::Result<()> {
    let p = home.state.join("backups").join(utc_stamp(now)).join(rel);
    if p.exists() {
        return Ok(());
    }
    write_atomic(&p, bytes)
}

/// Rewrite the import block of `~/.claude/CLAUDE.md`: one line per group (in the order given)
/// that is not disabled and has a copy. A file the app created and the block emptied goes
/// away. Returns whether the file changed.
pub fn update_block(home: &Home, groups: &[(String, bool)], now: i64) -> std::io::Result<bool> {
    let path = home.claude.join("CLAUDE.md");
    if blocked(&home.claude, &path)
        || std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink())
    {
        return Ok(false); // never written through a link
    }
    let mut copies: Vec<GroupCopy> = vec![];
    for (r, disabled) in groups {
        let f = copy_path(home, r, "CLAUDE.md");
        copies.push(GroupCopy {
            present: f.is_file(),
            file: f.to_string_lossy().into_owned(),
            disabled: *disabled,
        });
        // the CLAUDE.md of each pack that is on in this group (Parity 79), after the group's own
        let list = std::fs::read_to_string(copy_path(home, r, "pack-imports")).unwrap_or_default();
        copies.extend(list.lines().filter(|l| !l.is_empty()).map(|l| GroupCopy {
            present: std::path::Path::new(l).is_file(),
            file: l.to_string(),
            disabled: *disabled,
        }));
    }
    let existing = read_file(&path)?;
    let Some(out) = compose_block(existing.as_deref(), &copies, home.home.to_str()) else {
        return Ok(false);
    };
    let marker = home.state.join("claude-md-created");
    if let Some(old) = &existing {
        backup(home, now, "CLAUDE.md", old)?;
    }
    if out.is_empty() && (existing.is_none() || marker.exists()) {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&marker);
        return Ok(existing.is_some());
    }
    if existing.is_none() {
        write_atomic(&marker, b"")?;
    }
    write_atomic(&path, &out)?;
    Ok(true)
}

/// Add `line` to the person's own text in `~/.claude/CLAUDE.md` (after the managed block, never
/// through a link). A line that is already there is not added twice. Returns whether it wrote.
pub fn append_own_line(home: &Home, line: &str, now: i64) -> std::io::Result<bool> {
    let path = home.claude.join("CLAUDE.md");
    let line = line.trim();
    if line.is_empty()
        || blocked(&home.claude, &path)
        || std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink())
    {
        return Ok(false);
    }
    let existing = read_file(&path)?.unwrap_or_default();
    let text = String::from_utf8_lossy(&existing).into_owned();
    if text.lines().any(|l| l.trim() == line) {
        return Ok(false);
    }
    if !existing.is_empty() {
        backup(home, now, "CLAUDE.md", &existing)?;
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lead = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        eol
    };
    write_atomic(&path, format!("{text}{lead}{line}{eol}").as_bytes())?;
    Ok(true)
}

/// Replace the first line of `~/.claude/CLAUDE.md` equal to `line` with the lines of `text`
/// (none removes it), backing the file up first. Returns false when the line is not there.
pub fn replace_own_line(home: &Home, line: &str, text: &str, now: i64) -> std::io::Result<bool> {
    let path = home.claude.join("CLAUDE.md");
    if blocked(&home.claude, &path)
        || std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink())
    {
        return Ok(false);
    }
    let Some(existing) = read_file(&path)? else {
        return Ok(false);
    };
    let old = String::from_utf8_lossy(&existing).into_owned();
    let eol = if old.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = old.lines().collect();
    let Some(at) = lines.iter().position(|l| l.trim() == line.trim()) else {
        return Ok(false);
    };
    backup(home, now, "CLAUDE.md", &existing)?;
    let mut out: Vec<&str> = lines[..at].to_vec();
    out.extend(text.lines());
    out.extend(&lines[at + 1..]);
    let body = out.join(eol);
    let body = if body.is_empty() {
        body
    } else {
        format!("{body}{eol}")
    };
    write_atomic(&path, body.as_bytes())?;
    Ok(true)
}

fn owned_key(repo: &str) -> String {
    format!("owned/{repo}")
}

pub fn load_owned(db: &Db, repo: &str) -> Owned {
    db.read(&owned_key(repo))
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_owned(db: &Db, repo: &str, o: &Owned) -> Result<(), String> {
    let b = serde_json::to_vec(o).map_err(|e| e.to_string())?;
    db.write(&owned_key(repo), &b).map_err(|e| e.to_string())
}

/// The user's settings.json as an object (`{}` when missing); v1's messages when it cannot be
/// merged into.
fn read_settings(home: &Home) -> Result<(Value, Option<Vec<u8>>, PathBuf), String> {
    let p = home.claude.join("settings.json");
    let no = |why: &str| format!("{why}; settings.json was not changed");
    match std::fs::symlink_metadata(&p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Value::Object(Default::default()), None, p))
        }
        Err(e) => return Err(no(&format!("could not read settings.json ({e})"))),
        Ok(m) if m.file_type().is_symlink() || !m.is_file() => {
            return Err(no(
                "settings.json is a link or not a regular file, so nothing was merged",
            ))
        }
        Ok(_) => {}
    }
    let raw = std::fs::read(&p).map_err(|e| no(&format!("could not read settings.json ({e})")))?;
    match serde_json::from_slice::<Value>(&raw) {
        Ok(v) if v.is_object() => Ok((v, Some(raw), p)),
        _ => Err(crate::NOT_OBJECT_LOCAL.into()),
    }
}

fn put_settings(
    home: &Home,
    now: i64,
    v: &Value,
    old: Option<&Vec<u8>>,
    p: &Path,
) -> Result<bool, String> {
    let mut b = serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?;
    b.push(b'\n');
    if old == Some(&b) {
        return Ok(false);
    }
    if let Some(o) = old {
        backup(home, now, "settings.json", o).map_err(|e| e.to_string())?;
    }
    write_atomic(p, &b).map_err(|e| e.to_string())?;
    Ok(true)
}

pub struct SettingsOutcome {
    pub changed: bool,
    pub notes: Vec<String>,
}

/// Merge the group's `settings.shared.json` (`None`: the group dropped it) into the user's
/// settings.json. `taken`: pointers earlier-ranked groups own, with the group's name.
/// `pinned`: plugin pointers of packs that are on here. A mods-canary trial value stays.
pub fn apply_settings(
    home: &Home,
    db: &Db,
    repo: &str,
    shared: Option<&[u8]>,
    taken: &BTreeMap<String, String>,
    pinned: &BTreeSet<String>,
    now: i64,
) -> Result<SettingsOutcome, String> {
    let prev = load_owned(db, repo);
    let shared_v: Value = match shared {
        Some(b) => match serde_json::from_slice::<Value>(b) {
            Ok(v) if v.is_object() => v,
            _ => {
                return Err("settings.shared.json is not one JSON object, so nothing was merged; settings.json was not changed".into())
            }
        },
        None if prev.values.is_empty() && prev.array_items.is_empty() => {
            return Ok(SettingsOutcome { changed: false, notes: vec![] })
        }
        None => Value::Object(Default::default()),
    };
    let (local, raw, p) = read_settings(home)?;
    let m = merge(&local, &shared_v, &prev, taken)?;
    let mut out = m.settings;
    keep_overrides(&local, &mut out, pinned);
    let changed = put_settings(home, now, &out, raw.as_ref(), &p)?;
    save_owned(db, repo, &m.owned)?;
    Ok(SettingsOutcome {
        changed,
        notes: m.notes,
    })
}

/// Take a group's values out of settings.json (group off or left).
pub fn unapply_settings(home: &Home, db: &Db, repo: &str, now: i64) -> Result<bool, String> {
    let prev = load_owned(db, repo);
    if prev.values.is_empty() && prev.array_items.is_empty() {
        return Ok(false);
    }
    let (local, raw, p) = read_settings(home)?;
    let m = unapply(&local, &prev)?;
    let changed = put_settings(home, now, &m.settings, raw.as_ref(), &p)?;
    save_owned(db, repo, &Owned::default())?;
    Ok(changed)
}

/// Owned pointers the user edited or deleted (kept as they are; shown as local edits).
pub fn edited(
    home: &Home,
    db: &Db,
    repo: &str,
    pinned: &BTreeSet<String>,
) -> Result<Vec<(String, bool)>, String> {
    let (local, _, _) = read_settings(home)?;
    Ok(edited_pointers(&local, &load_owned(db, repo), pinned))
}

/// Undo: put the group's values back at `under` (`""`: everywhere).
pub fn restore_settings(
    home: &Home,
    db: &Db,
    repo: &str,
    under: &str,
    now: i64,
) -> Result<bool, String> {
    let (local, raw, p) = read_settings(home)?;
    let back = restore(&local, &load_owned(db, repo), under);
    put_settings(home, now, &back, raw.as_ref(), &p)
}

/// `<state>/marketplaces/<mp>/COMMIT` for every marketplace in `paths`, rewritten only when the
/// head changes.
pub fn write_commit_markers(
    home: &Home,
    paths: &BTreeSet<&String>,
    sha: &str,
    date: Option<&str>,
) -> std::io::Result<()> {
    let mps: BTreeSet<&str> = paths
        .iter()
        .filter_map(|p| p.strip_prefix("plugins/")?.split('/').next())
        .collect();
    let want = CommitMarker {
        commit: sha.into(),
        date: date.map(String::from),
    }
    .to_bytes();
    for mp in mps {
        let f = home.state.join("marketplaces").join(mp).join("COMMIT");
        let have = read_file(&f)?.and_then(|b| CommitMarker::parse(&b));
        if have.as_ref().map(|m| m.commit.as_str()) != Some(sha) {
            write_atomic(&f, &want)?;
        }
    }
    Ok(())
}

pub fn fingerprint(b: &[u8]) -> String {
    sha256_hex(b)
}
