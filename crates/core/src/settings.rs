//! settings.shared.json -> ~/.claude/settings.json with ownership (§8, issue #17).
//! `merge` is pure; the apply layer reads and writes the files.
//!
//! Rules (v0's, plus ownership):
//! - Objects merge key by key. Arrays get the shared items they lack, after the local
//!   items, without duplicates. For plain values the shared value wins.
//! - A local object or array where the group has another type is left alone and that
//!   key is reported in `skipped` (v0 stopped the whole merge; the app skips one key).
//! - [`Owned`] records every plain value the app set (JSON pointer -> value) and every
//!   array item it added. A value or item the group no longer ships is removed locally
//!   only if it still equals what the app set. Objects a removal leaves empty are pruned.
//! - A value the user changed (or deleted) after the app set it is not reverted while
//!   the group's value stays the same: it stays tracked, so [`edited_pointers`] reports
//!   it and [`reconstruct`] carries it into the group's file for sharing. When the group
//!   ships a new value, the group wins (the three-way merge upstream already saw both).
//! - A value the user already had, equal to the group's, is not owned: removing the
//!   group never deletes it.
//! - Never applied (reported in `skipped`): `apiKeyHelper`, `env` values the secret
//!   scan flags, `hooks` entries whose command contains an absolute path.
use crate::Error;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Owned {
    /// JSON pointer -> value the app set.
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
    /// JSON pointer of an array -> items the app added.
    #[serde(default)]
    pub array_items: BTreeMap<String, Vec<Value>>,
}

impl Owned {
    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.array_items.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    pub settings: Value,
    pub owned: Owned,
    /// Shared keys not applied, with the reason (§8 "never shared").
    pub skipped: Vec<String>,
    pub changed: bool,
}

/// `local` and `shared` must be JSON objects. `prev` is what this group owned after the last merge.
pub fn merge(local: &Value, shared: &Value, prev: &Owned) -> crate::Result<Merged> {
    if !local.is_object() {
        return Err(Error::InvalidJson {
            path: "settings.json".into(),
        });
    }
    if !shared.is_object() {
        return Err(Error::InvalidJson {
            path: "settings.shared.json".into(),
        });
    }
    let mut skipped = Vec::new();
    let shared = filter_blocked(shared, &mut skipped);
    let mut out = local.clone();
    let mut owned = Owned::default();
    if let (Value::Object(om), Value::Object(sm)) = (&mut out, &shared) {
        merge_object(om, sm, "", prev, &mut owned, &mut skipped);
    }
    remove_dropped(&mut out, &shared, prev, &mut owned);
    let changed = out != *local;
    Ok(Merged {
        settings: out,
        owned,
        skipped,
        changed,
    })
}

/// Parse a settings file: one JSON object (a UTF-8 BOM is allowed). `None` otherwise.
pub fn parse_object(bytes: &[u8]) -> Option<Value> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v @ Value::Object(_)) => Some(v),
        _ => None,
    }
}

/// How the app writes settings.json: two-space indent, LF, trailing newline.
pub fn to_bytes(v: &Value) -> Vec<u8> {
    let mut s = serde_json::to_string_pretty(v).expect("a Value always serializes");
    s.push('\n');
    s.into_bytes()
}

/// `shared` without the given pointers. Each pointer that was present is reported as
/// `"<key path>: <reason>"`. The apply layer uses it for keys an earlier group owns
/// and for pointers the user chose to keep local.
pub fn without(shared: &Value, ptrs: &[(String, String)]) -> (Value, Vec<String>) {
    let mut out = shared.clone();
    let mut skipped = Vec::new();
    for (ptr, reason) in ptrs {
        if !ptr.is_empty() && out.pointer(ptr).is_some() {
            remove_at(&mut out, ptr);
            skipped.push(format!("{}: {reason}", display(ptr)));
        }
    }
    (out, skipped)
}

