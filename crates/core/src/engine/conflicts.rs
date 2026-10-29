//! Rules that clash across the user's groups and their own `CLAUDE.md` (docs/architecture.md
//! §7b), and how the user settles each one on this device.
//!
//! The detection is `crate::conflicts`; this side reads what Claude reads here: each joined
//! group's `CLAUDE.md` copy (with the changes the user declined left out), the user's own
//! text and each group's `settings.shared.json` as last synced. Settling a line never pushes
//! for the other side: "keep this one" declines the other group's line for this user only,
//! with the per-person decline of §7a, or takes it out of the user's own text. Only "Edit"
//! of a group's line commits, and that change reaches the group's other members through
//! their review like any other.
use super::{read_base_file, read_group_file, Engine};
use crate::conflicts::{self, Conflict, SettingsFile, Source, Text};
use crate::remote::Changes;
use crate::state::{now_unix, AppState};
use crate::{group, Error, GroupId};

/// What the user's own text is called next to group names.
pub(crate) const OWN_NAME: &str = "Your CLAUDE.md";

impl Engine {
    /// Duplicate and contradicting `CLAUDE.md` lines across the user's groups and own text,
    /// then settings two groups set differently.
    pub fn conflicts(&self) -> crate::Result<Vec<Conflict>> {
        let mut out = conflicts::claude_md(&self.claude_md_texts()?);
        out.extend(conflicts::settings(&self.settings_files()?));
        out.truncate(conflicts::MAX);
        Ok(out)
    }

    /// How the lines of `text` would clash with what Claude reads on this device: shown
    /// before they are shared with or copied to `to`. `from` is where the text comes from
    /// (a group it is copied from, or `None` for the user's own text); that source is left
    /// out of the check.
    pub fn claude_md_check(
        &self,
        to: &GroupId,
        text: &str,
        from: Option<&GroupId>,
    ) -> crate::Result<Vec<Conflict>> {
        let source = match from {
            Some(id) => Source::Group { id: id.clone() },
            None => Source::Own,
        };
        let incoming = Text {
            name: self.source_name(&source),
            source: source.clone(),
            text: text.to_string(),
        };
        let mut texts: Vec<Text> = self
            .claude_md_texts()?
            .into_iter()
            .filter(|t| t.source != source)
            .collect();
        // The target's text as the group has it, so lines the user declined count too.
        let target = Source::Group { id: to.clone() };
        let group_text = read_base_file(&self.paths, to, group::CLAUDE_MD)?
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        if let Some(text) = group_text {
            texts.retain(|t| t.source != target);
            texts.insert(
                0,
                Text {
                    name: self.source_name(&target),
                    source: target,
                    text,
                },
            );
        }
        Ok(conflicts::incoming(&incoming, &texts))
    }

    /// Take `line` out of what Claude reads from `source`. For a group, the line is declined
    /// like a change under review (§7a) and saved in `choices`, so the person's other devices
    /// leave it out too; "Use the group's version" brings it back. For the user's own text,
    /// the file is backed up first.
    pub async fn drop_claude_md_line(&self, source: &Source, line: &str) -> crate::Result<()> {
        let _busy = self.busy.lock().await;
        match source {
            Source::Group { id } => {
                self.require_joined(id)?;
                self.drop_group_line(id, line)?;
                let line = line.trim().to_string();
                crate::choices::set_group(&self.paths, id, |c| {
                    if !c.dropped_lines.contains(&line) {
                        c.dropped_lines.push(line);
                    }
                })?;
                Ok(())
            }
            Source::Own => {
                let line = line.trim();
                let done = self.rewrite_own_claude_md(|mine| {
                    let at = mine.lines().position(|l| l.trim() == line)?;
                    let left: Vec<&str> = mine
                        .lines()
                        .enumerate()
                        .filter(|(i, _)| *i != at)
                        .map(|(_, l)| l)
                        .collect();
                    Some(text_or_none(left))
                })?;
                match done {
                    true => Ok(()),
                    false => Err(Error::Invalid(
                        "your CLAUDE.md no longer has that line".into(),
                    )),
                }
            }
        }
    }

