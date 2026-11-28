//! Merging a group's `settings.shared.json` into the user's `settings.json`, and the cut that
//! keeps machine-only values out of anything shared.
//!
//! Parity: group keys applied by JSON pointer; arrays get the shared items they lack; a value
//! the user edited (and the group did not change) stays; values the group stopped shipping
//! are removed only while still as the app set them; a key an earlier-ranked group owns is
//! skipped with a note; `apiKeyHelper`, secret-looking `env` values and hooks with absolute
//! paths are never applied; machine-specific values never travel.
//!
//! v2-Parity rows covered: 25, 43.
use crate::secrets::first_hit;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[allow(clippy::expect_used)]
static ABS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|[\s"'=(;,`])(?:[A-Za-z]:[\\/]|\\\\[^\\\s]|/[A-Za-z0-9._@+-]+/)"#)
        .expect("abs path pattern")
});

/// What the app set in `settings.json` for one group (by JSON pointer), so it can tell its
/// own values from the user's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Owned {
    pub values: BTreeMap<String, Value>,
    pub array_items: BTreeMap<String, Vec<Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    pub settings: Value,
    pub owned: Owned,
    pub notes: Vec<String>,
}

pub const NOT_OBJECT: &str =
    "settings.json is not one JSON object, so nothing was merged; settings.json was not changed";

fn esc(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

fn has_abs(v: &Value) -> bool {
    match v {
        Value::String(s) => ABS.is_match(s),
        Value::Array(a) => a.iter().any(has_abs),
        Value::Object(o) => o.values().any(has_abs),
        _ => false,
    }
}

/// Drop what must never be applied from a group; the notes say what and why.
pub fn filter_blocked(shared: &Value) -> (Value, Vec<String>) {
    let mut v = shared.clone();
    let mut notes = vec![];
    let Some(o) = v.as_object_mut() else {
        return (v, notes);
    };
    if o.remove("apiKeyHelper").is_some() {
        notes.push("apiKeyHelper: never shared; not applied".into());
    }
    if let Some(Value::Object(env)) = o.get_mut("env") {
        let bad: Vec<(String, &str)> = env
            .iter()
            .filter_map(|(k, x)| Some((k.clone(), first_hit(x.as_str()?)?)))
            .collect();
        for (k, pat) in bad {
            env.remove(&k);
            notes.push(format!("env.{k}: looks like a secret ({pat}); not applied"));
        }
    }
    if let Some(Value::Object(hooks)) = o.get_mut("hooks") {
        for (event, list) in hooks.iter_mut() {
            if let Value::Array(items) = list {
                let mut i = 0;
                items.retain(|it| {
                    let drop = has_abs(it);
                    if drop {
                        notes.push(format!(
                            "hooks.{event}[{i}]: command uses an absolute path; not applied"
                        ));
                    }
                    i += 1;
                    !drop
                });
            }
        }
    }
    (v, notes)
}

/// `settings` without what only works on this machine, for sharing through the personal repo.
pub fn machine_specific(settings: &Value) -> Value {
    fn prune(v: &mut Value) {
        match v {
            Value::Object(o) => {
                o.retain(|_, x| !has_abs(x) || x.is_object() || x.is_array());
                o.values_mut().for_each(prune);
            }
            Value::Array(a) => {
                a.retain(|x| !x.is_string() || !has_abs(x));
                a.iter_mut().for_each(prune);
            }
            _ => {}
        }
    }
    let mut v = settings.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("apiKeyHelper");
        if let Some(Value::Object(p)) = o.get_mut("permissions") {
            p.remove("additionalDirectories");
        }
        if let Some(Value::Object(m)) = o.get_mut("extraKnownMarketplaces") {
            m.retain(|_, x| {
                !matches!(
                    x.pointer("/source/source").and_then(Value::as_str),
                    Some("directory" | "file")
                )
            });
        }
    }
    prune(&mut v);
    v
}

