//! "Copy to another group": content of one group the user is in, committed to another
//! (docs/architecture.md §7b). The same paths as sharing the user's own content: the
//! `CLAUDE.md` text is redacted and appended under a heading, skills and agents are
//! committed as files, and every change is scanned for secrets first. The target's other
//! members get the copied `CLAUDE.md` lines through their review (§7a), like any change.
//!
//! What is copied is the source group's version as last synced, not the user's local
//! edits of it. The user needs write access to the target; when GitHub says they can only
//! read it, the preview and the copy say so.
use super::claude_md::{appended, group_text};
use super::{read_group_file, Engine};
use crate::remote::Changes;
use crate::settings::{self, set_at};
use crate::{group, Error, FileSet, GroupId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What the target group already has of each thing that can be copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyPreview {
    pub to: GroupId,
    pub to_name: String,
    pub host: String,
    /// `false` when GitHub says the user can read the target but not push to it.
    pub can_push: bool,
    /// What the target syncs; the rest cannot be copied to it.
    pub syncs: CopySyncs,
    /// The source group's `CLAUDE.md`, secrets redacted. Empty when it has none.
    pub claude_md: String,
    pub skills: Vec<CopyFile>,
    pub agents: Vec<CopyFile>,
    pub settings: Vec<CopySetting>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopySyncs {
    pub claude_md: bool,
    pub skills: bool,
    pub agents: bool,
    pub settings: bool,
}

/// A skill (folder name) or agent (file name) of the source group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyFile {
    pub name: String,
    /// The target has one by that name.
    pub in_target: bool,
    /// ... and it is the same, so copying it changes nothing.
    pub same: bool,
}

/// One value of the source group's `settings.shared.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopySetting {
    pub pointer: String,
    /// As JSON.
    pub value: String,
    /// The target's value at the same pointer, as JSON.
    pub target: Option<String>,
    /// The target sets it to a different plain value: it cannot be copied over it.
    pub clash: bool,
}

/// What the user picked in the preview.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CopyRequest {
    /// The `CLAUDE.md` lines to append, as the user left them. Redacted again.
    pub claude_md: String,
    /// Skill folder names.
    pub skills: Vec<String>,
    /// Agent file names (`reviewer.md`).
    pub agents: Vec<String>,
    /// Setting pointers.
    pub settings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Copied {
    /// `false`: the target already had all of it, nothing was committed.
    pub committed: bool,
    pub claude_md: bool,
    pub files: usize,
    pub settings: usize,
}

fn skill_prefix(name: &str) -> String {
    format!("skills/{name}/")
}

fn agent_path(name: &str) -> String {
    format!("agents/{name}")
}

/// Skill folder names and agent file names in `files`.
fn skills_and_agents(files: &FileSet) -> (Vec<String>, Vec<String>) {
    let mut skills: Vec<String> = Vec::new();
    let mut agents = Vec::new();
    for path in files.keys() {
        let parts: Vec<&str> = path.split('/').collect();
        match parts.as_slice() {
            ["skills", name, _, ..] if !skills.iter().any(|s| s == name) => {
                skills.push(name.to_string())
            }
            ["agents", name] => agents.push(name.to_string()),
            _ => {}
        }
    }
    (skills, agents)
}

/// The files of one skill or agent in `files`.
fn item_files(files: &FileSet, skill: Option<&str>, agent: Option<&str>) -> FileSet {
    files
        .iter()
        .filter(|(p, _)| match (skill, agent) {
            (Some(s), _) => p.starts_with(&skill_prefix(s)),
            (_, Some(a)) => **p == agent_path(a),
            _ => false,
        })
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}

fn settings_of(files: &FileSet) -> Option<Value> {
    files
        .get(group::SHARED_SETTINGS)
        .and_then(|b| settings::parse_object(b))
}

