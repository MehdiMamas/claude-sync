//! `state/**`: small per-user state that mods save under `~/.claude/personal-state/`, such
//! as model-router's opus-only mode. Two devices changing it is normal, so two kinds of
//! file merge without conflicts:
//! - `*.json` objects with a top-level `ts` (ISO-8601): the newer one wins.
//! - `*.jsonl` logs: the union of both sides' lines, sorted by each line's `ts`.
use serde_json::Value;
use std::collections::HashSet;

pub const DIR: &str = "state";
/// The folder under `~/.claude` that `state/**` maps to.
pub const LOCAL_DIR: &str = "personal-state";

/// `state/**/*.json`.
pub fn is_json(path: &str) -> bool {
    path.starts_with("state/") && path.ends_with(".json")
}

/// `state/**/*.jsonl`.
pub fn is_jsonl(path: &str) -> bool {
    path.starts_with("state/") && path.ends_with(".jsonl")
}

/// The newer of two JSON objects by their `ts`, or `None` when either side is not a JSON
/// object with a parseable string `ts` (then the normal three-way merge decides).
/// Equal `ts` keeps the remote.
pub fn newer<'a>(local: &'a [u8], remote: &'a [u8]) -> Option<&'a [u8]> {
    let (l, r) = (ts_of_doc(local)?, ts_of_doc(remote)?);
    Some(if l > r { local } else { remote })
}

fn ts_of_doc(bytes: &[u8]) -> Option<(i64, u32)> {
    ts_of(&serde_json::from_slice::<Value>(bytes).ok()?)
}

fn ts_of(v: &Value) -> Option<(i64, u32)> {
    parse_ts(v.as_object()?.get("ts")?.as_str()?)
}

/// Union of the lines of `local` and `remote`, empty lines dropped, exact duplicates
/// dropped, stably sorted by each line's `ts` (lines without one first). A file one side
/// deleted while the other left it as in `base` is deleted. Never conflicts.
pub fn merge_log(
    base: Option<&[u8]>,
    local: Option<&[u8]>,
    remote: Option<&[u8]>,
) -> Option<Vec<u8>> {
    match (local, remote) {
        (None, None) => return None,
        (None, Some(r)) if base == Some(r) => return None,
        (Some(l), None) if base == Some(l) => return None,
        _ => {}
    }
    let mut seen = HashSet::new();
    let mut lines: Vec<(Option<(i64, u32)>, String)> = Vec::new();
    for side in [local, remote].into_iter().flatten() {
        for line in String::from_utf8_lossy(side).lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || !seen.insert(line.to_string()) {
                continue;
            }
            let ts = serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|v| ts_of(&v));
            lines.push((ts, line.to_string()));
        }
    }
    // `None` sorts before every `Some`; `sort_by` is stable.
    lines.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::new();
    for (_, line) in lines {
        out.push_str(&line);
        out.push('\n');
    }
    Some(out.into_bytes())
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` as (unix seconds, nanoseconds).
pub fn parse_ts(s: &str) -> Option<(i64, u32)> {
    let b = s.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let part = s.get(from..from + len)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, sec) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut nanos = 0u32;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let frac = &s[start..i];
        if frac.is_empty() {
            return None;
        }
        let digits: String = frac.chars().chain(std::iter::repeat('0')).take(9).collect();
        nanos = digits.parse().ok()?;
    }
    let offset = match &s[i..] {
        "Z" | "z" => 0,
        rest if rest.len() == 6 && matches!(rest.as_bytes()[0], b'+' | b'-') => {
            if rest.as_bytes()[3] != b':' {
                return None;
            }
            let secs = num(i + 1, 2)? * 3600 + num(i + 4, 2)? * 60;
            if rest.starts_with('-') {
                -secs
            } else {
                secs
            }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    Some((days * 86400 + h * 3600 + mi * 60 + sec - offset, nanos))
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
