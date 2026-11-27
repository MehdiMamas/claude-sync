//! Merges of plain data: the JSON-object key merge and the MEMORY.md line merge.
//!
//! v2-Parity rows covered: 43, 46.
use serde_json::{Map, Value};
use std::collections::HashMap;

/// The same key changed on both sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clash;

/// Three-way merge of JSON values by key. `Err` = the same key changed on both sides.
pub fn merge_json(
    b: Option<&Value>,
    l: Option<&Value>,
    r: Option<&Value>,
) -> Result<Option<Value>, Clash> {
    if l == r || r == b {
        return Ok(l.cloned());
    }
    if l == b {
        return Ok(r.cloned());
    }
    let (Some(Value::Object(lo)), Some(Value::Object(ro))) = (l, r) else {
        return Err(Clash);
    };
    let bo = match b {
        Some(Value::Object(o)) => Some(o),
        _ => None,
    };
    let mut out = Map::new();
    for k in lo.keys().chain(ro.keys().filter(|k| !lo.contains_key(*k))) {
        let bv = bo.and_then(|o| o.get(k));
        if let Some(v) = merge_json(bv, lo.get(k), ro.get(k))? {
            out.insert(k.clone(), v);
        }
    }
    Ok(Some(Value::Object(out)))
}

/// Merge of two JSON object files; `None` when one is not an object or a key clashes.
pub fn merge_json_bytes(b: Option<&[u8]>, l: &[u8], r: &[u8]) -> Option<Vec<u8>> {
    let p = |x: &[u8]| {
        serde_json::from_slice::<Value>(x)
            .ok()
            .filter(Value::is_object)
    };
    let (l, r) = (p(l)?, p(r)?);
    let b = b.and_then(p);
    let m = merge_json(b.as_ref(), Some(&l), Some(&r)).ok()??;
    // shortcut: output is re-serialised (2-space), not byte-preserving; upgrade if styles matter
    let mut v = serde_json::to_vec_pretty(&m).ok()?;
    v.push(b'\n');
    Some(v)
}

/// The `choices.json` merge (architecture §16): every section (`order`, `app`, `personal`,
/// `sshPeers`, `activity`, and each group under `groups`) is taken whole from the side with
/// the newer `ts`; a tie goes to the larger text so both devices settle on the same bytes.
pub fn merge_choices_bytes(l: &[u8], r: &[u8]) -> Option<Vec<u8>> {
    let p = |x: &[u8]| match serde_json::from_slice::<Value>(x) {
        Ok(Value::Object(o)) => Some(o),
        _ => None,
    };
    let (l, r) = (p(l)?, p(r)?);
    fn pick(l: Option<&Value>, r: Option<&Value>) -> Option<Value> {
        let ts = |v: Option<&Value>| v.and_then(|v| v.get("ts")).and_then(Value::as_u64);
        match (l, r) {
            (Some(a), Some(b)) => Some(match ts(l).cmp(&ts(r)) {
                std::cmp::Ordering::Greater => a.clone(),
                std::cmp::Ordering::Less => b.clone(),
                _ => {
                    let (x, y) = (a.to_string(), b.to_string());
                    if x >= y {
                        a.clone()
                    } else {
                        b.clone()
                    }
                }
            }),
            (a, b) => a.or(b).cloned(),
        }
    }
    let mut out = Map::new();
    for k in l.keys().chain(r.keys().filter(|k| !l.contains_key(*k))) {
        let v = match (k.as_str(), l.get(k), r.get(k)) {
            ("groups", Some(Value::Object(a)), Some(Value::Object(b))) => {
                let mut g = Map::new();
                for n in a.keys().chain(b.keys().filter(|n| !a.contains_key(*n))) {
                    g.extend(pick(a.get(n), b.get(n)).map(|v| (n.clone(), v)));
                }
                Some(Value::Object(g))
            }
            (_, a, b) => pick(a, b),
        };
        out.extend(v.map(|v| (k.clone(), v)));
    }
    let mut v = serde_json::to_vec_pretty(&Value::Object(out)).ok()?;
    v.push(b'\n');
    Some(v)
}

