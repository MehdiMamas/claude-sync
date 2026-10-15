//! Sharing the user's own `~/.claude/CLAUDE.md` text with a group: as the first `CLAUDE.md`
//! of a group they create, or appended to the `CLAUDE.md` of a group they are in.
//!
//! The text offered is the user's file without the managed block and group import lines
//! (the same cut as personal memory makes), with secrets redacted. The user edits it before
//! it is committed. Since the group's copy comes back through the managed block, the user
//! can also have the shared lines taken out of their own text so Claude does not read them
//! twice; the block stays as it is and the old file is backed up first.
use super::Engine;
use crate::apply::Backups;
use crate::personal::claude_md as own;
use crate::remote::Changes;
use crate::state::write_atomic;
use crate::{group, Error, GroupId};
use serde::{Deserialize, Serialize};

/// What the user chose to share in the preview.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaudeMdShare {
    /// The text to commit, as the user left it in the preview. Secrets are redacted again.
    pub text: String,
    /// Take the shared lines out of the user's own `CLAUDE.md` once the group's copy is on
    /// this device.
    pub remove_own: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeMdShared {
    /// `false`: the group's `CLAUDE.md` already had the text, nothing was committed.
    pub appended: bool,
    pub removed_own: bool,
}

/// `text` as a group's `CLAUDE.md` gets it: secrets redacted, `\n` line ends, no blank
/// lines around it. `None` when it is blank.
pub(super) fn group_text(text: &str) -> Option<String> {
    let text = crate::secrets::redact(&text.replace("\r\n", "\n"));
    let text = text.trim_end().trim_start_matches('\n');
    (!text.trim().is_empty()).then(|| format!("{text}\n"))
}

/// Lines with trailing spaces dropped, to find text the group already has.
fn normalized(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    lines.join("\n")
}

/// The group's `CLAUDE.md` in `files` with `text` added under `## {heading}` (just `text`
/// when the file is empty). `None` when the file already has the text.
pub(super) fn appended(files: &crate::FileSet, text: &str, heading: &str) -> Option<Vec<u8>> {
    let old = files
        .get(group::CLAUDE_MD)
        .map(|b| String::from_utf8_lossy(b).replace("\r\n", "\n"))
        .unwrap_or_default();
    if normalized(&old).contains(&normalized(text)) {
        return None;
    }
    let new = match old.trim_end() {
        "" => text.to_string(),
        old => format!("{old}\n\n## {heading}\n\n{text}"),
    };
    Some(new.into_bytes())
}

fn non_blank_lines(text: &str) -> usize {
    text.lines().filter(|l| !l.trim().is_empty()).count()
}

impl Engine {
    /// The user's own `CLAUDE.md` text as it would be shared: without the managed block and
    /// group import lines, secrets redacted. Empty when there is none.
    pub fn claude_md_preview(&self) -> crate::Result<String> {
        let file = self.paths.claude_home.join(group::CLAUDE_MD);
        let Some(existing) = read_opt(&file)? else {
            return Ok(String::new());
        };
        Ok(own::strip(&existing)
            .and_then(|t| group_text(&String::from_utf8_lossy(&t)))
            .unwrap_or_default())
    }

    /// Append `share.text` to the group's `CLAUDE.md`, under a heading with this user's
    /// login. Nothing is committed when the group's copy already has the text.
    pub async fn share_claude_md(
        &self,
        id: &GroupId,
        share: &ClaudeMdShare,
    ) -> crate::Result<ClaudeMdShared> {
        let text = group_text(&share.text)
            .ok_or_else(|| Error::Invalid("there is no text to share".into()))?;
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        self.scan(&Changes::from([(
            group::CLAUDE_MD.to_string(),
            Some(text.clone().into_bytes()),
        )]))?;
        let heading = format!("From {}", self.login);
        let appended = self
            .commit_to_group(id, "share CLAUDE.md", |gf, current| {
                if !gf.sync.claude_md {
                    return Err(Error::Invalid(format!("{id} does not sync CLAUDE.md")));
                }
                Ok(appended(current, &text, &heading)
                    .map(|new| Changes::from([(group::CLAUDE_MD.to_string(), Some(new))])))
            })
            .await?;
        if let (_, Some(e)) = self.sync_group(id, None).await {
            return Err(e);
        }
        let removed_own = share.remove_own && self.remove_own_claude_md(id, &text)?;
        Ok(ClaudeMdShared {
            appended,
            removed_own,
        })
    }

