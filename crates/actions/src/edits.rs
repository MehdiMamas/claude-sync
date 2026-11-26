//! Local edits to delivered files and sharing (Parity 39, 40, 41, 42). A sync pass never
//! pushes (v1: only shared paths go up), so an edited file stays on disk as a local edit
//! until the person keeps it, undoes it (a backup is kept) or shares it.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{ConflictView, EditView, Event};
use crate::msg;
use claude_sync_engine::claude_md::strip;
use claude_sync_engine::paths::is_planned_path;
use claude_sync_engine::plan::FileSet;
use claude_sync_engine::secrets::{scan, ALLOW_MARKER};
use claude_sync_engine::traits::{Disk, Remote, Store};
use claude_sync_fs::exec::utc_stamp;
use claude_sync_fs::{FsDisk, Scope};
use claude_sync_store::{blob_sha, BaseRec};
use std::collections::BTreeMap;

fn kept_key(repo: &str) -> String {
    format!("kept-edits/{repo}")
}

impl Core {
    fn disk(&self, repo: &str) -> FsDisk {
        FsDisk::new(&self.home, Scope::Group(repo.into()))
    }

    fn base(&self, repo: &str) -> Result<BaseRec> {
        self.db
            .load_base(repo, &self.home.state)?
            .ok_or_else(|| Error::new(msg::not_a_group(repo)))
    }

    /// path to the git blob sha of the version the person keeps.
    fn kept(&self, repo: &str) -> BTreeMap<String, String> {
        Store::read(&self.db, &kept_key(repo))
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn set_kept(&self, repo: &str, k: &BTreeMap<String, String>) {
        if let Ok(b) = serde_json::to_vec(k) {
            let _ = Store::write(&self.db, &kept_key(repo), &b);
        }
    }

    /// Base files whose copy on disk differs or is gone: (path, local bytes).
    fn local_edits(&self, repo: &str) -> Result<Vec<(String, Option<Vec<u8>>)>> {
        let base = self.base(repo)?;
        let disk = self.disk(repo);
        let mut out = vec![];
        for (p, b) in &base.files {
            let local = disk.read(p).ok().flatten();
            if local.as_ref() != Some(b) {
                out.push((p.clone(), local));
            }
        }
        Ok(out)
    }

    pub(crate) fn edits_list(&self, repo: &str) -> Result<Event> {
        let kept = self.kept(repo);
        Ok(Event::Edits(
            self.local_edits(repo)?
                .into_iter()
                .map(|(path, b)| EditView {
                    kept: kept.get(&path) == b.as_deref().map(blob_sha).as_ref(),
                    path,
                })
                .collect(),
        ))
    }

    fn edit_of(&self, repo: &str, path: &str) -> Result<Option<Vec<u8>>> {
        self.local_edits(repo)?
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b)
            .ok_or_else(|| Error::new(msg::not_an_edit(path)))
    }

    pub(crate) fn keep_mine(&mut self, repo: &str, path: &str) -> Result<Event> {
        let mine = self.edit_of(repo, path)?;
        let mut k = self.kept(repo);
        k.insert(
            path.into(),
            mine.as_deref().map(blob_sha).unwrap_or_default(),
        );
        self.set_kept(repo, &k);
        Ok(Event::Done)
    }

    /// The group's version comes back; mine is saved under `<state>/backups` first.
    pub(crate) fn undo_edit(&mut self, repo: &str, path: &str) -> Result<Event> {
        let mine = self.edit_of(repo, path)?;
        let base = self.base(repo)?;
        let theirs = base
            .files
            .get(path)
            .ok_or_else(|| Error::new(msg::not_an_edit(path)))?;
        if let Some(m) = mine {
            let to = self
                .home
                .state
                .join("backups")
                .join(Scope::Group(repo.into()).dir())
                .join(utc_stamp(self.now()))
                .join(path);
            claude_sync_fs::atomic::write_atomic(&to, &m)?;
        }
        self.disk(repo).write(path, theirs)?;
        let mut k = self.kept(repo);
        if k.remove(path).is_some() {
            self.set_kept(repo, &k);
        }
        Ok(Event::Done)
    }

