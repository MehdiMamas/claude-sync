//! `MEMORY.md` is an index: one `- [Title](file.md) — hook` line per memory. Two devices
//! adding memories at the same time is normal, so it merges as a set of lines instead of
//! conflicting.
use std::collections::HashSet;

/// `memory/<key>/MEMORY.md`.
pub fn is_index(path: &str) -> bool {
    matches!(
        path.split('/').collect::<Vec<_>>()[..],
        ["memory", _, "MEMORY.md"]
    )
}

/// Three-way line merge. A line one side removed (it is in `base`) is dropped; every other
/// line of either side is kept, in local order, with remote's new lines placed after the
/// line that precedes them on the remote (after local's new lines there). Exact duplicate
/// lines are dropped (blank lines stay). With no base this is the union. Never conflicts.
/// `None` = the file is deleted.
pub fn merge(base: Option<&[u8]>, local: Option<&[u8]>, remote: Option<&[u8]>) -> Option<Vec<u8>> {
    let (l, r) = match (local, remote) {
        (None, None) => return None,
        // One side deleted the file: deleted if the other side left it alone.
        (None, Some(r)) => return (base != Some(r)).then(|| r.to_vec()),
        (Some(l), None) => return (base != Some(l)).then(|| l.to_vec()),
        (Some(l), Some(r)) if l == r => return Some(l.to_vec()),
        (Some(l), Some(r)) => (l, r),
    };
    let split = |b: &[u8]| -> Vec<String> {
        String::from_utf8_lossy(b)
            .lines()
            .map(str::to_string)
            .collect()
    };
    let base_lines: HashSet<String> = base.map(split).unwrap_or_default().into_iter().collect();
    let (l_lines, r_lines) = (split(l), split(r));
    let l_set: HashSet<&String> = l_lines.iter().collect();
    let r_set: HashSet<&String> = r_lines.iter().collect();
    let removed = |x: &String| {
        !x.trim().is_empty() && base_lines.contains(x) && !(l_set.contains(x) && r_set.contains(x))
    };

    let mut out: Vec<String> = l_lines.iter().filter(|x| !removed(x)).cloned().collect();
    let mut anchor: Option<usize> = None;
    for line in &r_lines {
        if line.trim().is_empty() || removed(line) {
            continue;
        }
        if let Some(i) = out.iter().position(|x| x == line) {
            anchor = Some(i);
            continue;
        }
        // After the anchor, and after the lines only local added there.
        let mut at = anchor.map_or(0, |i| i + 1);
        while out
            .get(at)
            .is_some_and(|x| !x.trim().is_empty() && !base_lines.contains(x) && !r_set.contains(x))
        {
            at += 1;
        }
        out.insert(at, line.clone());
        anchor = Some(at);
    }
    let mut seen = HashSet::new();
    out.retain(|x| x.trim().is_empty() || seen.insert(x.clone()));

    let eol = if l.windows(2).any(|w| w == b"\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = out.join(eol);
    if l.ends_with(b"\n") || r.ends_with(b"\n") {
        text.push_str(eol);
    }
    Some(text.into_bytes())
}