impl Engine {
    /// What copying from `from` to `to` would bring, against the target as last synced.
    pub async fn copy_preview(&self, from: &GroupId, to: &GroupId) -> crate::Result<CopyPreview> {
        self.require_copy(from, to)?;
        let source = self.load_base(from)?.files;
        let target = self.load_base(to)?.files;
        let (gf, read_only) = read_group_file(to, &target)?;
        let status = self.group_status(to);
        // Only the host's app writes to a group.
        let can_push = read_only.is_none()
            && status.is_host
            && self.remotes.open(to).can_push().await.unwrap_or(true);
        let claude_md = source
            .get(group::CLAUDE_MD)
            .and_then(|b| group_text(&String::from_utf8_lossy(b)))
            .unwrap_or_default();
        let file = |skill: Option<&str>, agent: Option<&str>, name: &str| {
            let mine = item_files(&source, skill, agent);
            let theirs = item_files(&target, skill, agent);
            CopyFile {
                name: name.to_string(),
                in_target: !theirs.is_empty(),
                same: mine == theirs,
            }
        };
        let (skills, agents) = skills_and_agents(&source);
        let theirs = settings_of(&target);
        let settings = settings_of(&source)
            .map(|v| crate::conflicts::values(&v))
            .unwrap_or_default()
            .into_iter()
            .map(|(pointer, value)| {
                let target = theirs.as_ref().and_then(|t| t.pointer(&pointer));
                CopySetting {
                    clash: target.is_some_and(|t| clashes(t, &value)),
                    target: target.map(Value::to_string),
                    value: value.to_string(),
                    pointer,
                }
            })
            .collect();
        Ok(CopyPreview {
            to: to.clone(),
            to_name: status.name,
            host: status.host,
            can_push,
            syncs: CopySyncs {
                claude_md: gf.sync.claude_md,
                skills: gf.sync.skills,
                agents: gf.sync.agents,
                settings: gf.sync.settings,
            },
            claude_md,
            skills: skills.iter().map(|s| file(Some(s), None, s)).collect(),
            agents: agents.iter().map(|a| file(None, Some(a), a)).collect(),
            settings,
        })
    }

