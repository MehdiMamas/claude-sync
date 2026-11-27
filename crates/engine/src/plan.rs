//! The planner: `plan(remote, base, local, opts)` decides, per file, what to write locally,
//! what to push, and what clashes. Pure: three byte maps in, a [`Plan`] out.
//!
//! Parity: three-way sync of group files and personal files (what the group changed, what
//! the user changed, conflicts remote-wins with the local copy kept in `conflicts`, local
//! edits kept), secret scan before push, MEMORY.md line merge (CRLF vs LF alone is no
//! change, pushed bytes are LF), personal 256 KiB per file / 16 MiB total, no write outside
//! managed folders or through a link. A file the user changed is never deleted.
//!
//! v2-Parity rows covered: 24, 25, 29, 31, 39, 43, 46.
use crate::formats::group::{SyncFlags, CHANGE_NOTES_PATH, SHARED_SETTINGS};
use crate::formats::personal::{MAX_FILE, MAX_TOTAL};
use crate::merge::{merge_choices_bytes, merge_json_bytes, merge_lines};
use crate::paths::{allowed_by, is_memory_index, is_personal_path, is_planned_path, under_link};
use crate::secrets::{scan, Hit};
use std::collections::{BTreeMap, BTreeSet};

pub type FileSet = BTreeMap<String, Vec<u8>>;

/// Most conflicts a caller keeps per pass.
pub const MAX_CONFLICTS: usize = 50;

#[derive(Debug, Clone)]
pub enum Scope {
    /// A group repo: managed folders and `settings.shared.json`, limited by the sync switches.
    Group(SyncFlags),
    /// The personal repo.
    Personal,
}

#[derive(Debug, Clone)]
pub struct Opts {
    pub scope: Scope,
    /// May this device push (host, or personal sync)? Members only receive.
    pub push: bool,
    /// Repo paths known to be links on disk; nothing is planned through them.
    pub links: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub path: String,
    /// The user's copy that lost (`None`: the user had deleted the file).
    pub local: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Files to write on this device.
    pub writes: FileSet,
    /// Files to delete on this device; only ones the user had not changed.
    pub deletes: BTreeSet<String>,
    /// Files to push (`None`: delete in the repo).
    pub pushes: BTreeMap<String, Option<Vec<u8>>>,
    pub conflicts: Vec<Conflict>,
    /// Paths the user changed and that stay local (nothing pushed).
    pub local_edits: Vec<String>,
    /// Pushes withheld because the text looks like a secret.
    pub secrets: Vec<(String, Hit)>,
    /// Plain-language notes (skipped files).
    pub notes: Vec<String>,
    /// What the base becomes once the plan is applied.
    pub new_base: FileSet,
}

impl Plan {
    /// Nothing to apply or push.
    pub fn is_noop(&self) -> bool {
        self.writes.is_empty()
            && self.deletes.is_empty()
            && self.pushes.is_empty()
            && self.conflicts.is_empty()
    }
}

fn strip_cr(b: &[u8]) -> Vec<u8> {
    b.iter().copied().filter(|c| *c != b'\r').collect()
}

fn lf(s: &str) -> Vec<u8> {
    s.replace("\r\n", "\n").into_bytes()
}

fn accepts(o: &Opts, p: &str) -> bool {
    if under_link(p, &o.links) {
        return false;
    }
    match &o.scope {
        Scope::Group(f) => p != CHANGE_NOTES_PATH && is_planned_path(p) && allowed_by(f, p),
        Scope::Personal => is_personal_path(p),
    }
}

pub fn plan(remote: &FileSet, base: &FileSet, local: &FileSet, o: &Opts) -> Plan {
    let personal = matches!(o.scope, Scope::Personal);
    let mut out = Plan::default();
    let paths: BTreeSet<&String> = remote
        .keys()
        .chain(base.keys())
        .chain(local.keys())
        .collect();
    let mut total = 0usize;
    for p in paths {
        if !accepts(o, p) {
            continue;
        }
        let (b, l, r) = (base.get(p), local.get(p), remote.get(p));
        if personal && l.is_some_and(|x| x.len() > MAX_FILE) {
            out.notes
                .push(format!("{p}: not synced, it is over 256 KiB"));
            continue; // stays local, not tracked
        }
        let keep = |out: &mut Plan, v: Option<&Vec<u8>>| {
            if let Some(v) = v {
                out.new_base.insert(p.clone(), v.clone());
            }
        };
        let same = match (l, r) {
            (Some(a), Some(c)) => a == c || (personal && strip_cr(a) == strip_cr(c)),
            (None, None) => true,
            _ => false,
        };
        if same {
            keep(&mut out, r);
        } else if l == b
            || (personal && matches!((l, b), (Some(x), Some(y)) if strip_cr(x) == strip_cr(y)))
        {
            // user untouched: take the group's version (the only case that deletes)
            match r {
                Some(v) => {
                    out.writes.insert(p.clone(), v.clone());
                }
                None => {
                    out.deletes.insert(p.clone());
                }
            }
            keep(&mut out, r);
        } else if r == b
            || (personal && matches!((r, b), (Some(x), Some(y)) if strip_cr(x) == strip_cr(y)))
        {
            push_edit(&mut out, o, p, b, l, &mut total);
        } else {
            both_changed(&mut out, o, p, (b, l, r), &mut total);
        }
    }
    hold_secrets(&mut out, base);
    out
}