/// The group's settings as this machine has them now: `shared_copy` (the file the app
/// last applied) with every owned value replaced by the local one where the user changed
/// it, removed where the user deleted it, and owned array items the user removed taken
/// out. Everything the app does not own (skipped keys, values the user already had,
/// keys another group set) stays as in `shared_copy`, so it is never pushed back
/// changed. New local array items are not included: they may be personal.
/// `None` when nothing differs (the caller then returns the copy's exact bytes).
pub fn reconstruct(shared_copy: &Value, local: &Value, owned: &Owned) -> Option<Value> {
    if !shared_copy.is_object() || !local.is_object() {
        return None;
    }
    let mut r = shared_copy.clone();
    for (ptr, v) in &owned.values {
        match local.pointer(ptr) {
            Some(lv) if lv == v => {}
            Some(lv) => {
                set_at(&mut r, ptr, lv.clone());
            }
            None => {
                remove_at(&mut r, ptr);
            }
        }
    }
    for (ptr, items) in &owned.array_items {
        let local_items = local.pointer(ptr).and_then(Value::as_array);
        for it in items {
            if local_items.is_some_and(|a| a.contains(it)) {
                continue;
            }
            if let Some(Value::Array(a)) = r.pointer_mut(ptr) {
                a.retain(|x| x != it);
            }
        }
    }
    (r != *shared_copy).then_some(r)
}

