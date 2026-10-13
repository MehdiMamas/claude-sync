//! Three-way merge (§6, issue #15). Pure functions, no I/O.
use crate::FileSet;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub path: String,
    /// The local version that lost (remote wins). `None` = local had deleted it.
    pub local: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeResult {
    /// What the local side should become.
    pub merged: FileSet,
    /// What to commit to the remote (`None` = delete). Empty = nothing to push.
    pub to_push: BTreeMap<String, Option<Vec<u8>>>,
    pub conflicts: Vec<Conflict>,
}

/// Files merged key by key when both sides changed them.
pub fn is_json_object_file(path: &str) -> bool {
    path == crate::group::FILE || path == crate::group::SHARED_SETTINGS
}

pub fn three_way(base: &FileSet, local: &FileSet, remote: &FileSet) -> MergeResult {
    let mut out = MergeResult::default();
    let paths: BTreeSet<&String> = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .collect();
    for path in paths {
        let (b, l, r) = (base.get(path), local.get(path), remote.get(path));
        let take = if l == r || l == b {
            r
        } else if r == b {
            out.to_push.insert(path.clone(), l.cloned());
            l
        } else if let Some(json) = json_merge(path, b, l, r) {
            if json.conflicted {
                out.conflicts.push(Conflict {
                    path: path.clone(),
                    local: l.cloned(),
                });
            }
            if json.bytes.as_ref() != r {
                out.to_push.insert(path.clone(), json.bytes.clone());
            }
            if let Some(bytes) = json.bytes {
                out.merged.insert(path.clone(), bytes);
            }
            continue;
        } else {
            out.conflicts.push(Conflict {
                path: path.clone(),
                local: l.cloned(),
            });
            r
        };
        if let Some(bytes) = take {
            out.merged.insert(path.clone(), bytes.clone());
        }
    }
    out
}

struct JsonMerge {
    /// `None` only if both sides are missing, which `three_way` never asks for.
    bytes: Option<Vec<u8>>,
    /// Some key changed differently on both sides; remote won it.
    conflicted: bool,
}

/// Key-level merge when base (or missing), local and remote are all JSON objects.
fn json_merge(
    path: &str,
    b: Option<&Vec<u8>>,
    l: Option<&Vec<u8>>,
    r: Option<&Vec<u8>>,
) -> Option<JsonMerge> {
    if !is_json_object_file(path) {
        return None;
    }
    let object = |bytes: &Vec<u8>| match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    };
    let base = match b {
        Some(bytes) => object(bytes)?,
        None => Map::new(),
    };
    let (local, remote) = (object(l?)?, object(r?)?);
    let mut conflicted = false;
    let merged = merge_objects(&base, &local, &remote, &mut conflicted);
    // Same content as remote (maybe formatted differently): keep remote's bytes, push nothing.
    let bytes = if merged == remote {
        r.cloned()
    } else {
        let mut v = serde_json::to_vec_pretty(&Value::Object(merged)).expect("json serializes");
        v.push(b'\n');
        Some(v)
    };
    Some(JsonMerge { bytes, conflicted })
}