pub(crate) fn set_at(root: &mut Value, ptr: &str, val: Value) {
    let keys: Vec<String> = ptr
        .split('/')
        .skip(1)
        .map(|k| k.replace("~1", "/").replace("~0", "~"))
        .collect();
    let mut cur = root;
    for (i, k) in keys.iter().enumerate() {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        let Some(o) = cur.as_object_mut() else { return };
        if i + 1 == keys.len() {
            o.insert(k.clone(), val);
            return;
        }
        cur = o
            .entry(k.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}

pub(crate) fn remove_at(root: &mut Value, ptr: &str) {
    if let Some((parent, key)) = ptr.rsplit_once('/') {
        let key = key.replace("~1", "/").replace("~0", "~");
        let gone = match root.pointer_mut(parent) {
            Some(Value::Object(o)) => {
                o.remove(&key);
                o.is_empty()
            }
            _ => false,
        };
        if gone && !parent.is_empty() {
            remove_at(root, parent); // prune empty parent objects
        }
    }
}

struct Ctx<'a> {
    prev: &'a Owned,
    taken: &'a BTreeMap<String, String>,
    owned: Owned,
    notes: Vec<String>,
}

fn walk(c: &mut Ctx, local: &mut Value, shared: &Value, ptr: &str) {
    match shared {
        Value::Object(o) if !o.is_empty() => {
            for (k, v) in o {
                walk(c, local, v, &format!("{ptr}/{}", esc(k)));
            }
        }
        _ if ptr.is_empty() => {}
        _ if c.taken.contains_key(ptr) => {
            c.notes
                .push(format!("{ptr}: set by group {}; not applied", c.taken[ptr]));
        }
        Value::Array(items) => match local.pointer_mut(ptr) {
            None => {
                set_at(local, ptr, shared.clone());
                c.owned.array_items.insert(ptr.into(), items.clone());
            }
            Some(Value::Array(have)) => {
                let add: Vec<Value> = items
                    .iter()
                    .filter(|i| !have.contains(i))
                    .cloned()
                    .collect();
                have.extend(add);
                c.owned.array_items.insert(ptr.into(), items.clone());
            }
            Some(_) => c.notes.push(format!("{ptr}: type differs; not applied")),
        },
        _ => {
            let cur = local.pointer(ptr).cloned();
            let set_by_app = c.prev.values.get(ptr);
            // the user edited it and the group did not change it: keep the edit, keep tracking
            if let (Some(cur), Some(prev)) = (&cur, set_by_app) {
                if prev == shared && cur != shared {
                    c.owned.values.insert(ptr.into(), prev.clone());
                    return;
                }
            }
            if matches!(cur, Some(Value::Object(_) | Value::Array(_))) {
                c.notes.push(format!("{ptr}: type differs; not applied"));
                return;
            }
            set_at(local, ptr, shared.clone());
            c.owned.values.insert(ptr.into(), shared.clone());
        }
    }
}

/// Merge `shared` into `local`. `prev`: what the app set last time for this group. `taken`:
/// pointers owned by earlier-ranked groups, with that group's name.
pub fn merge(
    local: &Value,
    shared: &Value,
    prev: &Owned,
    taken: &BTreeMap<String, String>,
) -> Result<Merged, String> {
    if !local.is_object() || !shared.is_object() {
        return Err(NOT_OBJECT.into());
    }
    let (shared, mut notes) = filter_blocked(shared);
    let mut out = local.clone();
    let mut c = Ctx {
        prev,
        taken,
        owned: Owned::default(),
        notes: vec![],
    };
    walk(&mut c, &mut out, &shared, "");
    // values the group stopped shipping go, if still as the app set them
    for (ptr, was) in &prev.values {
        if !c.owned.values.contains_key(ptr)
            && !c.owned.array_items.contains_key(ptr)
            && out.pointer(ptr) == Some(was)
        {
            remove_at(&mut out, ptr);
        }
    }
    for (ptr, was) in &prev.array_items {
        let keep = c.owned.array_items.get(ptr);
        if let Some(Value::Array(have)) = out.pointer_mut(ptr) {
            have.retain(|i| !was.contains(i) || keep.is_some_and(|k| k.contains(i)));
            if have.is_empty() && keep.is_none() {
                remove_at(&mut out, ptr);
            }
        }
    }
    notes.append(&mut c.notes);
    Ok(Merged {
        settings: out,
        owned: c.owned,
        notes,
    })
}

/// Take a group's values out (merge with an empty file).
pub fn unapply(local: &Value, prev: &Owned) -> Result<Merged, String> {
    merge(local, &Value::Object(Map::new()), prev, &BTreeMap::new())
}