    /// One commit with `files` on the group's main; the base learns them so they are not
    /// edits any more. A secret in any file stops it before anything is sent.
    fn push_files(&self, repo: &str, files: FileSet, what: &str) -> Result<Event> {
        for (p, b) in &files {
            let text = String::from_utf8_lossy(b);
            if !text.contains(ALLOW_MARKER) {
                if let Some(h) = scan(&text).first() {
                    return Err(Error::new(msg::secret_found(p, h.line, h.pattern)));
                }
            }
        }
        let me = self.me()?;
        let remote = self.remote(repo)?;
        let head = remote
            .head(None)?
            .ok_or_else(|| Error::new(msg::not_a_group(repo)))?;
        let pushes = files
            .iter()
            .map(|(p, b)| (p.clone(), Some(b.clone())))
            .collect();
        let sha = remote.push(
            &head.sha,
            &pushes,
            &format!("{me}@{}: share {what}", self.env.device),
        )?;
        let mut base = self.base(repo)?;
        base.files
            .extend(files.into_iter().filter(|(p, _)| is_planned_path(p)));
        base.head_sha = sha;
        base.etag = None;
        self.db.save_base(repo, &base, self.now())?;
        Ok(Event::Done)
    }

    pub(crate) fn share_edit(&mut self, repo: &str, path: &str) -> Result<Event> {
        let mine = self
            .edit_of(repo, path)?
            .ok_or_else(|| Error::new(msg::no_files_to_share(path)))?;
        self.push_files(repo, [(path.to_string(), mine)].into(), path)
    }

    pub(crate) fn share_skill(&mut self, repo: &str, name: &str) -> Result<Event> {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            || name.starts_with('.')
        {
            return Err(Error::new(msg::not_a_skill_name(name)));
        }
        let flags = crate::groups::cached(&self.db, repo)
            .map(|f| f.sync)
            .unwrap_or_default();
        if !flags.skills {
            return Err(Error::new(msg::does_not_sync(repo, "skills")));
        }
        let dir = self.home.claude.join("skills").join(name);
        if !dir.join("SKILL.md").is_file() {
            return Err(Error::new(msg::no_skill_md(name)));
        }
        if self
            .base(repo)?
            .files
            .contains_key(&format!("skills/{name}/SKILL.md"))
        {
            return Err(Error::new(msg::skill_exists(repo, name)));
        }
        let mut files = FileSet::new();
        let mut stack = vec![(dir, format!("skills/{name}"))];
        while let Some((d, rel)) = stack.pop() {
            for e in std::fs::read_dir(&d)?.flatten() {
                let t = e.file_type()?;
                let (p, r) = (
                    e.path(),
                    format!("{rel}/{}", e.file_name().to_string_lossy()),
                );
                if t.is_dir() {
                    stack.push((p, r));
                } else if t.is_file() {
                    files.insert(r, std::fs::read(p)?);
                }
            }
        }
        if files.is_empty() {
            return Err(Error::new(msg::no_files_to_share(name)));
        }
        self.push_files(repo, files, &format!("skills/{name}"))
    }

    /// The person's own CLAUDE.md text goes into the group's CLAUDE.md under `## <login>`.
    pub(crate) fn share_claude_md(&mut self, repo: &str) -> Result<Event> {
        let flags = crate::groups::cached(&self.db, repo)
            .map(|f| f.sync)
            .unwrap_or_default();
        if !flags.claude_md {
            return Err(Error::new(msg::does_not_sync(repo, "CLAUDE.md")));
        }
        let own = std::fs::read(self.home.claude.join("CLAUDE.md"))
            .ok()
            .and_then(|b| strip(&b))
            .ok_or_else(|| Error::new(msg::no_text_to_share()))?;
        let me = self.me()?;
        let remote = self.remote(repo)?;
        let head = remote
            .head(None)?
            .ok_or_else(|| Error::new(msg::not_a_group(repo)))?;
        let old = remote
            .files(&head.sha, &BTreeMap::new())?
            .remove("CLAUDE.md")
            .unwrap_or_default();
        let mut text = String::from_utf8_lossy(&old).into_owned();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("\n## {me}\n\n{}", String::from_utf8_lossy(&own)));
        self.push_files(
            repo,
            [("CLAUDE.md".to_string(), text.into_bytes())].into(),
            "CLAUDE.md",
        )
    }

    pub(crate) fn conflicts(&self) -> Result<Event> {
        Ok(Event::Conflicts(
            self.db
                .conflicts(50)?
                .into_iter()
                .map(|(repo, path, saved_at)| ConflictView {
                    repo,
                    path,
                    saved_at,
                })
                .collect(),
        ))
    }
}
