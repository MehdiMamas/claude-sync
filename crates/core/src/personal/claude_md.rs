//! `~/.claude/CLAUDE.md` in the personal repo: the user's own text only. The managed block
//! and group import lines belong to this device's groups and never leave it; on the way back
//! in, this device's block is kept where the apply layer put it.
use crate::apply::{block_lines, render_claude_md};

/// An `@…/.claude-sync/groups/<owner>__<repo>/CLAUDE.md` import line.
fn is_group_import(line: &str) -> bool {
    let line = line.trim().replace('\\', "/").to_ascii_lowercase();
    line.starts_with('@') && line.contains("/.claude-sync/groups/") && line.ends_with("/claude.md")
}

/// The user's text of a `CLAUDE.md`: without the managed block and group import lines,
/// with `\n` line ends. `None` when nothing but blank lines is left.
pub fn strip(text: &[u8]) -> Option<Vec<u8>> {
    let outside = render_claude_md(text, &[]);
    let outside = String::from_utf8_lossy(&outside).replace("\r\n", "\n");
    let kept: String = outside
        .split_inclusive('\n')
        .filter(|l| !is_group_import(l))
        .collect();
    let body = kept.trim_start_matches('\n');
    (!body.trim().is_empty()).then(|| body.as_bytes().to_vec())
}

/// The file to write: `synced` text under this device's block and stray group import lines,
/// in the line ends of `existing`. `None` when nothing is left (the file goes away).
pub fn compose(existing: Option<&[u8]>, synced: Option<&[u8]>) -> Option<Vec<u8>> {
    let existing = existing.unwrap_or_default();
    let crlf = existing
        .iter()
        .position(|b| *b == b'\n')
        .is_some_and(|i| i > 0 && existing[i - 1] == b'\r');
    let lines = block_lines(existing);
    let outside = String::from_utf8_lossy(&render_claude_md(existing, &[])).replace("\r\n", "\n");
    let imports: Vec<&str> = outside.lines().filter(|l| is_group_import(l)).collect();
    let mut body = String::new();
    for i in &imports {
        body.push_str(i);
        body.push('\n');
    }
    if let Some(text) = synced {
        if !imports.is_empty() {
            body.push('\n');
        }
        body.push_str(&String::from_utf8_lossy(text).replace("\r\n", "\n"));
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
