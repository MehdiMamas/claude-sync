//! What the user did to settings the group owns, and how that is kept or put back (v1
//! EngSettings): edited pointers (a value the app set that the user changed or deleted), the
//! undo that restores them, mods-canary trials and pinned-pack plugins (local-only overrides,
//! neither edits nor overwritten), and `reconstruct`: the shared copy with the user's edits,
//! what a host pushes back.
//!
//! v2-Parity rows covered: 25, 43.
use crate::settings::{remove_at, set_at, Owned};
use serde_json::Value;
use std::collections::BTreeSet;

fn join(base: &str, key: &str) -> String {
    format!("{base}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// Mods tried by a mods-canary here: `enabledPlugins["<name>@claude-mods"]` is `false` and
/// `env.CLAUDE_CODE_PLUGIN_DIRS` (split on `:` or `;`) has a path ending in `mods/<name>`.
pub fn canary_mods(local: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Some(dirs) = local
        .pointer("/env/CLAUDE_CODE_PLUGIN_DIRS")
        .and_then(Value::as_str)
    else {
        return out;
    };
    let Some(enabled) = local.get("enabledPlugins").and_then(Value::as_object) else {
        return out;
    };
    for dir in dirs.split([':', ';']) {
        let mut parts = dir.trim_end_matches(['/', '\\']).rsplit(['/', '\\']);
        let (Some(name), Some("mods")) = (parts.next(), parts.next()) else {
            continue;
        };
        if !name.is_empty()
            && enabled.get(&format!("{name}@claude-mods")) == Some(&Value::Bool(false))
        {
            out.insert(name.to_string());
        }
    }
    out
}

/// Pointers of the `enabledPlugins` entries a canary overrides.
pub fn canary_pointers(local: &Value) -> BTreeSet<String> {
    canary_mods(local)
        .into_iter()
        .map(|n| join("/enabledPlugins", &format!("{n}@claude-mods")))
        .collect()
}

/// After a merge, put the local value back at every canary and `pinned` pointer (plugins of
/// packs that are on here). True when `merged` still differs from `local`.
pub fn keep_overrides(local: &Value, merged: &mut Value, pinned: &BTreeSet<String>) -> bool {
    let mut keep = canary_pointers(local);
    keep.extend(pinned.iter().cloned());
    for ptr in keep {
        if let Some(v) = local.pointer(&ptr) {
            set_at(merged, &ptr, v.clone());
        }
    }
    *merged != *local
}

/// Owned pointers whose local value differs from what the app set: `(pointer, deleted)`.
/// An array is listed once when any owned item is missing. Canary and pinned pointers are
/// not edits.
pub fn edited_pointers(
    local: &Value,
    owned: &Owned,
    pinned: &BTreeSet<String>,
) -> Vec<(String, bool)> {
    let mut skip = canary_pointers(local);
    skip.extend(pinned.iter().cloned());
    let mut out = Vec::new();
    for (ptr, v) in owned.values.iter().filter(|(p, _)| !skip.contains(*p)) {
        match local.pointer(ptr) {
            Some(lv) if lv == v => {}
            Some(_) => out.push((ptr.clone(), false)),
            None => out.push((ptr.clone(), true)),
        }
    }
    for (ptr, items) in &owned.array_items {
        match local.pointer(ptr).and_then(Value::as_array) {
            None => out.push((ptr.clone(), true)),
            Some(a) if items.iter().any(|it| !a.contains(it)) => out.push((ptr.clone(), false)),
            Some(_) => {}
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `local` with the app's values put back for every owned pointer at or under `under`
/// (`""` = all): the settings "Undo".
pub fn restore(local: &Value, owned: &Owned, under: &str) -> Value {
    let mut out = local.clone();
    let inside = |p: &str| under.is_empty() || p == under || p.starts_with(&format!("{under}/"));
    for (ptr, v) in owned.values.iter().filter(|(p, _)| inside(p)) {
        set_at(&mut out, ptr, v.clone());
    }
    for (ptr, items) in owned.array_items.iter().filter(|(p, _)| inside(p)) {
        if out.pointer(ptr).is_none() {
            set_at(&mut out, ptr, Value::Array(vec![]));
        }
        if let Some(Value::Array(a)) = out.pointer_mut(ptr) {
            for it in items {
                if !a.contains(it) {
                    a.push(it.clone());
                }
            }
        }
    }
    out
}

/// The group's settings as this machine has them: `shared_copy` with every owned value
/// replaced by the user's edit (removed where deleted) and owned array items the user took
/// out removed. `None` when nothing differs.
pub fn reconstruct(
    shared_copy: &Value,
    local: &Value,
    owned: &Owned,
    pinned: &BTreeSet<String>,
) -> Option<Value> {
    if !shared_copy.is_object() || !local.is_object() {
        return None;
    }
    let mut r = shared_copy.clone();
    for (ptr, deleted) in edited_pointers(local, owned, pinned) {
        match (owned.values.contains_key(&ptr), local.pointer(&ptr)) {
            (true, Some(lv)) => set_at(&mut r, &ptr, lv.clone()),
            (true, None) => remove_at(&mut r, &ptr),
            (false, _) => {
                let items = local.pointer(&ptr).and_then(Value::as_array);
                if let (Some(Value::Array(a)), false) =
                    (r.pointer_mut(&ptr), deleted && items.is_none())
                {
                    a.retain(|x| {
                        owned.array_items[&ptr].iter().all(|i| i != x)
                            || items.is_some_and(|l| l.contains(x))
                    });
                } else if deleted {
                    remove_at(&mut r, &ptr);
                }
            }
        }
    }
    (r != *shared_copy).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn owned() -> Owned {
        let mut o = Owned::default();
        o.values.insert("/model".into(), json!("opus"));
        o.array_items
            .insert("/permissions/allow".into(), vec![json!("Read")]);
        o
    }

    #[test]
    fn an_edit_is_found_and_undone() {
        let local = json!({"model": "haiku", "permissions": {"allow": ["Bash"]}});
        let e = edited_pointers(&local, &owned(), &BTreeSet::new());
        assert_eq!(
            e,
            vec![
                ("/model".into(), false),
                ("/permissions/allow".into(), false)
            ]
        );
        let back = restore(&local, &owned(), "");
        assert_eq!(back["model"], "opus");
        assert_eq!(back["permissions"]["allow"], json!(["Bash", "Read"]));
        assert!(edited_pointers(&back, &owned(), &BTreeSet::new()).is_empty());
    }

    #[test]
    fn a_canary_and_a_pinned_plugin_are_not_edits_and_stay() {
        let mut o = Owned::default();
        o.values
            .insert("/enabledPlugins/m@claude-mods".into(), json!(true));
        o.values.insert("/enabledPlugins/p@x".into(), json!(true));
        let local = json!({"env": {"CLAUDE_CODE_PLUGIN_DIRS": "/a/mods/m"},
            "enabledPlugins": {"m@claude-mods": false, "p@x": false}});
        let pinned: BTreeSet<String> = ["/enabledPlugins/p@x".to_string()].into();
        assert!(edited_pointers(&local, &o, &pinned).is_empty());
        let mut merged =
            json!({"env": local["env"], "enabledPlugins": {"m@claude-mods": true, "p@x": true}});
        assert!(!keep_overrides(&local, &mut merged, &pinned));
        assert_eq!(merged, local);
    }

    #[test]
    fn reconstruct_takes_the_users_edit_into_the_copy() {
        let copy = json!({"model": "opus"});
        let local = json!({"model": "haiku"});
        let o = Owned {
            values: [("/model".to_string(), json!("opus"))].into(),
            ..Owned::default()
        };
        let r = reconstruct(&copy, &local, &o, &BTreeSet::new());
        assert_eq!(r, Some(json!({"model": "haiku"})));
        assert_eq!(reconstruct(&copy, &copy, &o, &BTreeSet::new()), None);
    }
}
