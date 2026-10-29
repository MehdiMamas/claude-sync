//! `~/.claude/settings.json` as a personal file `settings.json`.
//!
//! What syncs is a view of the real file: without the values a group set (the group wins on
//! those), without secrets, and without anything that cannot work on another machine (see
//! `settings::machine_specific`). Between a person's devices it merges per JSON pointer.
//! Applying puts the merged view back into the real file and leaves everything else in it.
use crate::apply::{load_owned, SHARED_SETTINGS};
use crate::settings::{self, filter_blocked, leaves, remove_at, set_at, sorted};
use crate::{FileSet, Paths};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const PATH: &str = "settings.json";

/// What groups set, so the personal copy leaves it alone.
#[derive(Debug, Clone, Default)]
pub struct Skip {
    /// Pointers of plain values.
    pub plain: BTreeSet<String>,
    /// Array pointer -> items a group added.
    pub items: BTreeMap<String, Vec<Value>>,
}

/// Everything a group's `settings.shared.json` sets or its merge owns.
pub fn skip_set(paths: &Paths) -> crate::Result<Skip> {
    let mut skip = Skip::default();
    for owned in load_owned(paths)?.into_values() {
        skip.plain.extend(owned.values.into_keys());
        for (ptr, items) in owned.array_items {
            skip.items.entry(ptr).or_default().extend(items);
        }
    }
    if let Ok(dirs) = std::fs::read_dir(paths.state_dir.join("groups")) {
        for dir in dirs.flatten() {
            let Ok(bytes) = std::fs::read(dir.path().join(SHARED_SETTINGS)) else {
                continue;
            };
            let Some(shared) = settings::parse_object(&bytes) else {
                continue;
            };
            for (ptr, v) in leaves(&shared) {
                match v {
                    Value::Array(items) => skip.items.entry(ptr).or_default().extend(items),
                    _ => {
                        skip.plain.insert(ptr);
                    }
                }
            }
        }
    }
    Ok(skip)
}

/// What the real file shares: sorted, and without group values, secrets and machine paths.
pub fn view(real: &Value, skip: &Skip) -> Value {
    let canary = settings::canary_pointers(real);
    let mut v = real.clone();
    for ptr in skip.plain.iter().chain(canary.iter()) {
        remove_at(&mut v, ptr);
    }
    for (ptr, items) in &skip.items {
        let Some(Value::Array(a)) = v.pointer_mut(ptr) else {
            continue;
        };
        a.retain(|x| !items.contains(x));
        if a.is_empty() {
            remove_at(&mut v, ptr);
        }
    }
    let mut v = filter_blocked(&v, &mut Vec::new());
    settings::machine_specific(&mut v);
    sorted(&v)
}

/// A shareable file, or `None` when it would be empty.
pub fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    (!leaves(v).is_empty()).then(|| settings::to_bytes(&sorted(v)))
}

fn parse(bytes: Option<&Vec<u8>>) -> Value {
    bytes
        .and_then(|b| settings::parse_object(b))
        .unwrap_or_else(|| Value::Object(Map::new()))
}

/// Per-pointer three-way merge. One side changed: that side. Both changed the same pointer:
/// the version already pushed (`remote`) wins, unless it deleted the value, then the change
/// stays. `None` when nothing is left.
pub fn merge(
    base: Option<&Vec<u8>>,
    local: Option<&Vec<u8>>,
    remote: Option<&Vec<u8>>,
) -> Option<Vec<u8>> {
    let (b, l, r) = (
        leaves(&parse(base)),
        leaves(&parse(local)),
        leaves(&parse(remote)),
    );
    let keys: BTreeSet<&String> = b.keys().chain(l.keys()).chain(r.keys()).collect();
    let mut out = Value::Object(Map::new());
    for k in keys {
        let (bv, lv, rv) = (b.get(k), l.get(k), r.get(k));
        let pick = if lv == rv || rv == bv {
            lv
        } else if lv == bv {
            rv
        } else {
            rv.or(lv)
        };
        if let Some(v) = pick {
            set_at(&mut out, k, v.clone());
        }
    }
    bytes_of(&out)
}

/// Keep what a group sets as the last sync had it, so setting it here is not a change to push.
pub fn keep_skipped(local: &mut FileSet, base: &FileSet, skip: &Skip) {
    let b = leaves(&parse(base.get(PATH)));
    let mut l = parse(local.get(PATH));
    for ptr in &skip.plain {
        match b.get(ptr) {
            Some(v) => {
                set_at(&mut l, ptr, v.clone());
            }
            None => remove_at(&mut l, ptr),
        }
    }
    match bytes_of(&l) {
        Some(bytes) => local.insert(PATH.into(), bytes),
        None => local.remove(PATH),
    };
}

/// `real` after moving it from the view it had (`old`) to the merged one (`new`). Values
/// groups set, secrets and machine paths in `new` are never applied. A value is removed
/// only when `old` had it, so what the person added since the scan stays.
pub fn apply(real: &mut Value, old: Option<&Vec<u8>>, new: Option<&Vec<u8>>, skip: &Skip) {
    let old = leaves(&parse(old));
    let mut want = parse(new);
    let mut blocked = Vec::new();
    want = filter_blocked(&want, &mut blocked);
    settings::machine_specific(&mut want);
    let want = leaves(&want);
    for (ptr, v) in &old {
        if want.contains_key(ptr) || skip.plain.contains(ptr) || real.pointer(ptr) != Some(v) {
            continue;
        }
        match (skip.items.get(ptr), real.pointer_mut(ptr)) {
            // Only the group's items stay in an array it added to.
            (Some(group), Some(Value::Array(cur))) => {
                cur.retain(|x| group.contains(x));
                if cur.is_empty() {
                    remove_at(real, ptr);
                }
            }
            _ => remove_at(real, ptr),
        }
    }
    for (ptr, v) in want {
        if skip.plain.contains(&ptr) || (v.is_object() && real.pointer(&ptr).is_some()) {
            continue;
        }
        let v = match (v, skip.items.get(&ptr)) {
            (Value::Array(mut a), Some(group)) => {
                if let Some(Value::Array(cur)) = real.pointer(&ptr) {
                    let extra: Vec<Value> = cur
                        .iter()
                        .filter(|x| group.contains(x) && !a.contains(x))
                        .cloned()
                        .collect();
                    a.extend(extra);
                }
                Value::Array(a)
            }
            (v, _) => v,
        };
        if real.pointer(&ptr) != Some(&v) {
            set_at(real, &ptr, v);
        }
    }
}