/// The non-blank lines of `t`, and which of them a blank line follows. Blank lines are not
/// content: they are merged as one flag per line so the merge is the same from either device.
fn content(t: &str) -> (Vec<&str>, HashMap<&str, bool>) {
    let all: Vec<&str> = t.lines().collect();
    let mut flags = HashMap::new();
    for (i, x) in all.iter().enumerate().filter(|(_, x)| !x.trim().is_empty()) {
        let blank = all.get(i + 1).is_some_and(|n| n.trim().is_empty());
        *flags.entry(*x).or_insert(false) |= blank;
    }
    (
        all.into_iter().filter(|x| !x.trim().is_empty()).collect(),
        flags,
    )
}

/// MEMORY.md merge: every line either side added is kept; a line removed on either side
/// (in base) is dropped. Local order first, remote additions after their anchor.
/// Blank lines are one flag per line ("a blank line follows"), merged three-way and written as
/// at most one, so two devices that differ only in blank lines (or line ends) settle on the same
/// lines instead of rewriting each other (a v1 push loop). The result uses local's line end and
/// ends with a newline if either side's does.
pub fn merge_lines(base: &str, local: &str, remote: &str) -> String {
    let ((b, bf), (l, lf), (r, rf)) = (content(base), content(local), content(remote));
    let removed = |x: &&str| b.contains(x) && !(l.contains(x) && r.contains(x));
    // a line set, as v1 says: a line either side repeats is kept once
    let mut out: Vec<&str> = Vec::new();
    for x in l.iter().filter(|x| !removed(x)) {
        if !out.contains(x) {
            out.push(x);
        }
    }
    // remote-only additions go after the line that preceded them in remote
    for (i, x) in r.iter().enumerate() {
        if b.contains(x) || out.contains(x) {
            continue;
        }
        let at = r[..i]
            .iter()
            .rev()
            .find_map(|p| out.iter().rposition(|o| o == p).map(|j| j + 1))
            .unwrap_or(out.len());
        out.insert(at, x);
    }
    // the same lines in another order is no change: the pushed order wins, so devices that
    // order them differently do not keep rewriting each other
    if out.iter().all(|x| r.contains(x)) && r.iter().all(|x| out.contains(x)) {
        out.clear();
        for x in &r {
            if !out.contains(x) {
                out.push(x);
            }
        }
    }
    let blank_after = |x: &str| match (lf.get(x), rf.get(x)) {
        (Some(a), Some(c)) if a == c => *a,
        // one side changed it: the change wins; new on both sides: a blank line if either has one
        (Some(a), Some(c)) => bf.get(x).map_or(*a || *c, |b| if a == b { *c } else { *a }),
        (a, c) => a.or(c).copied().unwrap_or(false),
    };
    let eol = if local.contains("\r\n") { "\r\n" } else { "\n" };
    let mut s = String::new();
    for (i, x) in out.iter().enumerate() {
        s.push_str(x);
        if i + 1 < out.len() {
            s.push_str(eol);
            if blank_after(x) {
                s.push_str(eol);
            }
        }
    }
    if !out.is_empty() && (local.ends_with('\n') || remote.ends_with('\n')) {
        s.push_str(eol);
    }
    s
}

#[cfg(test)]
mod choices_tests {
    use super::merge_choices_bytes;
    use serde_json::{json, Value};

    fn m(l: Value, r: Value) -> Value {
        let b = merge_choices_bytes(l.to_string().as_bytes(), r.to_string().as_bytes()).unwrap();
        serde_json::from_slice(&b).unwrap()
    }

    #[test]
    fn each_section_and_each_group_takes_the_newer_side() {
        let l = json!({"app": {"paused": true, "ts": 5}, "groups": {"a/b": {"off": true, "ts": 9}, "c/d": {"off": false, "ts": 1}}});
        let r = json!({"app": {"paused": false, "ts": 7}, "groups": {"a/b": {"off": false, "ts": 2}, "e/f": {"off": true, "ts": 3}}});
        let out = m(l.clone(), r.clone());
        assert_eq!(out["app"]["paused"], false);
        assert_eq!(out["groups"]["a/b"]["off"], true);
        assert!(out["groups"]["c/d"].is_object() && out["groups"]["e/f"].is_object());
        // the same result from either side, so two devices settle on the same bytes
        assert_eq!(out, m(r, l));
    }

    #[test]
    fn a_file_that_is_not_an_object_is_not_merged() {
        assert!(merge_choices_bytes(b"[]", b"{}").is_none());
    }
}