fn merge_objects(
    base: &Map<String, Value>,
    local: &Map<String, Value>,
    remote: &Map<String, Value>,
    conflicted: &mut bool,
) -> Map<String, Value> {
    let mut out = Map::new();
    // Remote's key order first, then keys only local has.
    let keys = remote
        .keys()
        .chain(local.keys().filter(|k| !remote.contains_key(*k)));
    let keys: Vec<&String> = keys.chain(base.keys()).collect();
    for key in keys {
        if out.contains_key(key) {
            continue;
        }
        let (b, l, r) = (base.get(key), local.get(key), remote.get(key));
        let value = if l == r || l == b {
            r.cloned()
        } else if r == b {
            l.cloned()
        } else {
            match (b, l, r) {
                (
                    None | Some(Value::Object(_)),
                    Some(Value::Object(lo)),
                    Some(Value::Object(ro)),
                ) => {
                    let empty = Map::new();
                    let bo = match b {
                        Some(Value::Object(bo)) => bo,
                        _ => &empty,
                    };
                    Some(Value::Object(merge_objects(bo, lo, ro, conflicted)))
                }
                _ => {
                    *conflicted = true;
                    r.cloned()
                }
            }
        };
        if let Some(v) = value {
            out.insert(key.clone(), v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[(&str, &str)]) -> FileSet {
        items
            .iter()
            .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
            .collect()
    }
    fn json(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn fast_forward_takes_remote() {
        let base = set(&[("agents/a.md", "1")]);
        let remote = set(&[("agents/a.md", "2"), ("agents/b.md", "new")]);
        let m = three_way(&base, &base, &remote);
        assert_eq!(m.merged, remote);
        assert!(m.to_push.is_empty() && m.conflicts.is_empty());
    }

    #[test]
    fn local_only_change_is_pushed() {
        let base = set(&[("agents/a.md", "1"), ("agents/gone.md", "x")]);
        let local = set(&[("agents/a.md", "2"), ("agents/b.md", "new")]);
        let m = three_way(&base, &local, &base);
        assert_eq!(m.merged, local);
        assert_eq!(
            m.to_push,
            BTreeMap::from([
                ("agents/a.md".to_string(), Some(b"2".to_vec())),
                ("agents/b.md".to_string(), Some(b"new".to_vec())),
                ("agents/gone.md".to_string(), None),
            ])
        );
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn both_changed_different_files() {
        let base = set(&[("agents/a.md", "1"), ("agents/b.md", "1")]);
        let local = set(&[("agents/a.md", "L"), ("agents/b.md", "1")]);
        let remote = set(&[("agents/a.md", "1"), ("agents/b.md", "R")]);
        let m = three_way(&base, &local, &remote);
        assert_eq!(m.merged, set(&[("agents/a.md", "L"), ("agents/b.md", "R")]));
        assert_eq!(
            m.to_push,
            BTreeMap::from([("agents/a.md".to_string(), Some(b"L".to_vec()))])
        );
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn same_change_on_both_sides_is_nothing() {
        let base = set(&[("agents/a.md", "1")]);
        let both = set(&[("agents/a.md", "2")]);
        let m = three_way(&base, &both, &both);
        assert_eq!(m.merged, both);
        assert!(m.to_push.is_empty() && m.conflicts.is_empty());
    }

    #[test]
    fn both_changed_one_file_is_a_conflict_remote_wins() {
        let base = set(&[("CLAUDE.md", "base")]);
        let local = set(&[("CLAUDE.md", "local")]);
        let remote = set(&[("CLAUDE.md", "remote")]);
        let m = three_way(&base, &local, &remote);
        assert_eq!(m.merged, remote);
        assert!(m.to_push.is_empty());
        assert_eq!(
            m.conflicts,
            [Conflict {
                path: "CLAUDE.md".into(),
                local: Some(b"local".to_vec())
            }]
        );
    }

    #[test]
    fn both_added_same_path_differently_is_a_conflict() {
        let m = three_way(
            &FileSet::new(),
            &set(&[("agents/a.md", "L")]),
            &set(&[("agents/a.md", "R")]),
        );
        assert_eq!(m.merged, set(&[("agents/a.md", "R")]));
        assert_eq!(m.conflicts.len(), 1);
    }

    #[test]
    fn delete_vs_edit() {
        let base = set(&[("agents/a.md", "1"), ("agents/b.md", "1")]);
        // Local deleted a, remote edited a: remote wins, conflict with no local copy.
        // Local edited b, remote deleted b: remote (delete) wins, local copy kept in the conflict.
        let local = set(&[("agents/b.md", "L")]);
        let remote = set(&[("agents/a.md", "R")]);
        let m = three_way(&base, &local, &remote);
        assert_eq!(m.merged, set(&[("agents/a.md", "R")]));
        assert!(m.to_push.is_empty());
        assert_eq!(
            m.conflicts,
            [
                Conflict {
                    path: "agents/a.md".into(),
                    local: None
                },
                Conflict {
                    path: "agents/b.md".into(),
                    local: Some(b"L".to_vec())
                },
            ]
        );
    }

    #[test]
    fn delete_on_one_side_unchanged_on_other() {
        let base = set(&[("agents/a.md", "1"), ("agents/b.md", "1")]);
        let local = set(&[("agents/b.md", "1")]);
        let remote = set(&[("agents/a.md", "1")]);
        let m = three_way(&base, &local, &remote);
        assert!(m.merged.is_empty());
        assert_eq!(
            m.to_push,
            BTreeMap::from([("agents/a.md".to_string(), None)])
        );
        let both_deleted = three_way(&base, &FileSet::new(), &FileSet::new());
        assert!(both_deleted.merged.is_empty() && both_deleted.to_push.is_empty());
    }

    #[test]
    fn json_keys_merge_three_way() {
        let p = "settings.shared.json";
        let base = set(&[(
            p,
            r#"{"model":"a","permissions":{"allow":["x"]},"keep":1,"drop":1}"#,
        )]);
        let local = set(&[(
            p,
            r#"{"model":"a","permissions":{"allow":["x"],"deny":["y"]},"keep":1,"drop":1,"mine":true}"#,
        )]);
        let remote = set(&[(
            p,
            r#"{"model":"b","permissions":{"allow":["x"]},"keep":1,"theirs":true}"#,
        )]);
        let m = three_way(&base, &local, &remote);
        let merged = json(&m.merged[p]);
        assert_eq!(
            merged,
            serde_json::json!({"model":"b","permissions":{"allow":["x"],"deny":["y"]},"keep":1,"theirs":true,"mine":true})
        );
        assert_eq!(json(m.to_push[p].as_ref().unwrap()), merged);
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn json_same_key_changed_both_ways_remote_wins() {
        let p = "claude-sync.json";
        let base = set(&[(p, r#"{"schema":1,"name":"a","members":["h"]}"#)]);
        let local = set(&[(p, r#"{"schema":1,"name":"L","members":["h","x"]}"#)]);
        let remote = set(&[(p, r#"{"schema":1,"name":"R","members":["h"]}"#)]);
        let m = three_way(&base, &local, &remote);
        assert_eq!(
            json(&m.merged[p]),
            serde_json::json!({"schema":1,"name":"R","members":["h","x"]})
        );
        assert_eq!(m.conflicts.len(), 1, "the lost local key is recorded");
        assert!(m.to_push.contains_key(p));
    }

    #[test]
    fn json_equal_values_push_nothing() {
        let p = "settings.shared.json";
        let base = set(&[(p, "{}")]);
        let local = set(&[(p, "{\"a\": 1}")]);
        let remote = set(&[(p, "{\n  \"a\": 1\n}\n")]);
        let m = three_way(&base, &local, &remote);
        assert_eq!(m.merged, remote);
        assert!(m.to_push.is_empty() && m.conflicts.is_empty());
    }

    #[test]
    fn non_object_json_falls_back_to_conflict() {
        let p = "settings.shared.json";
        let m = three_way(
            &set(&[(p, "{}")]),
            &set(&[(p, "[1]")]),
            &set(&[(p, r#"{"a":1}"#)]),
        );
        assert_eq!(m.merged, set(&[(p, r#"{"a":1}"#)]));
        assert_eq!(m.conflicts.len(), 1);
        // Missing base counts as an empty object.
        let m = three_way(
            &FileSet::new(),
            &set(&[(p, r#"{"a":1}"#)]),
            &set(&[(p, r#"{"b":2}"#)]),
        );
        assert_eq!(json(&m.merged[p]), serde_json::json!({"b":2,"a":1}));
        assert!(m.conflicts.is_empty());
    }
}