    /// Commit what the user picked from `from` to `to`: the `CLAUDE.md` text appended under
    /// "From <source group>", skills and agents as files, settings merged in. A skill or
    /// agent the target has in another version, or a setting it sets differently, is
    /// refused rather than overwritten.
    pub async fn copy_to_group(
        &self,
        from: &GroupId,
        to: &GroupId,
        req: &CopyRequest,
    ) -> crate::Result<Copied> {
        let _busy = self.busy.lock().await;
        self.require_copy(from, to)?;
        let source = self.load_base(from)?.files;
        let from_name = self.group_status(from).name;
        let text = group_text(&req.claude_md);

        let mut files = FileSet::new();
        let mut items = Vec::new();
        for s in &req.skills {
            let mine = item_files(&source, Some(s), None);
            if s.contains('/') || mine.is_empty() {
                return Err(Error::Invalid(format!(
                    "{from_name} has no skill named {s}"
                )));
            }
            files.extend(mine);
            items.push(format!("skills/{s}"));
        }
        for a in &req.agents {
            let mine = item_files(&source, None, Some(a));
            if a.contains('/') || mine.is_empty() {
                return Err(Error::Invalid(format!(
                    "{from_name} has no agent named {a}"
                )));
            }
            files.extend(mine);
            items.push(agent_path(a));
        }
        let source_settings = settings_of(&source).unwrap_or(Value::Null);
        let mut values = Vec::new();
        for ptr in &req.settings {
            let v = source_settings.pointer(ptr).ok_or_else(|| {
                Error::Invalid(format!(
                    "{from_name} does not set {}",
                    settings::display(ptr)
                ))
            })?;
            values.push((ptr.clone(), v.clone()));
        }
        if text.is_none() && files.is_empty() && values.is_empty() {
            return Err(Error::Invalid("nothing was picked to copy".into()));
        }
        let mut scanned: Changes = files
            .iter()
            .map(|(p, b)| (p.clone(), Some(b.clone())))
            .collect();
        if let Some(t) = &text {
            scanned.insert(group::CLAUDE_MD.into(), Some(t.clone().into_bytes()));
        }
        self.scan(&scanned)?;

        let mut what = Vec::new();
        if text.is_some() {
            what.push(group::CLAUDE_MD.to_string());
        }
        what.extend(items);
        if !values.is_empty() {
            what.push(format!("{} settings", values.len()));
        }
        let summary = format!("copy from {from_name}: {}", what.join(", "));
        let heading = format!("From {from_name}");
        let mut copied = Copied::default();
        let committed = self
            .commit_to_group(to, &summary, |gf, current| {
                copied = Copied::default();
                let name = &gf.name;
                let refuse =
                    |what: &str| Err(Error::Invalid(format!("{name} does not sync {what}")));
                let mut changes = Changes::new();
                if let Some(text) = &text {
                    if !gf.sync.claude_md {
                        return refuse("CLAUDE.md");
                    }
                    if let Some(new) = appended(current, text, &heading) {
                        changes.insert(group::CLAUDE_MD.into(), Some(new));
                        copied.claude_md = true;
                    }
                }
                for s in &req.skills {
                    if !gf.sync.skills {
                        return refuse("skills");
                    }
                    let theirs = item_files(current, Some(s), None);
                    let mine = item_files(&files, Some(s), None);
                    if theirs == mine {
                        continue;
                    }
                    if !theirs.is_empty() {
                        return Err(Error::Invalid(format!(
                            "{name} already has a different skill named {s}"
                        )));
                    }
                    copied.files += mine.len();
                    changes.extend(mine.into_iter().map(|(p, b)| (p, Some(b))));
                }
                for a in &req.agents {
                    if !gf.sync.agents {
                        return refuse("agents");
                    }
                    let path = agent_path(a);
                    match (current.get(&path), files.get(&path)) {
                        (Some(theirs), Some(mine)) if theirs == mine => continue,
                        (Some(_), _) => {
                            return Err(Error::Invalid(format!(
                                "{name} already has a different agent named {a}"
                            )))
                        }
                        (None, mine) => {
                            copied.files += 1;
                            changes.insert(path, mine.cloned());
                        }
                    }
                }
                if !values.is_empty() {
                    if !gf.sync.settings {
                        return refuse("settings");
                    }
                    let mut target =
                        settings_of(current).unwrap_or_else(|| Value::Object(Default::default()));
                    for (ptr, v) in &values {
                        let new = match target.pointer(ptr) {
                            Some(t) if t == v => continue,
                            Some(Value::Array(t)) if v.is_array() => {
                                let mut all = t.clone();
                                for x in v.as_array().into_iter().flatten() {
                                    if !all.contains(x) {
                                        all.push(x.clone());
                                    }
                                }
                                if all.len() == t.len() {
                                    continue;
                                }
                                Value::Array(all)
                            }
                            Some(t) if clashes(t, v) => {
                                return Err(Error::Invalid(format!(
                                    "{name} already sets {} to {t}",
                                    settings::display(ptr)
                                )))
                            }
                            _ => v.clone(),
                        };
                        if !set_at(&mut target, ptr, new) {
                            return Err(Error::Invalid(format!(
                                "{name} has a value that is not an object above {}",
                                settings::display(ptr)
                            )));
                        }
                        copied.settings += 1;
                    }
                    if copied.settings > 0 {
                        let bytes = settings::to_bytes(&target);
                        self.scan(&Changes::from([(
                            group::SHARED_SETTINGS.to_string(),
                            Some(bytes.clone()),
                        )]))?;
                        changes.insert(group::SHARED_SETTINGS.into(), Some(bytes));
                    }
                }
                Ok((!changes.is_empty()).then_some(changes))
            })
            .await?;
        copied.committed = committed;
        if committed {
            if let (_, Some(e)) = self.sync_group(to, None).await {
                return Err(e);
            }
        }
        Ok(copied)
    }

    fn require_copy(&self, from: &GroupId, to: &GroupId) -> crate::Result<()> {
        if from == to {
            return Err(Error::Invalid("pick another group to copy to".into()));
        }
        self.require_joined(from)?;
        self.require_joined(to)
    }
}

