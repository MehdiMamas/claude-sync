//! Joining a group on a machine that already has the group's text pasted into
//! `~/.claude/CLAUDE.md` by hand: the file is saved first to `<state>/backup/<time>/`, the
//! pasted text is taken out, and the sync delivers it again inside the managed block.
use claude_sync_engine::formats::text::{BLOCK_BEGIN, BLOCK_END};
use std::path::{Path, PathBuf};

/// Smallest run of the group's lines that counts as "the group text pasted in".
const MIN_RUN: usize = 3;

/// Copy `claude/CLAUDE.md` to `<state>/backup/<UTC time>/CLAUDE.md`. `None`: no such file.
pub fn backup(claude: &Path, state: &Path) -> Result<Option<PathBuf>, String> {
    let src = claude.join("CLAUDE.md");
    if !src.is_file() {
        return Ok(None);
    }
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let mut dir = state.join("backup").join(&stamp);
    let mut n = 1;
    while dir.exists() {
        n += 1;
        dir = state.join("backup").join(format!("{stamp}-{n}"));
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
        let mut best = 0usize;
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
                if solid >= need && k > best {
                    best = k;
                }
            }
        }
        if best > 0 {
            for flag in keep.iter_mut().skip(i).take(best) {
                *flag = false;
            }
            removed += best;
            i += best;
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

/// Take the pasted group text out of `claude/CLAUDE.md`; the number of lines removed.
pub fn remove_inlined(claude: &Path, group_text: &str) -> Result<usize, String> {
    let target = claude.join("CLAUDE.md");
    let Ok(existing) = std::fs::read_to_string(&target) else {
        return Ok(0);
    };
    let (stripped, removed) = strip_inlined(&existing, group_text);
    if removed == 0 {
        return Ok(0);
    }
    let tmp = target.with_extension("md.tmp");
    std::fs::write(&tmp, stripped.as_bytes())
        .and_then(|_| std::fs::rename(&tmp, &target))
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
        assert_eq!(strip_inlined(existing, GROUP), (existing.to_string(), 0));
    }

    #[test]
    fn a_short_group_text_is_matched_whole() {
        assert_eq!(
            strip_inlined("a\nOnly line\nb\n", "Only line\n"),
            ("a\nb\n".to_string(), 1)
        );
        assert_eq!(strip_inlined("a\n", "").1, 0);
    }

    #[test]
    fn backup_copies_the_file_and_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        let (claude, state) = (d.path().join("claude"), d.path().join("state"));
        assert_eq!(backup(&claude, &state).unwrap(), None);
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("CLAUDE.md"), "mine").unwrap();
        let a = backup(&claude, &state).unwrap().unwrap();
        let b = backup(&claude, &state).unwrap().unwrap();
        assert_ne!(a, b);
        assert_eq!(
            std::fs::read_to_string(a.join("CLAUDE.md")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn remove_inlined_rewrites_only_when_something_went() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("CLAUDE.md"), format!("mine\n{GROUP}")).unwrap();
        assert_eq!(remove_inlined(d.path(), GROUP).unwrap(), 5);
        assert_eq!(
            std::fs::read_to_string(d.path().join("CLAUDE.md")).unwrap(),
            "mine\n"
        );
        assert_eq!(remove_inlined(d.path(), GROUP).unwrap(), 0);
    }
}