    /// Decline `line` of the group's CLAUDE.md copy on this device. An error when the copy
    /// no longer has it.
    pub(crate) fn drop_group_line(&self, id: &GroupId, line: &str) -> crate::Result<()> {
        let mut st = self.require_review(id)?;
        let before = st.accepted.clone();
        let item = st.drop_line(line, now_unix()).ok_or_else(|| {
            Error::Invalid(format!(
                "{}'s CLAUDE.md on this device no longer has that line",
                self.source_name(&Source::Group { id: id.clone() })
            ))
        })?;
        self.write_accepted(id, &mut st, &before, &[item], false)
    }

    /// Replace `line` with `text` (several lines, or none to remove it). For a group this is
    /// committed to the group's `CLAUDE.md` and reaches its other members through review;
    /// for the user's own text, the file is backed up first.
    pub async fn edit_claude_md_line(
        &self,
        source: &Source,
        line: &str,
        text: &str,
    ) -> crate::Result<()> {
        let line = line.trim();
        let text = crate::secrets::redact(&text.replace("\r\n", "\n"));
        let text = text.trim_matches('\n');
        let _busy = self.busy.lock().await;
        match source {
            Source::Group { id } => {
                self.require_joined(id)?;
                let new = |old: &str| {
                    let lines: Vec<&str> = old.lines().collect();
                    let at = lines.iter().position(|l| l.trim() == line)?;
                    let mut out: Vec<&str> = lines[..at].to_vec();
                    if !text.is_empty() {
                        out.extend(text.lines());
                    }
                    out.extend(&lines[at + 1..]);
                    Some(format!("{}\n", out.join("\n")))
                };
                if !text.is_empty() {
                    self.scan(&Changes::from([(
                        group::CLAUDE_MD.to_string(),
                        Some(text.as_bytes().to_vec()),
                    )]))?;
                }
                self.commit_to_group(id, "edit CLAUDE.md", |gf, current| {
                    if !gf.sync.claude_md {
                        return Err(Error::Invalid(format!("{id} does not sync CLAUDE.md")));
                    }
                    let old = current
                        .get(group::CLAUDE_MD)
                        .map(|b| String::from_utf8_lossy(b).replace("\r\n", "\n"))
                        .unwrap_or_default();
                    let new = new(&old).ok_or_else(|| {
                        Error::Invalid(format!("{id}'s CLAUDE.md no longer has that line"))
                    })?;
                    Ok(Some(Changes::from([(
                        group::CLAUDE_MD.to_string(),
                        Some(new.into_bytes()),
                    )])))
                })
                .await?;
                match self.sync_group(id, None).await {
                    (_, Some(e)) => Err(e),
                    (_, None) => Ok(()),
                }
            }
            Source::Own => {
                let done = self.rewrite_own_claude_md(|mine| {
                    let lines: Vec<&str> = mine.lines().collect();
                    let at = lines.iter().position(|l| l.trim() == line)?;
                    let mut out: Vec<&str> = lines[..at].to_vec();
                    out.extend(text.lines());
                    out.extend(&lines[at + 1..]);
                    Some(text_or_none(out))
                })?;
                match done {
                    true => Ok(()),
                    false => Err(Error::Invalid(
                        "your CLAUDE.md no longer has that line".into(),
                    )),
                }
            }
        }
    }

    /// Each joined group's `CLAUDE.md` as Claude reads it here, in join order, then the
    /// user's own text.
    pub(super) fn claude_md_texts(&self) -> crate::Result<Vec<Text>> {
        let mut out = Vec::new();
        for id in AppState::load(&self.paths)?.groups {
            let copy = self
                .apply
                .local_files(&self.paths, &id)?
                .remove(group::CLAUDE_MD);
            if let Some(bytes) = copy {
                let source = Source::Group { id };
                out.push(Text {
                    name: self.source_name(&source),
                    source,
                    text: String::from_utf8_lossy(&bytes).into_owned(),
                });
            }
        }
        if let Some(text) = self.own_claude_md()? {
            out.push(Text {
                source: Source::Own,
                name: OWN_NAME.into(),
                text,
            });
        }
        Ok(out)
    }

    /// Each joined group's `settings.shared.json` as last synced, in join order. Groups that do not sync settings are left out.
    fn settings_files(&self) -> crate::Result<Vec<SettingsFile>> {
        let mut out = Vec::new();
        for id in AppState::load(&self.paths)?.groups {
            let base = self.load_base(&id)?;
            let syncs = base.files.contains_key(group::FILE)
                && read_group_file(&id, &base.files)
                    .map(|(gf, _)| gf.sync.settings)
                    .unwrap_or(false);
            if !syncs {
                continue;
            }
            let Some(json) = base
                .files
                .get(group::SHARED_SETTINGS)
                .and_then(|b| crate::settings::parse_object(b))
            else {
                continue;
            };
            out.push(SettingsFile {
                name: self.group_status(&id).name,
                id,
                json,
            });
        }
        Ok(out)
    }

