//! Path rules: which repo paths the engine plans, and that no planned write leaves the managed
//! folders (v2-EngRules path safety). Parity: managed targets only, no `..`, no absolute, no
//! write through a link (the planned-path part; the real filesystem check is in the fs step).
//!
//! v2-Parity rows covered: 25, 43.
use crate::formats::group::{SyncFlags, SHARED_SETTINGS};
use std::collections::BTreeSet;

/// One path segment that cannot escape its folder or mean something special on Windows.
pub fn safe_segment(s: &str) -> bool {
    if s.is_empty() || s == "." || s == ".." || s.ends_with('.') || s.ends_with(' ') {
        return false;
    }
    if s.chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return false;
    }
    let stem = s.split('.').next().unwrap_or(s).to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    !reserved
}

/// `skills/<name>/**`, a `.md` file under `agents/`, `commands/` or `output-styles/`, or
/// `plugins/<marketplace>/**`, every segment safe.
pub fn is_managed_target(rel: &str) -> bool {
    let segs: Vec<&str> = rel.split('/').collect();
    if !segs.iter().all(|s| safe_segment(s)) {
        return false;
    }
    match segs[0] {
        "skills" => segs.len() >= 3,
        "agents" | "commands" | "output-styles" => segs.len() >= 2 && rel.ends_with(".md"),
        "plugins" => segs.len() >= 3,
        _ => false,
    }
}

/// A path the group planner handles: a managed target or `settings.shared.json`.
/// (`CLAUDE.md` goes through review, `claude-sync.json` and the rest are repo-only.)
pub fn is_planned_path(rel: &str) -> bool {
    rel == SHARED_SETTINGS || is_managed_target(rel)
}

/// Whether the group's sync switches let `rel` through.
pub fn allowed_by(flags: &SyncFlags, rel: &str) -> bool {
    match rel.split('/').next().unwrap_or("") {
        "skills" => flags.skills,
        "agents" => flags.agents,
        "commands" => flags.commands,
        "output-styles" => flags.output_styles,
        "plugins" => flags.plugins,
        _ => rel != SHARED_SETTINGS || flags.settings,
    }
}

/// `rel` is a known link, or lies under one (`links` holds repo paths found to be links).
pub fn under_link(rel: &str, links: &BTreeSet<String>) -> bool {
    links.iter().any(|l| {
        rel == l
            || rel
                .strip_prefix(l.as_str())
                .is_some_and(|r| r.starts_with('/'))
    })
}

/// `@~/<path under home>` (forward slashes), else `@<absolute path>`: how the managed block
/// imports a group's CLAUDE.md copy.
pub fn import_line(file: &str, home: Option<&str>) -> String {
    let file = file.replace('\\', "/");
    let home = home.map(|h| h.replace('\\', "/"));
    if let Some(rel) = home
        .as_deref()
        .map(|h| h.trim_end_matches('/'))
        .filter(|h| !h.is_empty())
        .and_then(|h| file.strip_prefix(h))
        .and_then(|r| r.strip_prefix('/'))
    {
        return format!("@~/{rel}");
    }
    format!("@{file}")
}

/// `home` or `home-<alphanumerics and dashes>`, at most 200 characters (v1 memory keys).
pub fn is_memory_key(k: &str) -> bool {
    k.len() <= 200
        && k.strip_prefix("home").is_some_and(|rest| {
            rest.is_empty()
                || rest
                    .strip_prefix('-')
                    .is_some_and(|r| r.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        })
}

/// Project folders that are never shared: app data, job runs and worktrees.
pub fn is_skipped_key(k: &str) -> bool {
    let rest = k.strip_prefix("home").unwrap_or(k);
    rest.starts_with("-appdata")
        || rest.starts_with("--claude-jobs")
        || rest.starts_with("-.claude-jobs")
        || rest.contains("--claude-worktrees-")
        || rest.contains("--worktrees-")
}

/// `memory/<key>/MEMORY.md`.
pub fn is_memory_index(rel: &str) -> bool {
    let s: Vec<&str> = rel.split('/').collect();
    s.len() == 3 && s[0] == "memory" && s[2] == "MEMORY.md" && is_memory_key(s[1])
}

/// A path the personal repo may hold and the planner may touch.
pub fn is_personal_path(rel: &str) -> bool {
    let s: Vec<&str> = rel.split('/').collect();
    if !s.iter().all(|x| safe_segment(x)) {
        return false;
    }
    match s[0] {
        "claude-sync-personal.json"
        | "devices.json"
        | "README.md"
        | "CLAUDE.md"
        | "settings.json" => s.len() == 1,
        "memory" => {
            s.len() >= 3
                && is_memory_key(s[1])
                && !is_skipped_key(s[1])
                && s[2..].iter().all(|x| !x.starts_with('.'))
        }
        "machines" => s.len() == 2 && s[1].ends_with(".json"),
        "state" => s.len() >= 2,
        _ => false,
    }
}
