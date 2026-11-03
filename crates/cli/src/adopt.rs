//! Joining a group on a machine that already has the group's text pasted into
//! `~/.claude/CLAUDE.md` by hand (issue #158, decision C): the file is saved first to
//! `~/.claude-sync/backup/<time>/`, the pasted text is taken out, and the sync delivers it
//! again inside the managed block.
use claude_sync_core::apply::{BLOCK_BEGIN, BLOCK_END};
use claude_sync_core::{GroupId, Paths};
use std::path::PathBuf;

/// Smallest run of the group's lines that counts as "the group text pasted in".
const MIN_RUN: usize = 3;

/// Copy `~/.claude/CLAUDE.md` to `<state>/backup/<time>/CLAUDE.md`. `None`: no such file.
pub fn backup(paths: &Paths) -> Result<Option<PathBuf>, String> {
    let src = paths.claude_home.join("CLAUDE.md");
    if !src.is_file() {
        return Ok(None);
    }
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let mut dir = paths.state_dir.join("backup").join(&stamp);
    let mut n = 1;
    while dir.exists() {
        n += 1;
        dir = paths.state_dir.join("backup").join(format!("{stamp}-{n}"));
    }
    let err = |e: std::io::Error| format!("cannot back up {}: {e}", src.display());
    std::fs::create_dir_all(&dir).map_err(err)?;
    std::fs::copy(&src, dir.join("CLAUDE.md")).map_err(err)?;
    Ok(Some(dir))
}

/// `existing` without the runs of `group_text` pasted outside the managed block, and how
/// many lines went. A run is at least [`MIN_RUN`] of the group's non-blank lines in a row
/// (fewer if the group has fewer); blank lines inside it go with it.
pub fn strip_inlined(existing: &str, group_text: &str) -> (String, usize) {
    let group: Vec<&str> = group_text.lines().map(|l| l.trim_end()).collect();
    let need = MIN_RUN.min(group.iter().filter(|l| !l.is_empty()).count());
    if need == 0 {
        return (existing.to_string(), 0);
    }
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let mut keep = vec![true; lines.len()];
    let mut in_block = false;
    let mut protected = vec![false; lines.len()];
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_end();
        if t == BLOCK_BEGIN {
            in_block = true;
        }
        protected[i] = in_block;
        if t == BLOCK_END {
            in_block = false;
        }
    }
    let mut i = 0;
    let mut removed = 0;
    while i < lines.len() {
        let mut best = (0usize, 0usize); // (length in existing lines, non-blank lines)
        if !protected[i] && !lines[i].trim_end().is_empty() {
            for j in 0..group.len() {
                if group[j] != lines[i].trim_end() {
                    continue;
                }
                let (mut k, mut solid) = (0, 0);
                while i + k < lines.len()
                    && j + k < group.len()
                    && !protected[i + k]
                    && lines[i + k].trim_end() == group[j + k]
                {
                    solid += usize::from(!group[j + k].is_empty());
                    k += 1;
                }
                // A run does not end on a blank line.
                while k > 0 && group[j + k - 1].is_empty() {
                    k -= 1;
                }
                if solid >= need && k > best.0 {
                    best = (k, solid);
                }
            }
        }
        if best.0 > 0 {
            for flag in keep.iter_mut().skip(i).take(best.0) {
                *flag = false;
            }
            removed += best.0;
            i += best.0;
        } else {
            i += 1;
        }
    }
    let out: String = lines
        .iter()
        .zip(&keep)
        .filter_map(|(l, k)| k.then_some(*l))
        .collect();
    (out, removed)
}

/// After a join: take the pasted group text out of `~/.claude/CLAUDE.md`. Returns the
/// number of lines removed.
pub fn remove_inlined(paths: &Paths, group: &GroupId) -> Result<usize, String> {
    let copy = paths
        .group_dir(group)
        .join(claude_sync_core::group::CLAUDE_MD);
    let target = paths.claude_home.join("CLAUDE.md");
    let (Ok(group_text), Ok(existing)) = (
        std::fs::read_to_string(&copy),
        std::fs::read_to_string(&target),
    ) else {
        return Ok(0);
    };
    let (stripped, removed) = strip_inlined(&existing, &group_text);
    if removed == 0 {
        return Ok(0);
    }
    claude_sync_core::state::write_atomic(&target, stripped.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUP: &str =
        "# Team rules\n\n- Use worktrees.\n- Run tests first.\n- No force pushes.\n";

    #[test]
    fn takes_out_the_pasted_group_text_and_keeps_the_rest() {
        let mine = "# My notes\nI like tabs.\n\n";
        let existing = format!("{mine}{GROUP}\n## Mine too\nKeep this.\n");
        let (out, removed) = strip_inlined(&existing, GROUP);
        assert_eq!(out, format!("{mine}\n## Mine too\nKeep this.\n"));
        assert_eq!(removed, 5);
    }

    #[test]
    fn leaves_the_managed_block_alone() {
        let existing = format!("{BLOCK_BEGIN}\n{GROUP}{BLOCK_END}\nmine\n");
        let (out, removed) = strip_inlined(&existing, GROUP);
        assert_eq!(out, existing);
        assert_eq!(removed, 0);
    }

    #[test]
    fn a_stray_line_or_two_is_not_the_group_text() {
        let existing = "- Use worktrees.\n- Run tests first.\nsomething else\n";
        let (out, removed) = strip_inlined(existing, GROUP);
        assert_eq!(out, existing);
        assert_eq!(removed, 0);
    }

    #[test]
    fn a_short_group_text_is_matched_whole() {
        let (out, removed) = strip_inlined("a\nOnly line\nb\n", "Only line\n");
        assert_eq!(out, "a\nb\n");
        assert_eq!(removed, 1);
        assert_eq!(strip_inlined("a\n", "").1, 0);
    }

    #[test]
    fn backup_copies_the_file_and_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths {
            claude_home: d.path().join("claude"),
            state_dir: d.path().join("state"),
        };
        assert_eq!(backup(&p).unwrap(), None);
        std::fs::create_dir_all(&p.claude_home).unwrap();
        std::fs::write(p.claude_home.join("CLAUDE.md"), "mine").unwrap();
        let a = backup(&p).unwrap().unwrap();
        let b = backup(&p).unwrap().unwrap();
        assert_ne!(a, b);
        assert_eq!(
            std::fs::read_to_string(a.join("CLAUDE.md")).unwrap(),
            "mine"
        );
    }
}
