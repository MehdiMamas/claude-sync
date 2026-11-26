//! Merges of plain data: the JSON-object key merge and the MEMORY.md line merge.
//!
//! v2-Parity rows covered: 43, 46.
use serde_json::{Map, Value};

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

fn lines(t: &str) -> Vec<&str> {
    t.lines().collect()
}

/// MEMORY.md merge: every line either side added is kept; a line removed on either side
/// (non-blank, in base) is dropped. Local order first, remote additions after their anchor.
/// The result uses local's line end and ends with a newline if either side's does.
pub fn merge_lines(base: &str, local: &str, remote: &str) -> String {
    let (b, l, r) = (lines(base), lines(local), lines(remote));
    let removed =
        |x: &&str| !x.trim().is_empty() && b.contains(x) && !(l.contains(x) && r.contains(x));
    let mut out: Vec<&str> = l.iter().copied().filter(|x| !removed(x)).collect();
    // remote-only additions go after the line that preceded them in remote
    for (i, x) in r.iter().enumerate() {
        if x.trim().is_empty() || b.contains(x) || out.contains(x) {
            continue;
        }
        let at = r[..i]
            .iter()
            .rev()
            .find_map(|p| out.iter().rposition(|o| o == p).map(|j| j + 1))
            .unwrap_or(out.len());
        out.insert(at, x);
    }
    let eol = if local.contains("\r\n") { "\r\n" } else { "\n" };
    let mut s = out.join(eol);
    let ends = |t: &str| t.ends_with('\n');
    if !out.is_empty() && (ends(local) || ends(remote)) {
        s.push_str(eol);
    }
    s
}
