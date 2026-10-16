//! Personal memory keys. Claude Code keeps a project's auto-memory in
//! `~/.claude/projects/<folder>/memory/`, where `<folder>` is the project's path with every
//! character other than an ASCII letter or digit replaced by `-`. That is lossy, so folder
//! names are never decoded: a folder under the home folder gets the key `home<rest>`, and
//! another device maps the key back to its own `encode(home)<rest>`.
use std::path::Path;

pub const HOME_KEY: &str = "home";

/// Longest key accepted from a repo (folder names stay well under path limits).
const MAX_KEY: usize = 200;

/// Claude Code's folder name for `path`: one `-` per UTF-16 unit that is not `[A-Za-z0-9]`.
pub fn encode(path: &str) -> String {
    path.encode_utf16()
        .map(|u| match char::from_u32(u32::from(u)) {
            Some(c) if c.is_ascii_alphanumeric() => c,
            _ => '-',
        })
        .collect()
}

/// Rests (the part after the home folder) that are never synced: temp folders, app data,
/// background job and worktree folders. They are per machine and short lived.
pub fn is_skipped_rest(rest: &str) -> bool {
    let rest = rest.to_ascii_lowercase();
    ["-appdata", "--claude-jobs", "-.claude-jobs"]
        .iter()
        .any(|p| rest.starts_with(p))
        || ["--claude-worktrees-", "--worktrees-"]
            .iter()
            .any(|p| rest.contains(p))
}

/// The key of a project folder, `None` when the folder is not under the home folder (this
/// covers `/tmp` and `/private/var/folders` on macOS) or is skipped. The home part matches
/// without regard to case: Windows has both `C--Users-…` and `c--Users-…`.
pub fn key_for(folder: &str, home_enc: &str) -> Option<String> {
    if !folder.is_ascii() || folder.len() < home_enc.len() || home_enc.is_empty() {
        return None;
    }
    let (head, rest) = folder.split_at(home_enc.len());
    if !head.eq_ignore_ascii_case(home_enc) || !(rest.is_empty() || rest.starts_with('-')) {
        return None;
    }
    let key = format!("{HOME_KEY}{rest}");
    is_key(&key).then_some(key)
}

/// A key as it may appear in a repo: `home` or `home-…` with letters, digits and `-`.
pub fn is_key(key: &str) -> bool {
    let Some(rest) = key.strip_prefix(HOME_KEY) else {
        return false;
    };
    key.len() <= MAX_KEY
        && (rest.is_empty() || rest.starts_with('-'))
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !is_skipped_rest(rest)
}

/// The folder a key gets on this device when it has none yet.
pub fn folder_for(key: &str, home_enc: &str) -> Option<String> {
    is_key(key).then(|| format!("{home_enc}{}", &key[HOME_KEY.len()..]))
}

/// One folder per key when several case variants exist: the one with a `memory/` folder,
/// then the one with the most memory files, then the one spelled like `home_enc`, then the
/// first by name.
pub fn choose(projects: &Path, mut names: Vec<String>, home_enc: &str) -> String {
    let rank = |name: &String| {
        let mem = projects.join(name).join("memory");
        let count = std::fs::read_dir(&mem).map_or(0, |d| d.count());
        (
            !mem.is_dir(),
            std::cmp::Reverse(count),
            !name.starts_with(home_enc),
        )
    };
    names.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    names.swap_remove(0)
}