    /// Take the lines of `shared` out of the user's own `CLAUDE.md` text. Only once the
    /// group's `CLAUDE.md` is applied on this device (not kept local), so nothing is lost.
    /// The managed block and import lines stay; the old file is backed up first.
    pub(super) fn remove_own_claude_md(&self, id: &GroupId, shared: &str) -> crate::Result<bool> {
        if !self
            .apply
            .local_files(&self.paths, id)?
            .contains_key(group::CLAUDE_MD)
        {
            return Ok(false);
        }
        self.rewrite_own_claude_md(|mine| {
            let left = own::without_lines(mine, shared);
            let changed =
                non_blank_lines(left.as_deref().unwrap_or_default()) != non_blank_lines(mine);
            changed.then_some(left)
        })
    }

    /// The user's own text in `~/.claude/CLAUDE.md`: without the managed block and group
    /// import lines. `None` when there is none.
    pub(super) fn own_claude_md(&self) -> crate::Result<Option<String>> {
        let file = self.paths.claude_home.join(group::CLAUDE_MD);
        Ok(read_opt(&file)?
            .and_then(|b| own::strip(&b))
            .map(|t| String::from_utf8_lossy(&t).into_owned()))
    }

    /// Rewrite the user's own text with `edit`; the managed block and import lines stay and
    /// the old file is backed up first. `edit` returns `None` to leave the file alone, and
    /// `Some(None)` when no text is left. `false` when nothing was written.
    pub(super) fn rewrite_own_claude_md(
        &self,
        edit: impl FnOnce(&str) -> Option<Option<String>>,
    ) -> crate::Result<bool> {
        let file = self.paths.claude_home.join(group::CLAUDE_MD);
        let Some(existing) = read_opt(&file)? else {
            return Ok(false);
        };
        let Some(mine) = own::strip(&existing) else {
            return Ok(false);
        };
        let Some(left) = edit(&String::from_utf8_lossy(&mine)) else {
            return Ok(false);
        };
        Backups::new(&self.paths).save(group::CLAUDE_MD, &existing)?;
        match own::compose(Some(&existing), left.as_deref().map(str::as_bytes)) {
            Some(bytes) => write_atomic(&file, &bytes)?,
            None => std::fs::remove_file(&file)?,
        }
        Ok(true)
    }
}