    pub(super) fn source_name(&self, source: &Source) -> String {
        match source {
            Source::Group { id } => self.group_status(id).name,
            Source::Own => OWN_NAME.into(),
        }
    }
}

/// Lines joined back into a text, `None` when only blank lines are left.
fn text_or_none(lines: Vec<&str>) -> Option<String> {
    let text = lines.join("\n");
    (!text.trim().is_empty()).then(|| format!("{}\n", text.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conflicts::Kind;
    use crate::engine::tests::{device, group_repo, Device};
    use crate::remote::fake::{FakeGitHub, FakeRepo};

    /// `bo` in two groups: "Friends" (hosted by `bo`, though the repo id says ana; joined first) and
    /// cy's "Crew".
    async fn two_groups() -> (FakeGitHub, Device, [(GroupId, FakeRepo); 2]) {
        let world = FakeGitHub::new();
        let a = group_repo(&world, &["ana", "bo"]);
        let b = group_repo(&world, &["cy", "bo"]);
        let mut gf = group::GroupFile::parse(b.1.file(group::FILE).unwrap().as_bytes()).unwrap();
        gf.name = "Crew".into();
        b.1.set_file(group::FILE, &String::from_utf8(gf.to_bytes()).unwrap());
        a.1.set_file(
            group::CLAUDE_MD,
            "# Friends\n\n- Always use tabs.\n- Never push to main\n- Answer in French\n",
        );
        b.1.set_file(
            group::CLAUDE_MD,
            "# Crew\n\n- always use TABS\n- Never answer in French\n- Keep PRs small\n",
        );
        a.1.set_file(
            group::SHARED_SETTINGS,
            "{\"model\": \"opus\", \"env\": {\"A\": \"1\"}}\n",
        );
        b.1.set_file(
            group::SHARED_SETTINGS,
            "{\"model\": \"sonnet\", \"env\": {\"A\": \"1\"}}\n",
        );
        a.1.set_owner("bo");
        a.1.add_collaborator("ana");
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&a.0).await.unwrap();
        bo.engine.join(&b.0).await.unwrap();
        (world, bo, [a, b])
    }

    fn summary(cs: &[Conflict]) -> Vec<(Kind, String, String, String, String)> {
        cs.iter()
            .map(|c| {
                (
                    c.kind,
                    c.a.name.clone(),
                    c.pointer.clone().unwrap_or_else(|| c.a.text.clone()),
                    c.b.name.clone(),
                    c.b.text.clone(),
                )
            })
            .collect()
    }

    fn own(d: &Device, text: &str) {
        let home = &d.engine.paths.claude_home;
        std::fs::create_dir_all(home).unwrap();
        std::fs::write(home.join(group::CLAUDE_MD), text).unwrap();
    }

    #[tokio::test]
    async fn lists_clashes_across_groups_and_the_users_text() {
        let (_, bo, _) = two_groups().await;
        own(&bo, "# Me\nKeep PRs small.\n");
        let s = |x: &str| x.to_string();
        assert_eq!(
            summary(&bo.engine.conflicts().unwrap()),
            [
                (
                    Kind::Duplicate,
                    s("Friends"),
                    s("- Always use tabs."),
                    s("Crew"),
                    s("- always use TABS")
                ),
                (
                    Kind::Contradiction,
                    s("Friends"),
                    s("- Answer in French"),
                    s("Crew"),
                    s("- Never answer in French")
                ),
                (
                    Kind::Duplicate,
                    s("Crew"),
                    s("- Keep PRs small"),
                    s(OWN_NAME),
                    s("Keep PRs small.")
                ),
                (
                    Kind::Setting,
                    s("Friends"),
                    s("/model"),
                    s("Crew"),
                    s("\"sonnet\"")
                ),
            ]
        );
    }

    #[tokio::test]
    async fn keeping_one_side_declines_the_other_for_this_user_only() {
        let (_, bo, [_, (crew, crew_repo)]) = two_groups().await;
        let commits = crew_repo.commits().len();
        let crew_source = Source::Group { id: crew.clone() };
        bo.engine
            .drop_claude_md_line(&crew_source, "- Never answer in French")
            .await
            .unwrap();
        assert_eq!(
            bo.apply.file(&crew, group::CLAUDE_MD).unwrap(),
            "# Crew\n\n- always use TABS\n- Keep PRs small\n"
        );
        assert_eq!(crew_repo.commits().len(), commits, "nothing is pushed");
        assert_eq!(bo.engine.claude_md_review(&crew).unwrap().declined, 1);
        let kinds: Vec<Kind> = bo
            .engine
            .conflicts()
            .unwrap()
            .iter()
            .map(|c| c.kind)
            .collect();
        assert_eq!(kinds, [Kind::Duplicate, Kind::Setting]);

        // It stays out after the group changes, until the group's version is used again.
        crew_repo.set_file_as(
            "cy",
            group::CLAUDE_MD,
            "# Crew\n\n- always use TABS\n- Never answer in French\n- Keep PRs small\n- Be kind\n",
        );
        bo.engine
            .set_claude_md_review(&crew, Some(false))
            .await
            .unwrap();
        bo.engine.sync_all().await.unwrap();
        assert!(!bo
            .apply
            .file(&crew, group::CLAUDE_MD)
            .unwrap()
            .contains("French"));
        bo.engine.use_group_claude_md(&crew).await.unwrap();
        assert!(bo
            .apply
            .file(&crew, group::CLAUDE_MD)
            .unwrap()
            .contains("Never answer in French"));

        let err = bo
            .engine
            .drop_claude_md_line(&crew_source, "not there")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no longer has that line"), "{err}");
    }

    #[tokio::test]
    async fn the_users_own_line_can_go_or_change_with_a_backup() {
        let (_, bo, _) = two_groups().await;
        own(&bo, "# Me\nKeep PRs small.\nUse tabs\n");
        bo.engine
            .drop_claude_md_line(&Source::Own, "Keep PRs small.")
            .await
            .unwrap();
        bo.engine
            .edit_claude_md_line(&Source::Own, "Use tabs", "Use tabs in Go only")
            .await
            .unwrap();
        assert_eq!(
            bo.engine.own_claude_md().unwrap().as_deref(),
            Some("# Me\nUse tabs in Go only\n")
        );
        assert_eq!(
            std::fs::read_dir(bo.engine.paths.backups_dir())
                .unwrap()
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn editing_a_group_line_commits_it_for_everyone() {
        let (world, bo, [(friends, repo), _]) = two_groups().await;
        bo.engine
            .edit_claude_md_line(
                &Source::Group {
                    id: friends.clone(),
                },
                "- Answer in French",
                "- Answer in French in the fr/ folder only",
            )
            .await
            .unwrap();
        let text = "# Friends\n\n- Always use tabs.\n- Never push to main\n- Answer in French in the fr/ folder only\n";
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), text);
        assert_eq!(
            repo.commits().pop().unwrap().message,
            "bo@pc: edit CLAUDE.md"
        );
        // bo's own change applies at once; ana gets it to review.
        assert_eq!(bo.apply.file(&friends, group::CLAUDE_MD).unwrap(), text);
        let ana = device(&world, "ana", "mac");
        ana.engine.join(&friends).await.unwrap();
        assert_eq!(ana.apply.file(&friends, group::CLAUDE_MD).unwrap(), text);
    }

    #[tokio::test]
    async fn incoming_lines_are_checked_against_the_target_and_the_rest() {
        let (_, bo, [(friends, _), (crew, _)]) = two_groups().await;
        own(&bo, "Write tests first\n");
        let got = bo
            .engine
            .claude_md_check(
                &crew,
                "- Never use tabs\n- Write tests first\n- New rule here\n",
                Some(&friends),
            )
            .unwrap();
        let s = |x: &str| x.to_string();
        assert_eq!(
            summary(&got),
            [
                (
                    Kind::Contradiction,
                    s("Friends"),
                    s("- Never use tabs"),
                    s("Crew"),
                    s("- always use TABS")
                ),
                (
                    Kind::Duplicate,
                    s("Friends"),
                    s("- Write tests first"),
                    s(OWN_NAME),
                    s("Write tests first")
                ),
            ]
        );
    }
}