/// `target` already holds a different value that copying `value` would overwrite. Arrays
/// are merged, objects are walked into one value at a time.
fn clashes(target: &Value, value: &Value) -> bool {
    target != value && !(target.is_array() && value.is_array()) && !target.is_object()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{device, group_repo, Device};
    use crate::remote::fake::{FakeGitHub, FakeRepo};

    const TOKEN: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

    /// `bo` in ana's "Friends" and hosts "Crew" (the repo id keeps cy's name); `dee` is in Crew too.
    async fn two_groups() -> (FakeGitHub, Device, [(GroupId, FakeRepo); 2]) {
        let world = FakeGitHub::new();
        let a = group_repo(&world, &["ana", "bo"]);
        let b = group_repo(&world, &["cy", "bo", "dee"]);
        let mut gf = group::GroupFile::parse(b.1.file(group::FILE).unwrap().as_bytes()).unwrap();
        gf.name = "Crew".into();
        b.1.set_file(group::FILE, &String::from_utf8(gf.to_bytes()).unwrap());
        a.1.set_file(
            group::CLAUDE_MD,
            &format!("# Friends\n\n- Use tabs\n- Token {TOKEN}\n"),
        );
        a.1.set_file("skills/lint/SKILL.md", "lint\n");
        a.1.set_file("skills/lint/run.sh", "run\n");
        a.1.set_file("agents/reviewer.md", "review\n");
        a.1.set_file(
            group::SHARED_SETTINGS,
            "{\"model\": \"opus\", \"permissions\": {\"allow\": [\"Bash(ls)\"]}, \"a\": 2}\n",
        );
        b.1.set_file(
            group::SHARED_SETTINGS,
            "{\"a\": 1, \"permissions\": {\"allow\": [\"Read\"]}}\n",
        );
        b.1.set_owner("bo");
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&a.0).await.unwrap();
        bo.engine.join(&b.0).await.unwrap();
        (world, bo, [a, b])
    }

    #[tokio::test]
    async fn the_preview_shows_what_the_target_has() {
        let (_, bo, [(friends, _), (crew, _)]) = two_groups().await;
        let p = bo.engine.copy_preview(&friends, &crew).await.unwrap();
        assert_eq!(
            (p.to_name.as_str(), p.host.as_str(), p.can_push),
            ("Crew", "bo", true)
        );
        assert_eq!(
            p.claude_md,
            "# Friends\n\n- Use tabs\n- Token [redacted:ghp]\n"
        );
        let names = |f: &[CopyFile]| -> Vec<(String, bool, bool)> {
            f.iter()
                .map(|f| (f.name.clone(), f.in_target, f.same))
                .collect()
        };
        assert_eq!(names(&p.skills), [("lint".into(), false, false)]);
        assert_eq!(
            names(&p.agents),
            [
                ("a.md".into(), true, true),
                ("reviewer.md".into(), false, false)
            ]
        );
        let settings: Vec<(&str, Option<&str>, bool)> = p
            .settings
            .iter()
            .map(|s| (s.pointer.as_str(), s.target.as_deref(), s.clash))
            .collect();
        assert_eq!(
            settings,
            [
                ("/model", None, false),
                ("/permissions/allow", Some("[\"Read\"]"), false),
                ("/a", Some("1"), true),
            ]
        );
        assert!(bo.engine.copy_preview(&crew, &crew).await.is_err());
    }

    #[tokio::test]
    async fn copied_content_reaches_the_target_and_its_members_review_it() {
        let (world, bo, [(friends, _), (crew, crew_repo)]) = two_groups().await;
        let dee = device(&world, "dee", "mac");
        dee.engine.join(&crew).await.unwrap();
        let req = CopyRequest {
            claude_md: format!("- Use tabs\n- Token {TOKEN}\n"),
            skills: vec!["lint".into()],
            agents: vec!["reviewer.md".into(), "a.md".into()],
            settings: vec!["/model".into(), "/permissions/allow".into()],
        };
        let copied = bo
            .engine
            .copy_to_group(&friends, &crew, &req)
            .await
            .unwrap();
        assert_eq!(
            copied,
            Copied {
                committed: true,
                claude_md: true,
                files: 3,
                settings: 2
            }
        );
        let text = "# Friends\n\n## From Friends\n\n- Use tabs\n- Token [redacted:ghp]\n";
        assert_eq!(crew_repo.file(group::CLAUDE_MD).unwrap(), text);
        assert_eq!(crew_repo.file("skills/lint/run.sh").unwrap(), "run\n");
        assert_eq!(crew_repo.file("agents/reviewer.md").unwrap(), "review\n");
        assert_eq!(
            crew_repo.file(group::SHARED_SETTINGS).unwrap(),
            "{\n  \"a\": 1,\n  \"permissions\": {\n    \"allow\": [\n      \"Read\",\n      \"Bash(ls)\"\n    ]\n  },\n  \"model\": \"opus\"\n}\n"
        );
        assert_eq!(
            crew_repo.commits().pop().unwrap().message,
            "bo@pc: copy from Friends: CLAUDE.md, skills/lint, agents/reviewer.md, agents/a.md, 2 settings"
        );
        assert_eq!(bo.apply.file(&crew, group::CLAUDE_MD).unwrap(), text);

        // dee reviews the copied lines like any other change.
        dee.engine.sync_all().await.unwrap();
        let review = dee.engine.claude_md_review(&crew).unwrap();
        assert_eq!(review.pending.len(), 1);
        assert_eq!(
            dee.apply.file(&crew, group::CLAUDE_MD).unwrap(),
            "# Friends\n"
        );

        // Again: nothing new.
        let again = bo
            .engine
            .copy_to_group(&friends, &crew, &req)
            .await
            .unwrap();
        assert!(!again.committed);
    }

    #[tokio::test]
    async fn a_different_value_or_file_is_refused_not_overwritten() {
        let (_, bo, [(friends, _), (crew, crew_repo)]) = two_groups().await;
        let commits = crew_repo.commits().len();
        let err = |req: CopyRequest| {
            let e = &bo.engine;
            let (f, c) = (&friends, &crew);
            async move { e.copy_to_group(f, c, &req).await.unwrap_err().to_string() }
        };
        let setting = err(CopyRequest {
            settings: vec!["/a".into()],
            ..Default::default()
        })
        .await;
        assert!(setting.contains("Crew already sets a to 1"), "{setting}");
        crew_repo.set_file("agents/reviewer.md", "theirs\n");
        let agent = err(CopyRequest {
            agents: vec!["reviewer.md".into()],
            ..Default::default()
        })
        .await;
        assert!(agent.contains("already has a different agent"), "{agent}");
        let none = err(CopyRequest::default()).await;
        assert!(none.contains("nothing was picked"), "{none}");
        assert_eq!(
            crew_repo.commits().len(),
            commits + 1,
            "only the agent set up above"
        );
    }

    #[tokio::test]
    async fn a_reader_is_told_plainly_they_cannot_push() {
        let request = CopyRequest {
            claude_md: "- Be kind to each other
"
            .into(),
            ..Default::default()
        };

        // A member is told only the host changes the group.
        let world = FakeGitHub::new();
        let (friends, _) = group_repo(&world, &["ana", "bo"]);
        let (crew, crew_repo) = group_repo(&world, &["cy"]);
        crew_repo.add_reader("bo");
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&friends).await.unwrap();
        bo.engine.join(&crew).await.unwrap();
        let p = bo.engine.copy_preview(&friends, &crew).await.unwrap();
        assert!(!p.can_push);
        let e = bo
            .engine
            .copy_to_group(&friends, &crew, &request)
            .await
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "Only cy can change Friends. Your change stays on this machine; ask cy, or propose it as a pull request."
        );

        // A host whose GitHub account can only read the repo is told plainly.
        let world = FakeGitHub::new();
        let (friends, _) = group_repo(&world, &["ana", "bo"]);
        let (crew, crew_repo) = group_repo(&world, &["cy"]);
        crew_repo.set_owner("bo");
        crew_repo.add_reader("bo");
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&friends).await.unwrap();
        bo.engine.join(&crew).await.unwrap();
        let p = bo.engine.copy_preview(&friends, &crew).await.unwrap();
        assert!(!p.can_push);
        let e = bo
            .engine
            .copy_to_group(&friends, &crew, &request)
            .await
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "You can't push to Friends: your GitHub account can read it but not write to it. Ask its host (bo) for write access."
        );
    }
}
