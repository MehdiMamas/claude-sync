//! `~/.claude/CLAUDE.md`: the managed block of group imports, and the user's own text for the
//! personal repo.
//!
//! Parity: one import line per joined group that has a CLAUDE.md copy, in the user's order,
//! none for a disabled group; the block uses `@~/...` under home; text outside the block is
//! never touched; the personal copy holds only the user's own text (no block, no group
//! imports); this device's block survives a personal pull.
//!
//! v2-Parity rows covered: 7, 25, 37, 43.
use crate::formats::text::{block_lines, render_claude_md};
use crate::paths::import_line;

/// One joined group as the block sees it.
#[derive(Debug, Clone)]
pub struct GroupCopy {
    /// Absolute path of the group's CLAUDE.md copy.
    pub file: String,
    pub disabled: bool,
    /// The copy is on disk.
    pub present: bool,
}

/// Import lines for the block, in the order given.
pub fn import_lines(groups: &[GroupCopy], home: Option<&str>) -> Vec<String> {
    groups
        .iter()
        .filter(|g| !g.disabled && g.present)
        .map(|g| import_line(&g.file, home))
        .collect()
}

/// `existing` with the block set for `groups`. `None`: no change (no block and no file stay
/// so). The caller deletes the file only if it created it and the result is empty.
pub fn compose_block(
    existing: Option<&[u8]>,
    groups: &[GroupCopy],
    home: Option<&str>,
) -> Option<Vec<u8>> {
    let lines = import_lines(groups, home);
    if lines.is_empty() && existing.is_none() {
        return None;
    }
    let out = render_claude_md(existing.unwrap_or_default(), &lines);
    (existing != Some(out.as_slice())).then_some(out)
}

fn is_group_import(line: &str) -> bool {
    let l = line.trim().replace('\\', "/").to_ascii_lowercase();
    l.starts_with('@') && l.contains("/.claude-sync/groups/") && l.ends_with("/claude.md")
}

/// The user's own text: no block, no group imports, `\n` line ends. `None` if only blanks.
pub fn strip(text: &[u8]) -> Option<Vec<u8>> {
    let outside = String::from_utf8_lossy(&render_claude_md(text, &[])).replace("\r\n", "\n");
    let kept: String = outside
        .split_inclusive('\n')
        .filter(|l| !is_group_import(l))
        .collect();
    let body = kept.trim_start_matches('\n');
    (!body.trim().is_empty()).then(|| body.as_bytes().to_vec())
}

/// The file to write after a personal pull: `synced` text under this device's block and stray
/// group imports, in `existing`'s line ends. `None`: nothing left, the file goes away.
pub fn compose(existing: Option<&[u8]>, synced: Option<&[u8]>) -> Option<Vec<u8>> {
    let existing = existing.unwrap_or_default();
    let crlf = existing
        .iter()
        .position(|b| *b == b'\n')
        .is_some_and(|i| i > 0 && existing[i - 1] == b'\r');
    let lines = block_lines(existing);
    let outside = String::from_utf8_lossy(&render_claude_md(existing, &[])).replace("\r\n", "\n");
    let imports: Vec<&str> = outside.lines().filter(|l| is_group_import(l)).collect();
    let mut body: String = imports.iter().map(|i| format!("{i}\n")).collect();
    if let Some(t) = synced {
        if !imports.is_empty() {
            body.push('\n');
        }
        body.push_str(&String::from_utf8_lossy(t).replace("\r\n", "\n"));
    }
    let mut out = render_claude_md(body.as_bytes(), &lines);
    if crlf {
        out = String::from_utf8_lossy(&out)
            .replace("\r\n", "\n")
            .replace('\n', "\r\n")
            .into_bytes();
    }
    (!out.is_empty()).then_some(out)
}