fn read_opt(file: &std::path::Path) -> crate::Result<Option<Vec<u8>>> {
    match std::fs::read(file) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::{block_lines, BLOCK_BEGIN, BLOCK_END};
    use crate::engine::tests::{device, fake_scanner, group_repo, joined_host, Device};
    use crate::engine::ApplyLayer;
    use crate::group::GroupFile;
    use crate::remote::fake::FakeGitHub;
    use std::sync::{Arc, Mutex};

    const TOKEN: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

    /// A `~/.claude/CLAUDE.md` with another group's block and a stray import line around
    /// the user's own text.
    fn write_own(d: &Device, text: &str) -> String {
        let file = format!(
            "{BLOCK_BEGIN}\n@~/.claude-sync/groups/acme__team/CLAUDE.md\n{BLOCK_END}\n\n\
             @~/.claude-sync/groups/old__crew/CLAUDE.md\n{text}"
        );
        let home = &d.engine.paths.claude_home;
        std::fs::create_dir_all(home).unwrap();
        std::fs::write(home.join(group::CLAUDE_MD), &file).unwrap();
        file
    }

    fn read_own(d: &Device) -> String {
        std::fs::read_to_string(d.engine.paths.claude_home.join(group::CLAUDE_MD)).unwrap()
    }

    /// Every file under `backups/`, with its text.
    fn backups(d: &Device) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(stamps) = std::fs::read_dir(d.engine.paths.backups_dir()) else {
            return out;
        };
        for stamp in stamps.flatten() {
            let f = stamp.path().join(group::CLAUDE_MD);
            if let Ok(t) = std::fs::read_to_string(f) {
                out.push(t);
            }
        }
        out
    }

    fn share(text: &str, remove_own: bool) -> ClaudeMdShare {
        ClaudeMdShare {
            text: text.into(),
            remove_own,
        }
    }

    #[test]
    fn group_text_redacts_and_trims() {
        assert_eq!(group_text(" \n\r\n"), None);
        assert_eq!(group_text("\n\n# A\r\nb  \n\n"), Some("# A\nb\n".into()));
        let t = group_text(&format!("token {TOKEN}\n")).unwrap();
        assert!(!t.contains(TOKEN), "{t}");
        assert!(t.contains("[redacted:ghp]"), "{t}");
    }

    #[test]
    fn without_lines_keeps_what_was_not_shared() {
        let mine = "# Me\n\nUse tabs.\nMy email is me@x.org.\n\n## Git\nNo stash.\n";
        let shared = "# Me\n\nUse tabs.\n\n## Git\nNo stash.\n";
        assert_eq!(
            own::without_lines(mine, shared).as_deref(),
            Some("My email is me@x.org.\n")
        );
        assert_eq!(own::without_lines(mine, mine), None);
        assert_eq!(
            own::without_lines("a\na\n", "a  \n").as_deref(),
            Some("a\n")
        );
    }

    #[test]
    fn the_preview_is_the_users_own_text_redacted() {
        let world = FakeGitHub::new();
        let ana = device(&world, "ana", "mac");
        assert_eq!(ana.engine.claude_md_preview().unwrap(), "");
        write_own(&ana, &format!("# Ana\nKey: {TOKEN}\n"));
        let preview = ana.engine.claude_md_preview().unwrap();
        assert_eq!(preview, "# Ana\nKey: [redacted:ghp]\n");
        write_own(&ana, "\n");
        assert_eq!(ana.engine.claude_md_preview().unwrap(), "");
    }

    #[tokio::test]
    async fn create_group_commits_the_shared_text() {
        let world = FakeGitHub::new();
        let ana = device(&world, "ana", "mac");
        let before = write_own(&ana, &format!("# Ana\nUse tabs.\nKey: {TOKEN}\n"));
        let preview = ana.engine.claude_md_preview().unwrap();
        ana.engine
            .create_group("Friends", "", Some(&share(&preview, false)))
            .await
            .unwrap();
        let id = GroupId::new("ana", group::DEFAULT_REPO);
        let repo = world.repo(&id).unwrap();
        let text = repo.file(group::CLAUDE_MD).unwrap();
        assert_eq!(text, "# Ana\nUse tabs.\nKey: [redacted:ghp]\n");
        assert!(!text.contains("claude-sync:begin") && !text.contains(".claude-sync/groups"));
        assert_eq!(repo.commits().len(), 1);
        assert_eq!(ana.apply.file(&id, group::CLAUDE_MD).unwrap(), text);
        assert_eq!(read_own(&ana), before, "the user's file is left alone");
        assert!(backups(&ana).is_empty());
    }

    #[tokio::test]
    async fn create_group_secrets_typed_into_the_preview_are_redacted() {
        let world = FakeGitHub::new();
        let ana = device(&world, "ana", "mac");
        ana.engine
            .create_group("Friends", "", Some(&share(&format!("x {TOKEN}\n"), false)))
            .await
            .unwrap();
        let repo = world
            .repo(&GroupId::new("ana", group::DEFAULT_REPO))
            .unwrap();
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), "x [redacted:ghp]\n");
    }

    #[tokio::test]
    async fn create_group_with_blank_text_keeps_the_heading_seed() {
        let world = FakeGitHub::new();
        let ana = device(&world, "ana", "mac");
        ana.engine
            .create_group("Friends", "", Some(&share("\n  \n", true)))
            .await
            .unwrap();
        let repo = world
            .repo(&GroupId::new("ana", group::DEFAULT_REPO))
            .unwrap();
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), "# Friends\n");
    }

    #[tokio::test]
    async fn create_group_can_take_the_text_out_of_the_users_file() {
        let world = FakeGitHub::new();
        let ana = device(&world, "ana", "mac");
        let before = write_own(&ana, "# Ana\n\nUse tabs.\nMy email is ana@x.org.\n");
        // The user removed the personal line from the preview.
        ana.engine
            .create_group("Friends", "", Some(&share("# Ana\n\nUse tabs.\n", true)))
            .await
            .unwrap();
        let after = read_own(&ana);
        assert_eq!(
            block_lines(after.as_bytes()),
            block_lines(before.as_bytes()),
            "the managed block is kept"
        );
        assert!(after.contains("@~/.claude-sync/groups/old__crew/CLAUDE.md"));
        assert_eq!(
            own::strip(after.as_bytes()).unwrap(),
            b"My email is ana@x.org.\n"
        );
        assert_eq!(backups(&ana), [before], "backed up before the change");
    }

    #[tokio::test]
    async fn share_appends_once_under_a_heading() {
        let (_, id, repo, bo) = joined_host().await;
        let before = write_own(&bo, "Use tabs.\n");
        let commits = repo.commits().len();
        let done = bo
            .engine
            .share_claude_md(&id, &share("Use tabs.\n", false))
            .await
            .unwrap();
        assert_eq!(
            done,
            ClaudeMdShared {
                appended: true,
                removed_own: false
            }
        );
        let text = "# Friends\n\n## From bo\n\nUse tabs.\n";
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), text);
        assert_eq!(repo.commits().len(), commits + 1);
        assert_eq!(
            repo.commits().pop().unwrap().message,
            "bo@pc: share CLAUDE.md"
        );
        assert_eq!(bo.apply.file(&id, group::CLAUDE_MD).unwrap(), text);
        assert_eq!(read_own(&bo), before);

        // Already there: nothing is committed, but the user's copy can still go.
        let again = bo
            .engine
            .share_claude_md(&id, &share("Use tabs.  \r\n", true))
            .await
            .unwrap();
        assert_eq!(
            again,
            ClaudeMdShared {
                appended: false,
                removed_own: true
            }
        );
        assert_eq!(repo.commits().len(), commits + 1);
        assert_eq!(own::strip(read_own(&bo).as_bytes()), None);
        assert_eq!(block_lines(read_own(&bo).as_bytes()).len(), 1);
        assert_eq!(backups(&bo), [before]);

        let err = bo.engine.share_claude_md(&id, &share(" \n", false)).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn share_is_refused_when_the_group_does_not_sync_claude_md() {
        let (_, id, repo, bo) = joined_host().await;
        let mut gf = GroupFile::parse(repo.file(group::FILE).unwrap().as_bytes()).unwrap();
        gf.sync.claude_md = false;
        repo.set_file(group::FILE, &String::from_utf8(gf.to_bytes()).unwrap());
        let commits = repo.commits().len();
        let err = bo
            .engine
            .share_claude_md(&id, &share("Use tabs.\n", true))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not sync CLAUDE.md"), "{err}");
        assert_eq!(repo.commits().len(), commits);
    }

    #[tokio::test]
    async fn share_scans_for_secrets() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let mut bo = device(&world, "bo", "pc");
        bo.engine = bo.engine.with_scanner(fake_scanner);
        bo.engine.join(&id).await.unwrap();
        let err = bo
            .engine
            .share_claude_md(&id, &share("ghp_123\n", false))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::SecretFound { .. }), "{err}");
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), "# Friends\n");
    }

    #[tokio::test]
    async fn share_appends_to_a_concurrent_change() {
        let (_, id, repo, bo) = joined_host().await;
        let raced = Arc::new(Mutex::new(false));
        let flag = raced.clone();
        repo.before_commit(move |r| {
            let mut done = flag.lock().unwrap();
            if !*done {
                *done = true;
                r.set_file(group::CLAUDE_MD, "# Friends\n\nBe kind.\n");
            }
        });
        bo.engine
            .share_claude_md(&id, &share("Use tabs.\n", false))
            .await
            .unwrap();
        assert!(*raced.lock().unwrap());
        assert_eq!(
            repo.file(group::CLAUDE_MD).unwrap(),
            "# Friends\n\nBe kind.\n\n## From bo\n\nUse tabs.\n"
        );
    }

    #[tokio::test]
    async fn the_users_text_stays_when_the_group_copy_is_kept_local() {
        let (_, id, _, bo) = joined_host().await;
        let before = write_own(&bo, "Use tabs.\n");
        bo.apply
            .keep_local(&bo.engine.paths, &id, group::CLAUDE_MD)
            .unwrap();
        let done = bo
            .engine
            .share_claude_md(&id, &share("Use tabs.\n", true))
            .await
            .unwrap();
        assert!(done.appended && !done.removed_own);
        assert_eq!(read_own(&bo), before);
        assert!(backups(&bo).is_empty());
    }
}