/// Owned pointers whose local value differs from what the app set:
/// `(pointer, deleted)`. An array is listed once when any owned item is missing.
pub fn edited_pointers(local: &Value, owned: &Owned) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for (ptr, v) in &owned.values {
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
/// (`""` = all): the "Undo" action for settings.
pub fn restore(local: &Value, owned: &Owned, under: &str) -> Value {
    let mut out = local.clone();
    let inside = |p: &str| under.is_empty() || p == under || p.starts_with(&format!("{under}/"));
    for (ptr, v) in owned.values.iter().filter(|(p, _)| inside(p)) {
        set_at(&mut out, ptr, v.clone());
    }
    for (ptr, items) in owned.array_items.iter().filter(|(p, _)| inside(p)) {
        if out.pointer(ptr).is_none() {
            set_at(&mut out, ptr, Value::Array(Vec::new()));
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

/// Remove the subtree at `ptr` from what `owned` tracks (the "Keep local" action).
pub fn forget(owned: &mut Owned, under: &str) {
    let inside = |p: &str| under.is_empty() || p == under || p.starts_with(&format!("{under}/"));
    owned.values.retain(|p, _| !inside(p));
    owned.array_items.retain(|p, _| !inside(p));
}

/// `/permissions/allow` -> `permissions.allow` (for messages; never shows values).
pub fn display(ptr: &str) -> String {
    if ptr.is_empty() {
        return "(top level)".into();
    }
    tokens(ptr).collect::<Vec<_>>().join(".")
}

/// Append one key to a JSON pointer (RFC 6901 escaping).
pub fn join(base: &str, key: &str) -> String {
    format!("{base}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn tokens(ptr: &str) -> impl Iterator<Item = String> + '_ {
    ptr.split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "true/false",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Plain values and empty containers are owned as one value.
fn is_leaf(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => true,
    }
}

fn merge_object(
    out: &mut Map<String, Value>,
    shared: &Map<String, Value>,
    base: &str,
    prev: &Owned,
    owned: &mut Owned,
    skipped: &mut Vec<String>,
) {
    for (k, sv) in shared {
        let p = join(base, k);
        let Some(lv) = out.get_mut(k) else {
            if let Some(v) = fresh(sv, &p, prev, owned, skipped) {
                out.insert(k.clone(), v);
            }
            continue;
        };
        match (&mut *lv, sv) {
            (Value::Object(lm), Value::Object(sm)) => {
                merge_object(lm, sm, &p, prev, owned, skipped)
            }
            (Value::Array(la), Value::Array(sa)) => merge_array(la, sa, &p, prev, owned),
            (Value::Object(_) | Value::Array(_), _) => skipped.push(format!(
                "{}: the group's value is {} {} and settings.json has {} {}; not applied",
                display(&p),
                article(sv),
                type_name(sv),
                article(lv),
                type_name(lv),
            )),
            (_, _) if !is_leaf(sv) => {
                // A local plain value replaced by a group object or array: shared wins.
                if let Some(v) = fresh(sv, &p, prev, owned, skipped) {
                    *lv = v;
                }
            }
            (_, _) => {
                if *lv == *sv {
                    if prev.values.contains_key(&p) {
                        owned.values.insert(p, sv.clone());
                    }
                } else if prev.values.get(&p) == Some(sv) {
                    // The user changed it and the group did not: keep theirs, keep tracking.
                    owned.values.insert(p, sv.clone());
                } else {
                    *lv = sv.clone();
                    owned.values.insert(p, sv.clone());
                }
            }
        }
    }
}

fn article(v: &Value) -> &'static str {
    match v {
        Value::Array(_) | Value::Object(_) => "an",
        Value::Bool(_) | Value::Null => "",
        _ => "a",
    }
}

/// The value to insert for a key the local side does not have (`None` = insert nothing,
/// for example because the user deleted every part the app had set).
fn fresh(
    sv: &Value,
    p: &str,
    prev: &Owned,
    owned: &mut Owned,
    skipped: &mut Vec<String>,
) -> Option<Value> {
    match sv {
        Value::Object(sm) if !sm.is_empty() => {
            let mut m = Map::new();
            merge_object(&mut m, sm, p, prev, owned, skipped);
            (!m.is_empty()).then_some(Value::Object(m))
        }
        Value::Array(sa) if !sa.is_empty() => {
            let mut a = Vec::new();
            merge_array(&mut a, sa, p, prev, owned);
            (!a.is_empty()).then_some(Value::Array(a))
        }
        _ => {
            owned.values.insert(p.to_string(), sv.clone());
            // Set before and missing now: the user deleted it; do not put it back.
            (prev.values.get(p) != Some(sv)).then(|| sv.clone())
        }
    }
}

fn merge_array(la: &mut Vec<Value>, sa: &[Value], p: &str, prev: &Owned, owned: &mut Owned) {
    let prev_items = prev.array_items.get(p);
    let mut mine: Vec<Value> = Vec::new();
    for it in sa {
        if mine.contains(it) {
            continue;
        }
        let was_mine = prev_items.is_some_and(|pi| pi.contains(it));
        if la.contains(it) {
            if was_mine {
                mine.push(it.clone());
            }
        } else if was_mine {
            // Added before, removed by the user since: keep it out, keep tracking.
            mine.push(it.clone());
        } else {
            la.push(it.clone());
            mine.push(it.clone());
        }
    }
    if !mine.is_empty() {
        owned.array_items.insert(p.to_string(), mine);
    }
}

/// Remove what the group no longer ships, where the local side still has what the app set.
fn remove_dropped(out: &mut Value, shared: &Value, prev: &Owned, owned: &mut Owned) {
    for (p, v) in &prev.values {
        if owned.values.contains_key(p) || shared.pointer(p).is_some() {
            continue;
        }
        match out.pointer(p) {
            Some(cur) if cur == v => remove_at(out, p),
            // Changed since the app set it: it stays, still tracked, so it can be shared.
            Some(_) => {
                owned.values.insert(p.clone(), v.clone());
            }
            None => {}
        }
    }
    for (p, items) in &prev.array_items {
        let still = shared.pointer(p).and_then(Value::as_array);
        let mut removed = false;
        if let Some(Value::Array(a)) = out.pointer_mut(p) {
            for it in items {
                if still.is_some_and(|s| s.contains(it)) {
                    continue;
                }
                if let Some(i) = a.iter().position(|x| x == it) {
                    a.remove(i);
                    removed = true;
                }
            }
        }
        if removed
            && out
                .pointer(p)
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        {
            remove_at(out, p);
        }
    }
}

/// Remove the key at `ptr`, then prune objects that the removal left empty.
pub(crate) fn remove_at(root: &mut Value, ptr: &str) {
    let toks: Vec<String> = tokens(ptr).collect();
    for depth in (1..=toks.len()).rev() {
        let parent_ptr: String = toks[..depth - 1]
            .iter()
            .fold(String::new(), |acc, t| join(&acc, t));
        let Some(Value::Object(parent)) = root.pointer_mut(&parent_ptr) else {
            return;
        };
        let key = &toks[depth - 1];
        let remove = depth == toks.len()
            || parent
                .get(key)
                .is_some_and(|v| v.as_object().is_some_and(Map::is_empty));
        if !remove {
            return;
        }
        parent.shift_remove(key);
    }
}

/// Set `ptr` to `v`, creating objects on the way. False when a parent is not an object.
pub(crate) fn set_at(root: &mut Value, ptr: &str, v: Value) -> bool {
    let toks: Vec<String> = tokens(ptr).collect();
    let Some((last, parents)) = toks.split_last() else {
        return false;
    };
    let mut cur = root;
    for t in parents {
        let Value::Object(m) = cur else { return false };
        cur = m
            .entry(t.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let Value::Object(m) = cur else { return false };
    m.insert(last.clone(), v);
    true
}

static ABSOLUTE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    // A drive path (`C:\`, `C:/`), a UNC path, or a `/`-rooted path with at least two
    // segments (`/usr/bin/x`; a lone `/c` as in `cmd /c` is a flag, not a path).
    Regex::new(r#"(?:^|[\s"'=(;,`])(?:[A-Za-z]:[\\/]|\\\\[^\\\s]|/[A-Za-z0-9._@+-]+/)"#)
        .expect("abs path regex")
});

/// True when a hook command refers to a machine path.
pub fn has_absolute_path(command: &str) -> bool {
    ABSOLUTE_PATH.is_match(command)
}

fn commands_in(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                match (k.as_str(), x) {
                    ("command", Value::String(s)) => out.push(s.clone()),
                    _ => commands_in(x, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| commands_in(x, out)),
        _ => {}
    }
}

/// `shared` without the keys §8 never applies, each reported in `skipped`.
fn filter_blocked(shared: &Value, skipped: &mut Vec<String>) -> Value {
    let mut s = shared.clone();
    let Value::Object(m) = &mut s else { return s };
    if m.shift_remove("apiKeyHelper").is_some() {
        skipped.push("apiKeyHelper: never shared; not applied".into());
    }
    if let Some(Value::Object(env)) = m.get_mut("env") {
        env.retain(|k, v| {
            let text =
                serde_json::to_string(&serde_json::json!({ k.as_str(): v })).unwrap_or_default();
            let hit = match v {
                Value::String(s) => crate::secrets::first_hit(s),
                _ => None,
            }
            .or_else(|| crate::secrets::first_hit(&text));
            if let Some(pattern) = hit {
                skipped.push(format!(
                    "env.{k}: looks like a secret ({pattern}); not applied"
                ));
            }
            hit.is_none()
        });
    }
    if let Some(Value::Object(hooks)) = m.get_mut("hooks") {
        for (event, entries) in hooks.iter_mut() {
            let Value::Array(list) = entries else {
                continue;
            };
            let mut i = 0;
            list.retain(|entry| {
                let mut cmds = Vec::new();
                commands_in(entry, &mut cmds);
                let keep = !cmds.iter().any(|c| has_absolute_path(c));
                if !keep {
                    skipped.push(format!(
                        "hooks.{event}[{i}]: command uses an absolute path; not applied"
                    ));
                }
                i += 1;
                keep
            });
        }
        hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
        if hooks.is_empty() {
            m.shift_remove("hooks");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(local: Value, shared: Value, prev: &Owned) -> Merged {
        merge(&local, &shared, prev).expect("merge")
    }

    // ---- cases carried over from the v0 CLI ----

    #[test]
    fn v0_merge_cases() {
        let local = json!({
            "model": "local-model",
            "localOnly": {"keep": true, "path": "C:\\Users\\me"},
            "permissions": {"allow": ["Bash(ls)", "Read"], "deny": ["WebFetch"]},
            "env": {"A": "1"},
            "theme": "dark"
        });
        let shared = json!({
            "model": "shared-model",
            "permissions": {"allow": ["Read", "Bash(git status)", "Read"]},
            "env": {"B": "2"},
            "newKey": "v",
            "nested": {"deep": {"x": 1}},
            "note": "caf\u{e9}"
        });
        let r = m(local.clone(), shared, &Owned::default());
        let s = &r.settings;
        assert!(r.changed);
        assert!(r.skipped.is_empty(), "{:?}", r.skipped);
        assert_eq!(s["newKey"], json!("v"), "new key");
        assert_eq!(s["nested"], json!({"deep": {"x": 1}}), "new nested object");
        assert_eq!(
            s["env"],
            json!({"A": "1", "B": "2"}),
            "nested objects merge"
        );
        assert_eq!(
            s["permissions"]["allow"],
            json!(["Bash(ls)", "Read", "Bash(git status)"]),
            "array union"
        );
        assert_eq!(
            s["permissions"]["deny"],
            json!(["WebFetch"]),
            "local nested key kept"
        );
        assert_eq!(
            s["model"],
            json!("shared-model"),
            "shared wins a scalar conflict"
        );
        assert_eq!(s["localOnly"], local["localOnly"], "local-only key kept");
        assert_eq!(s["theme"], json!("dark"));
        assert_eq!(
            s["note"]
                .as_str()
                .unwrap()
                .chars()
                .map(|c| c as u32)
                .collect::<Vec<_>>(),
            [99, 97, 102, 233]
        );
        let keys: Vec<_> = s.as_object().unwrap().keys().take(5).cloned().collect();
        assert_eq!(
            keys,
            ["model", "localOnly", "permissions", "env", "theme"],
            "local key order kept"
        );
        let bytes = to_bytes(s);
        assert!(!bytes.contains(&b'\r'), "written with LF");
        assert!(
            String::from_utf8(bytes).unwrap().contains("caf\u{e9}"),
            "UTF-8 kept, not escaped"
        );

        // Ownership: only what the app set or added. "Read" was already there.
        assert_eq!(r.owned.values.get("/model"), Some(&json!("shared-model")));
        assert_eq!(r.owned.values.get("/nested/deep/x"), Some(&json!(1)));
        assert_eq!(r.owned.values.get("/env/B"), Some(&json!("2")));
        assert_eq!(
            r.owned.array_items.get("/permissions/allow"),
            Some(&vec![json!("Bash(git status)")])
        );

        // Nothing new: no change.
        let again = m(
            r.settings.clone(),
            json!({
                "model": "shared-model",
                "permissions": {"allow": ["Read", "Bash(git status)", "Read"]},
                "env": {"B": "2"}, "newKey": "v", "nested": {"deep": {"x": 1}}, "note": "caf\u{e9}"
            }),
            &r.owned,
        );
        assert!(!again.changed);
        assert_eq!(again.settings, r.settings);
        assert_eq!(again.owned, r.owned);
    }

    #[test]
    fn v0_created_from_shared_when_local_is_empty() {
        let r = m(json!({}), json!({"model": "hook-model"}), &Owned::default());
        assert_eq!(r.settings, json!({"model": "hook-model"}));
        assert!(r.changed);
    }

    #[test]
    fn v0_hook_pull_keeps_earlier_keys() {
        // A later shared file without env: env.B is removed only because the app set it.
        let first = m(
            json!({"env": {"A": "1"}}),
            json!({"env": {"B": "2"}, "model": "m"}),
            &Owned::default(),
        );
        let second = m(
            first.settings.clone(),
            json!({"model": "hook-model"}),
            &first.owned,
        );
        assert_eq!(
            second.settings,
            json!({"env": {"A": "1"}, "model": "hook-model"})
        );
    }

    // ---- cases carried over from the v0 CLI ----

    #[test]
    fn v0_invalid_json_is_an_error() {
        assert!(
            matches!(merge(&json!(["model"]), &json!({}), &Owned::default()),
            Err(Error::InvalidJson { path }) if path == "settings.json")
        );
        assert!(
            matches!(merge(&json!({}), &json!("model"), &Owned::default()),
            Err(Error::InvalidJson { path }) if path == "settings.shared.json")
        );
        for bad in [
            &b"{\"model\": \"local\",\r\n"[..],
            b"{\"a\": 1}\n{\"b\": 2}\n",
            b"[\"model\"]\n",
            b"",
            b"\"model\"\n",
        ] {
            assert!(
                parse_object(bad).is_none(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
        assert_eq!(
            parse_object(b"\xEF\xBB\xBF{\"a\":1}\r\n"),
            Some(json!({"a": 1}))
        );
    }

    #[test]
    fn v0_type_clash_skips_only_that_key() {
        let local = json!({"permissions": {"allow": ["Read"]}, "model": "local"});
        let r = m(
            local.clone(),
            json!({"permissions": "Read", "model": "shared"}),
            &Owned::default(),
        );
        assert_eq!(r.settings["permissions"], local["permissions"]);
        assert_eq!(
            r.settings["model"],
            json!("shared"),
            "other keys still merge"
        );
        assert_eq!(r.skipped.len(), 1);
        assert!(r.skipped[0].starts_with("permissions:"), "{:?}", r.skipped);
        assert!(!r.skipped[0].contains("Read"), "never shows values");

        let r = m(
            local.clone(),
            json!({"permissions": {"allow": {"Read": true}}}),
            &Owned::default(),
        );
        assert!(!r.changed);
        assert!(
            r.skipped[0].starts_with("permissions.allow:"),
            "{:?}",
            r.skipped
        );
    }

    #[test]
    fn v0_object_over_local_scalar_wins() {
        let r = m(
            json!({"model": "local", "permissions": {"allow": ["Read"]}}),
            json!({"model": {"name": "m"}}),
            &Owned::default(),
        );
        assert_eq!(
            r.settings,
            json!({"model": {"name": "m"}, "permissions": {"allow": ["Read"]}})
        );
        assert!(r.skipped.is_empty());
    }

    // ---- cases carried over from the v0 CLI ----

    #[test]
    fn v0_ten_merges_keep_local_only_keys() {
        let mut local = json!({"localOnly": {"machine": "A", "list": [1, 2]}, "permissions": {"allow": ["Local"]}});
        let mut owned = Owned::default();
        for i in 1..=10 {
            let shared = json!({"model": format!("m{i}"), "permissions": {"allow": [format!("S{i}")]}, "env": {format!("K{i}"): format!("{i}")}});
            let r = m(local, shared, &owned);
            assert!(r.changed);
            local = r.settings;
            owned = r.owned;
            assert_eq!(local["localOnly"], json!({"machine": "A", "list": [1, 2]}));
            assert_eq!(local["model"], json!(format!("m{i}")));
            assert_eq!(local["permissions"]["allow"][0], json!("Local"));
            assert!(local["permissions"]["allow"]
                .as_array()
                .unwrap()
                .contains(&json!(format!("S{i}"))));
        }
        // With ownership, items and env keys the group dropped are gone (v0 kept them all).
        assert_eq!(local["permissions"]["allow"], json!(["Local", "S10"]));
        assert_eq!(local["env"], json!({"K10": "10"}));
    }

    // ---- ownership (#17) ----

    #[test]
    fn group_removes_a_key() {
        let a = m(
            json!({"theme": "dark"}),
            json!({"model": "x", "env": {"A": "1"}}),
            &Owned::default(),
        );
        let b = m(a.settings.clone(), json!({"model": "x"}), &a.owned);
        assert_eq!(
            b.settings,
            json!({"theme": "dark", "model": "x"}),
            "env.A removed and empty env pruned"
        );
        assert!(b.changed);
        assert!(!b.owned.values.contains_key("/env/A"));
        let c = m(b.settings.clone(), json!({}), &b.owned);
        assert_eq!(c.settings, json!({"theme": "dark"}));
        assert!(c.owned.is_empty());
    }

    #[test]
    fn group_removes_an_array_item() {
        let a = m(
            json!({"permissions": {"allow": ["Mine"]}}),
            json!({"permissions": {"allow": ["A", "B", "Mine"]}}),
            &Owned::default(),
        );
        assert_eq!(
            a.settings["permissions"]["allow"],
            json!(["Mine", "A", "B"])
        );
        assert_eq!(
            a.owned.array_items["/permissions/allow"],
            vec![json!("A"), json!("B")]
        );
        let b = m(
            a.settings.clone(),
            json!({"permissions": {"allow": ["B"]}}),
            &a.owned,
        );
        assert_eq!(
            b.settings["permissions"]["allow"],
            json!(["Mine", "B"]),
            "user's own item stays"
        );
        let c = m(b.settings.clone(), json!({}), &b.owned);
        assert_eq!(c.settings, json!({"permissions": {"allow": ["Mine"]}}));
        // An array the app created entirely is removed with its last item.
        let d = m(
            json!({}),
            json!({"permissions": {"deny": ["X"]}}),
            &Owned::default(),
        );
        let e = m(d.settings, json!({}), &d.owned);
        assert_eq!(e.settings, json!({}));
    }

    #[test]
    fn user_changed_a_group_value_it_is_kept() {
        let a = m(
            json!({}),
            json!({"model": "x", "env": {"A": "1"}}),
            &Owned::default(),
        );
        let mut edited = a.settings.clone();
        edited["model"] = json!("mine");
        // Group unchanged: the user's value is not reverted, and stays tracked.
        let b = m(
            edited.clone(),
            json!({"model": "x", "env": {"A": "1"}}),
            &a.owned,
        );
        assert_eq!(b.settings["model"], json!("mine"));
        assert!(!b.changed);
        assert_eq!(
            edited_pointers(&b.settings, &b.owned),
            vec![("/model".to_string(), false)]
        );
        // Group drops it: it stays because the user changed it.
        let c = m(edited.clone(), json!({"env": {"A": "1"}}), &b.owned);
        assert_eq!(c.settings["model"], json!("mine"));
        assert!(!c.changed);
        assert_eq!(
            c.owned.values.get("/model"),
            Some(&json!("x")),
            "still tracked, shows as a local edit"
        );
        // Group ships a new value: the group wins.
        let d = m(edited, json!({"model": "y", "env": {"A": "1"}}), &b.owned);
        assert_eq!(d.settings["model"], json!("y"));
    }

    #[test]
    fn user_deleted_a_group_value_it_is_not_put_back() {
        let a = m(
            json!({}),
            json!({"model": "x", "permissions": {"allow": ["A", "B"]}}),
            &Owned::default(),
        );
        let local = json!({"permissions": {"allow": ["B"]}});
        let b = m(
            local.clone(),
            json!({"model": "x", "permissions": {"allow": ["A", "B"]}}),
            &a.owned,
        );
        assert_eq!(b.settings, local);
        assert!(!b.changed);
        assert_eq!(
            edited_pointers(&b.settings, &b.owned),
            vec![
                ("/model".to_string(), true),
                ("/permissions/allow".to_string(), false)
            ]
        );
        // Undo puts the group's values back.
        let undone = restore(&b.settings, &b.owned, "");
        assert_eq!(
            undone,
            json!({"permissions": {"allow": ["B", "A"]}, "model": "x"})
        );
        assert!(edited_pointers(&undone, &b.owned).is_empty());
    }

    #[test]
    fn value_the_user_already_had_is_not_owned() {
        let a = m(
            json!({"model": "x"}),
            json!({"model": "x"}),
            &Owned::default(),
        );
        assert!(!a.changed);
        assert!(a.owned.is_empty());
        let b = m(a.settings, json!({}), &a.owned);
        assert_eq!(
            b.settings,
            json!({"model": "x"}),
            "removing the group keeps the user's value"
        );
    }

    #[test]
    fn blocked_keys_are_reported_and_not_applied() {
        let ghp = format!("ghp_{}", "a".repeat(36));
        let shared = json!({
            "apiKeyHelper": "/usr/local/bin/key.sh",
            "env": {"SAFE": "1", "TOKEN": ghp, "access_token": "x"},
            "hooks": {
                "PreToolUse": [
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": "~/.claude/hooks/check.sh"}]},
                    {"matcher": "Edit", "hooks": [{"type": "command", "command": "C:\\Users\\me\\hook.ps1"}]},
                    {"matcher": "Read", "hooks": [{"type": "command", "command": "node /Users/me/hook.js"}]}
                ],
                "Stop": [{"hooks": [{"type": "command", "command": "bash \"D:/tools/stop.sh\""}]}]
            }
        });
        let r = m(json!({}), shared, &Owned::default());
        assert!(r.settings.get("apiKeyHelper").is_none());
        assert_eq!(r.settings["env"], json!({"SAFE": "1"}));
        assert_eq!(
            r.settings["hooks"]["PreToolUse"].as_array().unwrap().len(),
            1
        );
        assert!(
            r.settings["hooks"].get("Stop").is_none(),
            "an event left empty is dropped"
        );
        let joined = r.skipped.join("\n");
        for want in [
            "apiKeyHelper:",
            "env.TOKEN: looks like a secret (ghp)",
            "env.access_token:",
            "hooks.PreToolUse[1]:",
            "hooks.PreToolUse[2]:",
            "hooks.Stop[0]:",
        ] {
            assert!(joined.contains(want), "missing {want} in {joined}");
        }
        assert!(
            !joined.contains(&ghp) && !joined.contains("Users"),
            "no values in messages"
        );
        assert!(!r
            .owned
            .values
            .keys()
            .any(|k| k.contains("TOKEN") || k.contains("apiKeyHelper")));
    }

    #[test]
    fn absolute_path_detection() {
        for abs in [
            "C:\\x\\y.ps1",
            "powershell -File c:/x.ps1",
            "/usr/bin/env python3 x.py",
            "\\\\server\\share\\x",
            "sh '/opt/x/y'",
            "x=/home/me/y",
        ] {
            assert!(has_absolute_path(abs), "{abs}");
        }
        for rel in [
            "~/.claude/hooks/x.sh",
            "$HOME/bin/x",
            "./scripts/x.sh",
            "cmd /c echo hi",
            "npx prettier --write",
            "curl https://example.com/a/b",
            "node scripts/x.js",
        ] {
            assert!(!has_absolute_path(rel), "{rel}");
        }
    }

    #[test]
    fn without_reports_present_pointers() {
        let (s, skipped) = without(
            &json!({"model": "x", "env": {"A": "1"}}),
            &[
                ("/env/A".into(), "set by group o/r".into()),
                ("/nope".into(), "x".into()),
            ],
        );
        assert_eq!(s, json!({"model": "x"}));
        assert_eq!(skipped, vec!["env.A: set by group o/r".to_string()]);
    }

    #[test]
    fn reconstruct_reflects_local_edits_to_owned_values_only() {
        let copy = json!({"model": "x", "apiKeyHelper": "k", "permissions": {"allow": ["A", "Mine"]}, "theme": "t"});
        let a = m(
            json!({"permissions": {"allow": ["Mine"]}, "theme": "t"}),
            copy.clone(),
            &Owned::default(),
        );
        assert!(
            reconstruct(&copy, &a.settings, &a.owned).is_none(),
            "nothing changed"
        );
        let mut local = a.settings.clone();
        local["model"] = json!("y");
        local["theme"] = json!("mine"); // not owned: the user had it already
        local["permissions"]["allow"] = json!(["Mine", "New"]); // removed A, added New
        local["extra"] = json!(1); // not the group's
        let r = reconstruct(&copy, &local, &a.owned).expect("changed");
        assert_eq!(
            r,
            json!({"model": "y", "apiKeyHelper": "k", "permissions": {"allow": ["Mine"]}, "theme": "t"})
        );
        // Deleted locally -> gone from the reconstruction.
        let mut del = a.settings.clone();
        del.as_object_mut().unwrap().shift_remove("model");
        assert!(reconstruct(&copy, &del, &a.owned)
            .unwrap()
            .get("model")
            .is_none());
    }

    #[test]
    fn forget_and_pointer_helpers() {
        let mut o = Owned::default();
        o.values.insert("/env/A".into(), json!(1));
        o.values.insert("/envX".into(), json!(1));
        o.array_items.insert("/env/L".into(), vec![json!(1)]);
        forget(&mut o, "/env");
        assert_eq!(o.values.keys().collect::<Vec<_>>(), ["/envX"]);
        assert!(o.array_items.is_empty());
        assert_eq!(join("", "a/b~c"), "/a~1b~0c");
        assert_eq!(display("/a~1b~0c/d"), "a/b~c.d");
        let r = m(json!({}), json!({"a/b": {"c~d": 1}}), &Owned::default());
        assert_eq!(r.owned.values.keys().collect::<Vec<_>>(), ["/a~1b/c~0d"]);
        let gone = m(r.settings, json!({}), &r.owned);
        assert_eq!(gone.settings, json!({}));
    }
}