/// The one secret scan: every push, whichever branch made it, is scanned here. A hit stays
/// local (not pushed, base kept as it was so the next pass tries again).
fn hold_secrets(out: &mut Plan, base: &FileSet) {
    let hits: Vec<(String, Hit)> = out
        .pushes
        .iter()
        .filter_map(|(p, v)| {
            let hit = scan(&String::from_utf8_lossy(v.as_ref()?))
                .into_iter()
                .next()?;
            Some((p.clone(), hit))
        })
        .collect();
    for (p, hit) in hits {
        out.pushes.remove(&p);
        match base.get(&p) {
            Some(b) => out.new_base.insert(p.clone(), b.clone()),
            None => out.new_base.remove(&p),
        };
        if !out.local_edits.contains(&p) {
            out.local_edits.push(p.clone());
        }
        out.secrets.push((p, hit));
    }
}

/// The user changed `p`, the group did not.
fn push_edit(
    out: &mut Plan,
    o: &Opts,
    p: &str,
    b: Option<&Vec<u8>>,
    l: Option<&Vec<u8>>,
    total: &mut usize,
) {
    let stay = |out: &mut Plan| {
        out.local_edits.push(p.to_string());
        if let Some(b) = b {
            out.new_base.insert(p.to_string(), b.clone());
        }
    };
    if !o.push {
        return stay(out);
    }
    let Some(l) = l else {
        // the user deleted it: push the delete
        out.pushes.insert(p.to_string(), None);
        return;
    };
    let bytes = if is_memory_index(p) {
        lf(&String::from_utf8_lossy(l))
    } else {
        l.clone()
    };
    if matches!(o.scope, Scope::Personal) {
        if *total + bytes.len() > MAX_TOTAL {
            out.notes
                .push(format!("{p}: not synced, personal memory is over 16 MiB"));
            return stay(out);
        }
        *total += bytes.len();
    }
    out.new_base.insert(p.to_string(), bytes.clone());
    out.pushes.insert(p.to_string(), Some(bytes));
}

type Triple<'a> = (
    Option<&'a Vec<u8>>,
    Option<&'a Vec<u8>>,
    Option<&'a Vec<u8>>,
);

/// Both sides changed `p` since the base.
fn both_changed(out: &mut Plan, o: &Opts, p: &str, (b, l, r): Triple, total: &mut usize) {
    let personal = matches!(o.scope, Scope::Personal);
    if let (Some(l), Some(r)) = (l, r) {
        if o.push && p == SHARED_SETTINGS {
            if let Some(m) = merge_json_bytes(b.map(Vec::as_slice), l, r) {
                out.writes.insert(p.to_string(), m.clone());
                out.new_base.insert(p.to_string(), m.clone());
                if &m != r {
                    out.pushes.insert(p.to_string(), Some(m));
                }
                return;
            }
        }
        if personal && p == crate::formats::personal::CHOICES_PATH {
            if let Some(m) = merge_choices_bytes(l, r) {
                out.writes.insert(p.to_string(), m.clone());
                out.new_base.insert(p.to_string(), m.clone());
                if &m != r && o.push {
                    out.pushes.insert(p.to_string(), Some(m));
                }
                return;
            }
        }
        if is_memory_index(p) {
            let s = |x: &[u8]| String::from_utf8_lossy(x).into_owned();
            let merged = merge_lines(&b.map_or(String::new(), |x| s(x)), &s(l), &s(r));
            let pushed = lf(&merged);
            out.writes.insert(p.to_string(), merged.into_bytes());
            out.new_base.insert(p.to_string(), pushed.clone());
            if o.push && pushed != lf(&s(r)) && *total + pushed.len() <= MAX_TOTAL {
                *total += pushed.len();
                out.pushes.insert(p.to_string(), Some(pushed));
            }
            return;
        }
    }
    match (l, r) {
        (_, Some(r)) => {
            // the group wins; the user's copy is kept in the conflict list
            out.writes.insert(p.to_string(), r.clone());
            out.new_base.insert(p.to_string(), r.clone());
            out.conflicts.push(Conflict {
                path: p.to_string(),
                local: l.cloned(),
            });
        }
        (Some(l), None) => {
            // the group deleted it, the user changed it: the user's file is never deleted
            if o.push {
                out.pushes.insert(p.to_string(), Some(l.clone()));
                out.new_base.insert(p.to_string(), l.clone());
            } else {
                out.local_edits.push(p.to_string());
            }
        }
        (None, None) => {}
    }
}
