//! The facade the app uses. Everything the UI shows comes from here.
//!
//! Sync of one group (§6): remote head -> remote files -> three-way merge against the base
//! snapshot and the local files read back by the apply layer -> apply -> push the local
//! changes the user chose to share (secret scan first, retried on push races) -> new base ->
//! membership reconciliation when this user is the host.
//!
//! Local edits to managed files are pushed only after **Share** (§7). Until then they stay
//! local: the engine neither pushes nor overwrites them, unless the remote changed the same
//! file, which is a conflict (remote wins, the local copy is saved).
use crate::apply::{ApplyReport, Collision};
use crate::github::{GitHub, Invitation};
use crate::group::{self, GroupFile};
use crate::merge;
use crate::remote::{Changes, Collaborator, GitHubRemotes, Head, RemoteFactory, RemoteRepo};
use crate::state::{now_unix, utc_stamp, write_atomic, write_json, AppState};
use crate::watch::{EditAction, LocalEdit};
use crate::{Error, FileSet, GroupId, Paths};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupStatus {
    pub id: GroupId,
    pub name: String,
    pub host: String,
    pub members: Vec<String>,
    /// Pending invitations (only known on the host's device).
    pub invited: Vec<String>,
    pub last_sync_unix: Option<u64>,
    pub last_error: Option<String>,
    pub conflicts: Vec<ConflictInfo>,
    pub collisions: Vec<crate::apply::Collision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictInfo {
    pub path: String,
    /// Where the losing local copy was saved. Empty when the local side had deleted the file.
    pub saved_to: String,
    pub at_unix: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub groups: Vec<GroupSyncReport>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSyncReport {
    pub id: GroupId,
    pub pulled: bool,
    pub pushed: bool,
    pub apply: crate::apply::ApplyReport,
    pub conflicts: Vec<ConflictInfo>,
    pub error: Option<String>,
    /// Access to the repo is gone; its content was removed from this machine.
    pub lost_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub at_unix: u64,
}

/// Retries after someone else pushed first (§6).
const PUSH_RETRIES: usize = 3;
/// Conflicts remembered per group for the UI.
const MAX_CONFLICTS: usize = 50;

/// What the engine needs from the apply layer and the watcher. `FsApply` is the real thing;
/// tests plug in their own.
pub trait ApplyLayer: Send + Sync {
    fn local_files(&self, paths: &Paths, group: &GroupId) -> crate::Result<FileSet>;
    fn apply_group(
        &self,
        paths: &Paths,
        group: &GroupId,
        order: &[GroupId],
        files: &FileSet,
    ) -> crate::Result<ApplyReport>;
    fn remove_group(&self, paths: &Paths, group: &GroupId) -> crate::Result<ApplyReport>;
    fn keep_local(&self, paths: &Paths, group: &GroupId, path: &str) -> crate::Result<()>;
    /// Put the group's version (`None`: the group does not have it) of `path` back.
    fn undo(
        &self,
        paths: &Paths,
        group: &GroupId,
        path: &str,
        group_version: Option<&[u8]>,
    ) -> crate::Result<()>;
    fn local_edits(&self, paths: &Paths) -> crate::Result<Vec<LocalEdit>>;
    /// Paths the group ships that this device does not manage (kept local, collisions,
    /// unsafe). The engine uses the base version as their local side.
    fn excluded_paths(&self, _paths: &Paths, _group: &GroupId) -> crate::Result<BTreeSet<String>> {
        Ok(BTreeSet::new())
    }
}

/// `crate::apply` and `crate::watch` on the real `~/.claude`.
pub struct FsApply;

impl ApplyLayer for FsApply {
    fn local_files(&self, paths: &Paths, group: &GroupId) -> crate::Result<FileSet> {
        crate::apply::local_files(paths, group)
    }
    fn apply_group(
        &self,
        paths: &Paths,
        group: &GroupId,
        order: &[GroupId],
        files: &FileSet,
    ) -> crate::Result<ApplyReport> {
        crate::apply::apply_group(paths, group, order, files)
    }
    fn remove_group(&self, paths: &Paths, group: &GroupId) -> crate::Result<ApplyReport> {
        crate::apply::remove_group(paths, group)
    }
    fn keep_local(&self, paths: &Paths, group: &GroupId, path: &str) -> crate::Result<()> {
        crate::apply::keep_local(paths, group, path)
    }
    fn undo(
        &self,
        paths: &Paths,
        group: &GroupId,
        path: &str,
        group_version: Option<&[u8]>,
    ) -> crate::Result<()> {
        crate::apply::undo(paths, group, path, group_version)
    }
    fn local_edits(&self, paths: &Paths) -> crate::Result<Vec<LocalEdit>> {
        crate::watch::local_edits(paths)
    }
    fn excluded_paths(&self, paths: &Paths, group: &GroupId) -> crate::Result<BTreeSet<String>> {
        crate::apply::excluded_paths(paths, group)
    }
}

/// The secret scan run on every file before it is committed (§9).
pub type Scanner = fn(&str) -> Vec<crate::secrets::Hit>;

pub struct Engine {
    pub paths: Paths,
    pub gh: GitHub,
    pub login: String,
    pub device: String,
    remotes: Arc<dyn RemoteFactory>,
    apply: Arc<dyn ApplyLayer>,
    scanner: Scanner,
    /// One operation at a time: syncs and user actions share the base snapshots.
    busy: tokio::sync::Mutex<()>,
}

/// Last synced state of a group (§6): the remote head and its allowlisted files.
#[derive(Debug, Clone, PartialEq)]
struct Base {
    head: Head,
    files: FileSet,
}

impl Default for Base {
    fn default() -> Self {
        Self {
            head: Head::empty(),
            files: FileSet::new(),
        }
    }
}

/// `groups/<owner>__<repo>/status.json`: what `groups()` shows between syncs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Status {
    name: String,
    host: String,
    members: Vec<String>,
    invited: Vec<String>,
    last_sync_unix: Option<u64>,
    last_error: Option<String>,
    conflicts: Vec<ConflictInfo>,
    collisions: Vec<StoredCollision>,
    /// Paths whose local edit the user chose to share; pushed by the next sync.
    shared: BTreeSet<String>,
}

/// `apply::Collision` is not `Deserialize`; this mirrors it on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
enum StoredCollision {
    UserFile { path: String },
    OtherGroup { path: String, owner: GroupId },
}

impl StoredCollision {
    fn path(&self) -> &str {
        match self {
            Self::UserFile { path } | Self::OtherGroup { path, .. } => path,
        }
    }
}

impl From<&Collision> for StoredCollision {
    fn from(c: &Collision) -> Self {
        match c.clone() {
            Collision::UserFile { path } => Self::UserFile { path },
            Collision::OtherGroup { path, owner } => Self::OtherGroup { path, owner },
        }
    }
}

impl From<StoredCollision> for Collision {
    fn from(c: StoredCollision) -> Self {
        match c {
            StoredCollision::UserFile { path } => Self::UserFile { path },
            StoredCollision::OtherGroup { path, owner } => Self::OtherGroup { path, owner },
        }
    }
}

impl Engine {
    pub fn new(paths: Paths, gh: GitHub, login: String, device: String) -> Self {
        let remotes = Arc::new(GitHubRemotes::new(gh.clone()));
        Self::with_remotes(paths, gh, login, device, remotes)
    }

    /// An engine whose repos come from `remotes` (for example `remote::fake::FakeAccount`).
    pub fn with_remotes(
        paths: Paths,
        gh: GitHub,
        login: String,
        device: String,
        remotes: Arc<dyn RemoteFactory>,
    ) -> Self {
        Self {
            paths,
            gh,
            login,
            device,
            remotes,
            apply: Arc::new(FsApply),
            scanner: crate::secrets::scan,
            busy: tokio::sync::Mutex::new(()),
        }
    }

    pub fn with_apply(mut self, apply: Arc<dyn ApplyLayer>) -> Self {
        self.apply = apply;
        self
    }

    pub fn with_scanner(mut self, scanner: Scanner) -> Self {
        self.scanner = scanner;
        self
    }

    /// One pass over every joined group: pull, merge, apply, push local changes, reconcile members
    /// if this user is the host, detect lost access. Errors per group go in the report, not `Err`.
    pub async fn sync_all(&self) -> crate::Result<SyncReport> {
        let _busy = self.busy.lock().await;
        let mut out = SyncReport::default();
        for id in AppState::load(&self.paths)?.groups {
            out.groups.push(self.sync_group(&id, None).await.0);
        }
        Ok(out)
    }

    /// Same, but only for groups whose remote head changed (ETag) or that have local edits to push.
    /// The 60 s loop calls this.
    pub async fn sync_if_changed(&self) -> crate::Result<SyncReport> {
        let _busy = self.busy.lock().await;
        let state = AppState::load(&self.paths)?;
        let mut out = SyncReport::default();
        if state.paused {
            return Ok(out);
        }
        for id in &state.groups {
            let head = match self.remotes.open(id).head_if_changed().await {
                Ok(Some(head)) => Some(head),
                Ok(None) => {
                    // Unchanged remote: only shared edits waiting to be pushed, or a failed
                    // last pass (whose remote change was already seen), need a sync.
                    let status = self.load_status(id);
                    if status.shared.is_empty() && status.last_error.is_none() {
                        continue;
                    }
                    None
                }
                // `sync_group` asks again and reports the error (or the lost access).
                Err(_) => None,
            };
            out.groups.push(self.sync_group(id, head).await.0);
        }
        Ok(out)
    }

    pub async fn groups(&self) -> crate::Result<Vec<GroupStatus>> {
        let state = AppState::load(&self.paths)?;
        Ok(state
            .groups
            .iter()
            .map(|id| self.group_status(id))
            .collect())
    }

    /// Create a private group repo (default name `claude-sync-data`) and join it.
    pub async fn create_group(&self, name: &str, repo: &str) -> crate::Result<GroupStatus> {
        let _busy = self.busy.lock().await;
        let repo = match repo.trim() {
            "" => group::DEFAULT_REPO,
            r => r,
        };
        let name = match name.trim() {
            "" => repo,
            n => n,
        };
        let id = self.remotes.create_repo(repo).await?;
        let remote = self.remotes.open(&id);
        if !remote.is_private().await? {
            return Err(Error::PublicRepo(id.to_string()));
        }
        let head = remote.head().await?;
        let files = Changes::from([
            (
                group::FILE.to_string(),
                Some(GroupFile::new(name, &self.login).to_bytes()),
            ),
            (
                group::README.to_string(),
                Some(group::readme(name, &self.login).into_bytes()),
            ),
            (
                group::CLAUDE_MD.to_string(),
                Some(format!("# {name}\n").into_bytes()),
            ),
            (group::SHARED_SETTINGS.to_string(), Some(b"{}\n".to_vec())),
        ]);
        remote
            .commit(&head, &files, &self.message("create group"))
            .await?;
        self.join_locked(&id).await
    }

    /// Pending invitations to repos with the `claude-sync-group` topic.
    pub async fn invitations(&self) -> crate::Result<Vec<Invitation>> {
        self.remotes.invitations().await
    }

    /// Accept and set the group up on this device.
    pub async fn accept_invitation(&self, invitation_id: u64) -> crate::Result<GroupStatus> {
        let _busy = self.busy.lock().await;
        let id = self.remotes.accept_invitation(invitation_id).await?;
        self.join_locked(&id).await
    }

    /// Join a group repo this user already has access to.
    pub async fn join(&self, id: &GroupId) -> crate::Result<GroupStatus> {
        let _busy = self.busy.lock().await;
        self.join_locked(id).await
    }

    /// Group repos this user can access but has not joined on this device.
    pub async fn discover(&self) -> crate::Result<Vec<GroupId>> {
        let joined = AppState::load(&self.paths)?.groups;
        let mut found = self.remotes.group_repos().await?;
        found.retain(|id| !joined.contains(id));
        Ok(found)
    }

    pub async fn add_member(&self, id: &GroupId, login: &str) -> crate::Result<()> {
        if !valid_login(login) {
            return Err(Error::Invalid(format!("{login:?} is not a GitHub login")));
        }
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        let summary = format!("add member {login}");
        if self
            .edit_group_file(id, &summary, |gf| Ok(gf.add_member(login)))
            .await?
        {
            self.sync_group(id, None).await;
        }
        Ok(())
    }

    pub async fn remove_member(&self, id: &GroupId, login: &str) -> crate::Result<()> {
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        let host = self.remotes.open(id).owner_login().await?;
        if host.eq_ignore_ascii_case(login) {
            return Err(Error::Invalid(format!(
                "{login} hosts {id} and cannot be removed"
            )));
        }
        let summary = format!("remove member {login}");
        if self
            .edit_group_file(id, &summary, |gf| Ok(gf.remove_member(login)))
            .await?
        {
            self.sync_group(id, None).await;
        }
        Ok(())
    }

    /// Leave on this device: remove managed content. Does not change membership.
    pub async fn leave(&self, id: &GroupId) -> crate::Result<()> {
        let _busy = self.busy.lock().await;
        self.forget(id).map(drop)
    }

    pub fn local_edits(&self) -> crate::Result<Vec<LocalEdit>> {
        self.apply.local_edits(&self.paths)
    }

    pub async fn resolve_edit(&self, edit: &LocalEdit, action: EditAction) -> crate::Result<()> {
        let _busy = self.busy.lock().await;
        let id = &edit.group;
        self.require_joined(id)?;
        let mut status = self.load_status(id);
        // A settings value (`settings.shared.json#/env/X`) is shared with its file.
        let file = edit.path.split_once('#').map_or(&*edit.path, |(f, _)| f);
        let whole_file = file == edit.path;
        match action {
            EditAction::Share => {
                status.shared.insert(file.to_string());
                self.save_status(id, &status)?;
                match self.sync_group(id, None).await {
                    (_, Some(e)) => Err(e),
                    (_, None) => Ok(()),
                }
            }
            EditAction::KeepLocal => {
                self.apply.keep_local(&self.paths, id, &edit.path)?;
                if whole_file {
                    status.shared.remove(file);
                }
                self.save_status(id, &status)
            }
            EditAction::Undo => {
                if whole_file {
                    status.shared.remove(file);
                }
                self.save_status(id, &status)?;
                let base = self.load_base(id)?;
                let version = base.files.get(file).map(Vec::as_slice);
                self.apply.undo(&self.paths, id, &edit.path, version)
            }
        }
    }

    /// Share a local skill folder (`~/.claude/skills/<name>`) with a group.
    pub async fn share_skill(&self, id: &GroupId, skill: &str) -> crate::Result<()> {
        let prefix = format!("skills/{skill}/");
        if skill.contains('/') || !group::is_allowlisted(&format!("{prefix}SKILL.md")) {
            return Err(Error::Invalid(format!("{skill:?} is not a skill name")));
        }
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        let dir = self.paths.claude_home.join("skills").join(skill);
        let files: Changes = read_tree(&dir)?
            .into_iter()
            .map(|(rel, bytes)| (format!("{prefix}{rel}"), Some(bytes)))
            .filter(|(p, _)| group::is_allowlisted(p))
            .collect();
        if files.is_empty() {
            return Err(Error::Invalid(format!(
                "{} has no files to share",
                dir.display()
            )));
        }
        self.scan(&files)?;
        let remote = self.remotes.open(id);
        let message = self.message(&format!("share skills/{skill}"));
        let mut attempt = 0;
        loop {
            let head = remote.head().await?;
            let current = remote.files_at(&head.commit).await?;
            let (gf, read_only) = read_group_file(id, &current)?;
            if let Some(e) = read_only {
                return Err(e);
            }
            if !gf.sync.skills {
                return Err(Error::Invalid(format!("{id} does not sync skills")));
            }
            if current.keys().any(|p| p.starts_with(&prefix)) {
                return Err(Error::Invalid(format!(
                    "{id} already has a skill named {skill}"
                )));
            }
            match remote.commit(&head, &files, &message).await {
                Ok(_) => break,
                Err(Error::PushRace) if attempt < PUSH_RETRIES => attempt += 1,
                Err(e) => return Err(e),
            }
        }
        // The group's copy replaces the user's folder, which is kept as a backup.
        let backup = self
            .paths
            .backups_dir()
            .join(utc_stamp(now_unix()))
            .join("skills")
            .join(skill);
        std::fs::create_dir_all(backup.parent().expect("has a parent"))?;
        std::fs::rename(&dir, &backup)?;
        match self.sync_group(id, None).await {
            (_, Some(e)) => Err(e),
            (_, None) => Ok(()),
        }
    }

    pub async fn recent_commits(
        &self,
        id: &GroupId,
        limit: usize,
    ) -> crate::Result<Vec<CommitInfo>> {
        self.remotes.open(id).recent_commits(limit).await
    }

    // ---- internals ----

    fn message(&self, summary: &str) -> String {
        format!("{}@{}: {summary}", self.login, self.device)
    }

    fn require_joined(&self, id: &GroupId) -> crate::Result<()> {
        if AppState::load(&self.paths)?.groups.contains(id) {
            Ok(())
        } else {
            Err(Error::Invalid(format!("{id} is not joined on this device")))
        }
    }

    async fn join_locked(&self, id: &GroupId) -> crate::Result<GroupStatus> {
        let remote = self.remotes.open(id);
        if !remote.is_private().await? {
            return Err(Error::PublicRepo(id.to_string()));
        }
        let head = remote.head().await?;
        let files = remote.files_at(&head.commit).await?;
        if !files.contains_key(group::FILE) {
            return Err(Error::Invalid(format!(
                "{id} is not a claude-sync group (no {})",
                group::FILE
            )));
        }
        let mut state = AppState::load(&self.paths)?;
        if !state.groups.contains(id) {
            state.groups.push(id.clone());
            state.save(&self.paths)?;
        }
        // Errors of the first sync show in the status, like any later one.
        self.sync_group(id, Some(head)).await;
        Ok(self.group_status(id))
    }

    /// Change `claude-sync.json` and commit it, retrying on push races. `false` when `edit`
    /// changed nothing.
    async fn edit_group_file(
        &self,
        id: &GroupId,
        summary: &str,
        edit: impl Fn(&mut GroupFile) -> crate::Result<bool>,
    ) -> crate::Result<bool> {
        let remote = self.remotes.open(id);
        let message = self.message(summary);
        let mut attempt = 0;
        loop {
            let head = remote.head().await?;
            let files = remote.files_at(&head.commit).await?;
            let bytes = files
                .get(group::FILE)
                .ok_or_else(|| Error::Invalid(format!("{id} has no {}", group::FILE)))?;
            let mut gf = GroupFile::parse(bytes)?;
            if !edit(&mut gf)? {
                return Ok(false);
            }
            let changes = Changes::from([(group::FILE.to_string(), Some(gf.to_bytes()))]);
            match remote.commit(&head, &changes, &message).await {
                Ok(_) => return Ok(true),
                Err(Error::PushRace) if attempt < PUSH_RETRIES => attempt += 1,
                Err(e) => return Err(e),
            }
        }
    }

    /// Sync one group and record the outcome in its status. The error is also returned so user
    /// actions can surface it.
    async fn sync_group(
        &self,
        id: &GroupId,
        head: Option<Head>,
    ) -> (GroupSyncReport, Option<Error>) {
        let mut report = GroupSyncReport {
            id: id.clone(),
            pulled: false,
            pushed: false,
            apply: ApplyReport::default(),
            conflicts: Vec::new(),
            error: None,
            lost_access: false,
        };
        let remote = self.remotes.open(id);
        let head = match head {
            Some(h) => Ok(h),
            None => remote.head().await,
        };
        let mut status = self.load_status(id);
        let result = match head {
            Err(e) if is_lost_access(&e) => return self.lose(id, report, e),
            Err(e) => Err(e),
            Ok(head) => {
                self.pull_push(remote.as_ref(), id, head, &mut status, &mut report)
                    .await
            }
        };
        status.conflicts.extend(report.conflicts.iter().cloned());
        let excess = status.conflicts.len().saturating_sub(MAX_CONFLICTS);
        status.conflicts.drain(..excess);
        match &result {
            Ok(()) => {
                status.last_sync_unix = Some(now_unix());
                status.last_error = None;
            }
            Err(e) => {
                // A secret blocks the push until the user fixes the file and shares again.
                if let Error::SecretFound { path, .. } = e {
                    status.shared.remove(path);
                }
                tracing::warn!(group = %id, error = %e, "sync failed");
                status.last_error = Some(e.to_string());
                report.error = Some(e.to_string());
            }
        }
        if let Err(e) = self.save_status(id, &status) {
            tracing::warn!(group = %id, error = %e, "could not save the group status");
        }
        (report, result.err())
    }

    async fn pull_push(
        &self,
        remote: &dyn RemoteRepo,
        id: &GroupId,
        mut head: Head,
        status: &mut Status,
        report: &mut GroupSyncReport,
    ) -> crate::Result<()> {
        let order = AppState::load(&self.paths)?.groups;
        let mut base = self.load_base(id)?;
        let mut attempt = 0;
        let (gf, read_only) = loop {
            let remote_files = if head == base.head {
                base.files.clone()
            } else {
                report.pulled = true;
                remote.files_at(&head.commit).await?
            };
            let (gf, read_only) = read_group_file(id, &remote_files)?;
            let keep = |p: &str| group::is_allowlisted(p) && gf.syncs(p);
            let base_v = only(&base.files, keep);
            let remote_v = only(&remote_files, keep);
            let local = self.local_view(id, &base_v, status, keep)?;
            let merged = merge::three_way(&base_v, &local, &remote_v);

            // Save losing local copies before the apply overwrites them.
            report
                .conflicts
                .extend(self.save_conflicts(id, &merged.conflicts)?);
            let mut to_apply = merged.merged;
            to_apply.remove(group::FILE);
            // A local edit that is not being shared stays a local edit: the apply gets the
            // group's version, so it leaves the edit in place instead of adopting it.
            for p in merged.to_push.keys() {
                if read_only.is_none() && status.shared.contains(p) {
                    continue;
                }
                match remote_v.get(p) {
                    Some(bytes) => to_apply.insert(p.clone(), bytes.clone()),
                    None => to_apply.remove(p),
                };
            }
            report.apply = self.apply.apply_group(&self.paths, id, &order, &to_apply)?;
            status.collisions = report.apply.collisions.iter().map(Into::into).collect();
            base = Base {
                head: head.clone(),
                files: remote_files,
            };
            self.save_base(id, &base)?;

            status.shared.retain(|p| merged.to_push.contains_key(p));
            let push: Changes = merged
                .to_push
                .into_iter()
                .filter(|(p, _)| read_only.is_none() && status.shared.contains(p))
                .collect();
            if push.is_empty() {
                break (gf, read_only);
            }
            self.scan(&push)?;
            match remote
                .commit(&head, &push, &self.message(&summarize(&push)))
                .await
            {
                Ok(new) => {
                    base = Base {
                        head: new,
                        files: with_changes(base.files, &push),
                    };
                    self.save_base(id, &base)?;
                    status.shared.retain(|p| !push.contains_key(p));
                    report.pushed = true;
                    break (gf, read_only);
                }
                Err(Error::PushRace) if attempt < PUSH_RETRIES => {
                    attempt += 1;
                    head = remote.head().await?;
                }
                Err(e) => return Err(e),
            }
        };

        let host = remote.owner_login().await?;
        status.name = gf.name.clone();
        status.members = gf.members.clone();
        status.host = host.clone();
        if let Some(e) = read_only {
            return Err(e);
        }
        if host.eq_ignore_ascii_case(&self.login) {
            status.invited = self.reconcile(remote, &gf, &host).await?;
        } else {
            status.invited.clear();
        }
        Ok(())
    }

    /// The local side of the merge. The apply layer reads managed files back; the engine
    /// supplies the rest from the base: the group file (repo only), paths the apply layer did
    /// not write (collisions) and paths missing locally that the user did not choose to delete
    /// (kept local, or not readable back).
    fn local_view(
        &self,
        id: &GroupId,
        base: &FileSet,
        status: &Status,
        keep: impl Fn(&str) -> bool,
    ) -> crate::Result<FileSet> {
        let mut local: FileSet = self
            .apply
            .local_files(&self.paths, id)?
            .into_iter()
            .filter(|(p, _)| keep(p) && !group::is_repo_only(p))
            .collect();
        let mut held = self.apply.excluded_paths(&self.paths, id)?;
        held.extend(status.collisions.iter().map(|c| c.path().to_string()));
        let paths: BTreeSet<String> = base.keys().chain(&held).cloned().collect();
        for p in paths {
            let unshared_delete = !local.contains_key(&p) && !status.shared.contains(&p);
            if p == group::FILE || held.contains(&p) || unshared_delete {
                match base.get(&p) {
                    Some(bytes) => local.insert(p, bytes.clone()),
                    None => local.remove(&p),
                };
            }
        }
        Ok(local)
    }

    fn scan(&self, changes: &Changes) -> crate::Result<()> {
        for (path, content) in changes {
            let Some(bytes) = content else { continue };
            if let Some(hit) = (self.scanner)(&String::from_utf8_lossy(bytes))
                .into_iter()
                .next()
            {
                return Err(Error::SecretFound {
                    path: path.clone(),
                    line: hit.line,
                    pattern: hit.pattern.into(),
                });
            }
        }
        Ok(())
    }

    /// Host only: make collaborators match `members` (§5). Every step is tried; the first
    /// failure is returned. Returns the logins with a pending invitation.
    async fn reconcile(
        &self,
        remote: &dyn RemoteRepo,
        gf: &GroupFile,
        host: &str,
    ) -> crate::Result<Vec<String>> {
        let current = remote.collaborators().await?;
        let plan = crate::members::plan(&gf.members, host, &current);
        let mut failures = Vec::new();
        let mut invited: Vec<String> = current
            .iter()
            .filter_map(|c| match c {
                Collaborator::Invited {
                    login,
                    invitation_id,
                } if !plan.cancel_invitations.contains(invitation_id) => Some(login.clone()),
                _ => None,
            })
            .collect();
        for login in &plan.invite {
            match remote.invite(login).await {
                Ok(()) => invited.push(login.clone()),
                Err(e) => failures.push(format!("could not invite {login}: {e}")),
            }
        }
        for login in &plan.remove {
            if let Err(e) = remote.remove_collaborator(login).await {
                failures.push(format!("could not remove {login}: {e}"));
            }
        }
        for inv in &plan.cancel_invitations {
            if let Err(e) = remote.cancel_invitation(*inv).await {
                failures.push(format!("could not cancel invitation {inv}: {e}"));
            }
        }
        match failures.into_iter().next() {
            Some(first) => Err(Error::Invalid(first)),
            None => Ok(invited),
        }
    }

    fn save_conflicts(
        &self,
        id: &GroupId,
        conflicts: &[merge::Conflict],
    ) -> crate::Result<Vec<ConflictInfo>> {
        let at_unix = now_unix();
        let dir = self.paths.conflicts_dir(id).join(utc_stamp(at_unix));
        conflicts
            .iter()
            .map(|c| {
                let saved_to = match &c.local {
                    Some(bytes) => {
                        let path = join_rel(&dir, &c.path);
                        write_atomic(&path, bytes)?;
                        path.display().to_string()
                    }
                    None => String::new(),
                };
                Ok(ConflictInfo {
                    path: c.path.clone(),
                    saved_to,
                    at_unix,
                })
            })
            .collect()
    }

    /// Access is gone: remove the group from this machine.
    fn lose(
        &self,
        id: &GroupId,
        mut report: GroupSyncReport,
        cause: Error,
    ) -> (GroupSyncReport, Option<Error>) {
        tracing::warn!(group = %id, error = %cause, "lost access");
        report.lost_access = true;
        report.error = Some(match self.forget(id) {
            Ok(applied) => {
                report.apply = applied;
                format!("access to {id} is gone; its files were removed from this machine")
            }
            Err(e) => format!("access to {id} is gone, but removing its files failed: {e}"),
        });
        (report, Some(cause))
    }

    /// Remove managed content, the group's state and its entry in `state.json`.
    fn forget(&self, id: &GroupId) -> crate::Result<ApplyReport> {
        let applied = self.apply.remove_group(&self.paths, id)?;
        let mut state = AppState::load(&self.paths)?;
        state.groups.retain(|g| g != id);
        state.save(&self.paths)?;
        remove_dir_if_exists(&self.paths.group_dir(id))?;
        Ok(applied)
    }

    fn status_file(&self, id: &GroupId) -> PathBuf {
        self.paths.group_dir(id).join("status.json")
    }

    fn load_status(&self, id: &GroupId) -> Status {
        std::fs::read(self.status_file(id))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_status(&self, id: &GroupId, status: &Status) -> crate::Result<()> {
        write_json(&self.status_file(id), status)
    }

    fn group_status(&self, id: &GroupId) -> GroupStatus {
        let s = self.load_status(id);
        let or = |v: String, fallback: &str| {
            if v.is_empty() {
                fallback.to_string()
            } else {
                v
            }
        };
        GroupStatus {
            id: id.clone(),
            name: or(s.name, &id.repo),
            host: or(s.host, &id.owner),
            members: s.members,
            invited: s.invited,
            last_sync_unix: s.last_sync_unix,
            last_error: s.last_error,
            conflicts: s.conflicts,
            collisions: s.collisions.into_iter().map(Into::into).collect(),
        }
    }

    /// `base/head.json` and `base/files/<path>`.
    fn load_base(&self, id: &GroupId) -> crate::Result<Base> {
        let dir = self.paths.base_dir(id);
        // `base.old` only survives a crash in the middle of `save_base`.
        let dir = [dir.clone(), dir.with_extension("old")]
            .into_iter()
            .find(|d| d.join("head.json").is_file());
        let Some(dir) = dir else {
            return Ok(Base::default());
        };
        let head: Head = serde_json::from_slice(&std::fs::read(dir.join("head.json"))?)?;
        let files = match read_tree(&dir.join("files")) {
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => FileSet::new(),
            r => r?,
        };
        Ok(Base { head, files })
    }

    /// Written next to the old snapshot, then swapped in.
    fn save_base(&self, id: &GroupId, base: &Base) -> crate::Result<()> {
        let dir = self.paths.base_dir(id);
        let new = dir.with_extension("new");
        let old = dir.with_extension("old");
        remove_dir_if_exists(&new)?;
        for (path, bytes) in &base.files {
            let file = join_rel(&new.join("files"), path);
            std::fs::create_dir_all(file.parent().expect("has a parent"))?;
            std::fs::write(file, bytes)?;
        }
        write_json(&new.join("head.json"), &base.head)?;
        remove_dir_if_exists(&old)?;
        if dir.exists() {
            std::fs::rename(&dir, &old)?;
        }
        std::fs::rename(&new, &dir)?;
        remove_dir_if_exists(&old)
    }
}

/// The group file of a snapshot, and `SchemaTooNew` when it is from a newer app: then the
/// group is read-only (pulled and applied, never pushed).
fn read_group_file(id: &GroupId, files: &FileSet) -> crate::Result<(GroupFile, Option<Error>)> {
    let bytes = files
        .get(group::FILE)
        .ok_or_else(|| Error::Invalid(format!("{id} has no {}", group::FILE)))?;
    match GroupFile::parse(bytes) {
        Ok(gf) => Ok((gf, None)),
        Err(e @ Error::SchemaTooNew(_)) => {
            let gf = serde_json::from_slice(bytes)
                .unwrap_or_else(|_| GroupFile::new(&id.repo, &id.owner));
            Ok((gf, Some(e)))
        }
        Err(e) => Err(e),
    }
}

fn is_lost_access(e: &Error) -> bool {
    matches!(
        e,
        Error::GitHub {
            status: 403 | 404,
            ..
        }
    )
}

fn only(files: &FileSet, keep: impl Fn(&str) -> bool) -> FileSet {
    files
        .iter()
        .filter(|(p, _)| keep(p))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}

fn with_changes(mut files: FileSet, changes: &Changes) -> FileSet {
    for (path, content) in changes {
        match content {
            Some(bytes) => files.insert(path.clone(), bytes.clone()),
            None => files.remove(path),
        };
    }
    files
}

/// `update CLAUDE.md, skills/review`, `remove agents/old.md` or `update 5 items`.
fn summarize(changes: &Changes) -> String {
    let unit = |p: &str| match p.split('/').collect::<Vec<_>>()[..] {
        ["skills", name, ..] => format!("skills/{name}"),
        _ => p.to_string(),
    };
    let updated: BTreeSet<String> = changes
        .iter()
        .filter(|(_, c)| c.is_some())
        .map(|(p, _)| unit(p))
        .collect();
    let removed: BTreeSet<String> = changes
        .iter()
        .filter(|(_, c)| c.is_none())
        .map(|(p, _)| unit(p))
        .filter(|u| !updated.contains(u))
        .collect();
    let count = updated.len() + removed.len();
    if count > 3 {
        return format!("update {count} items");
    }
    let list = |set: &BTreeSet<String>| set.iter().cloned().collect::<Vec<_>>().join(", ");
    match (updated.is_empty(), removed.is_empty()) {
        (false, true) => format!("update {}", list(&updated)),
        (true, false) => format!("remove {}", list(&removed)),
        _ => format!("update {}; remove {}", list(&updated), list(&removed)),
    }
}

/// GitHub logins: 1-39 letters, digits or inner single hyphens.
fn valid_login(login: &str) -> bool {
    (1..=39).contains(&login.len())
        && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !login.starts_with('-')
        && !login.ends_with('-')
        && !login.contains("--")
}

fn join_rel(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

/// Every regular file under `root`, keyed by its relative path with `/`. Symlinks are skipped.
fn read_tree(root: &Path) -> crate::Result<FileSet> {
    let mut out = FileSet::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file() {
                let rel = path.strip_prefix(root).expect("walked from root");
                let key: Vec<_> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect();
                out.insert(key.join("/"), std::fs::read(&path)?);
            }
        }
    }
    Ok(out)
}

fn remove_dir_if_exists(dir: &Path) -> crate::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::fake::{FakeGitHub, FakeRepo};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// `~/.claude` of one device, in memory. Paths in `collide` belong to the user and are never
    /// written; kept paths stop being managed. Like the real apply, a file edited locally is
    /// left alone while the group's version stays the one last applied.
    #[derive(Default)]
    struct StubApply {
        st: Mutex<StubState>,
    }

    #[derive(Default)]
    struct StubState {
        local: HashMap<GroupId, FileSet>,
        /// The group's version of each file as last applied.
        applied: HashMap<GroupId, FileSet>,
        kept: BTreeSet<String>,
        collide: BTreeSet<String>,
        removed: Vec<GroupId>,
    }

    impl StubApply {
        fn file(&self, id: &GroupId, path: &str) -> Option<String> {
            let st = self.st.lock().unwrap();
            let bytes = st.local.get(id)?.get(path)?;
            Some(String::from_utf8_lossy(bytes).into_owned())
        }
        fn paths(&self, id: &GroupId) -> Vec<String> {
            let st = self.st.lock().unwrap();
            st.local
                .get(id)
                .map(|f| f.keys().cloned().collect())
                .unwrap_or_default()
        }
        /// The user edits a managed file.
        fn edit(&self, id: &GroupId, path: &str, content: &str) {
            let mut st = self.st.lock().unwrap();
            let files = st.local.entry(id.clone()).or_default();
            files.insert(path.into(), content.as_bytes().to_vec());
        }
        fn delete(&self, id: &GroupId, path: &str) {
            let mut st = self.st.lock().unwrap();
            st.local.entry(id.clone()).or_default().remove(path);
        }
        fn collide(&self, path: &str) {
            self.st.lock().unwrap().collide.insert(path.into());
        }
    }

    impl ApplyLayer for StubApply {
        fn local_files(&self, _: &Paths, group: &GroupId) -> crate::Result<FileSet> {
            let st = self.st.lock().unwrap();
            let files = st.local.get(group).cloned().unwrap_or_default();
            Ok(files
                .into_iter()
                .filter(|(p, _)| !st.kept.contains(p) && !st.collide.contains(p))
                .collect())
        }
        fn apply_group(
            &self,
            _: &Paths,
            group: &GroupId,
            _: &[GroupId],
            files: &FileSet,
        ) -> crate::Result<ApplyReport> {
            let mut st = self.st.lock().unwrap();
            let mut report = ApplyReport::default();
            let old = st.local.remove(group).unwrap_or_default();
            let applied = st.applied.insert(group.clone(), files.clone()).unwrap_or_default();
            let mut new = FileSet::new();
            for (path, bytes) in files {
                if st.collide.contains(path) {
                    report
                        .collisions
                        .push(Collision::UserFile { path: path.clone() });
                } else if !st.kept.contains(path) {
                    if applied.get(path) == Some(bytes) && old.get(path) != Some(bytes) {
                        // Edited locally, the group's version did not change.
                        if let Some(mine) = old.get(path) {
                            new.insert(path.clone(), mine.clone());
                        }
                        continue;
                    }
                    if old.get(path) != Some(bytes) {
                        report.written.push(path.clone());
                    }
                    new.insert(path.clone(), bytes.clone());
                }
            }
            for (path, bytes) in old {
                if st.kept.contains(&path) {
                    new.insert(path, bytes);
                } else if !new.contains_key(&path) {
                    report.removed.push(path);
                }
            }
            st.local.insert(group.clone(), new);
            Ok(report)
        }
        fn remove_group(&self, _: &Paths, group: &GroupId) -> crate::Result<ApplyReport> {
            let mut st = self.st.lock().unwrap();
            st.removed.push(group.clone());
            let removed = st.local.remove(group).unwrap_or_default();
            Ok(ApplyReport {
                removed: removed.into_keys().collect(),
                ..Default::default()
            })
        }
        fn undo(
            &self,
            _: &Paths,
            group: &GroupId,
            path: &str,
            version: Option<&[u8]>,
        ) -> crate::Result<()> {
            let mut st = self.st.lock().unwrap();
            let files = st.local.entry(group.clone()).or_default();
            match version {
                Some(bytes) => files.insert(path.into(), bytes.to_vec()),
                None => files.remove(path),
            };
            Ok(())
        }
        fn keep_local(&self, _: &Paths, _: &GroupId, path: &str) -> crate::Result<()> {
            self.st.lock().unwrap().kept.insert(path.into());
            Ok(())
        }
        fn local_edits(&self, _: &Paths) -> crate::Result<Vec<LocalEdit>> {
            Ok(Vec::new())
        }
    }

    fn no_secrets(_: &str) -> Vec<crate::secrets::Hit> {
        Vec::new()
    }

    fn fake_scanner(text: &str) -> Vec<crate::secrets::Hit> {
        text.lines()
            .enumerate()
            .filter(|(_, l)| l.contains("ghp_"))
            .map(|(n, _)| crate::secrets::Hit {
                line: n + 1,
                pattern: "github-token",
            })
            .collect()
    }

    struct Device {
        engine: Engine,
        apply: Arc<StubApply>,
        _dir: tempfile::TempDir,
    }

    fn device(world: &FakeGitHub, login: &str, device: &str) -> Device {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            claude_home: dir.path().join("claude"),
            state_dir: dir.path().join("state"),
        };
        // Never called: every repo comes from the fake world.
        let gh = GitHub::with_api(String::new(), "http://127.0.0.1:9");
        let apply = Arc::new(StubApply::default());
        let engine = Engine::with_remotes(
            paths,
            gh,
            login.into(),
            device.into(),
            Arc::new(world.account(login)),
        )
        .with_apply(apply.clone())
        .with_scanner(no_secrets);
        Device {
            engine,
            apply,
            _dir: dir,
        }
    }

    fn group_json(name: &str, members: &[&str]) -> String {
        let mut gf = GroupFile::new(name, members[0]);
        gf.members = members.iter().map(|m| m.to_string()).collect();
        String::from_utf8(gf.to_bytes()).unwrap()
    }

    /// `ana/claude-sync-data` with `members` as collaborators and a few files.
    fn group_repo(world: &FakeGitHub, members: &[&str]) -> (GroupId, FakeRepo) {
        let id = GroupId::new(members[0], group::DEFAULT_REPO);
        let json = group_json("Friends", members);
        let repo = FakeRepo::with_files(
            id.clone(),
            &[
                (group::FILE, &json),
                (group::CLAUDE_MD, "# Friends\n"),
                (group::SHARED_SETTINGS, "{\"a\": 1}\n"),
                ("agents/a.md", "base\n"),
                (group::README, "readme\n"),
            ],
        );
        for m in &members[1..] {
            repo.add_collaborator(m);
        }
        world.add(&repo);
        (id, repo)
    }

    /// `bo` on `pc`, joined to ana's group with members ana and bo.
    async fn joined() -> (FakeGitHub, GroupId, FakeRepo, Device) {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&id).await.unwrap();
        (world, id, repo, bo)
    }

    fn edit(id: &GroupId, path: &str) -> LocalEdit {
        LocalEdit {
            group: id.clone(),
            path: path.into(),
            deleted: false,
        }
    }

    fn commit_count(repo: &FakeRepo) -> usize {
        repo.commits().len()
    }

    fn calls(repo: &FakeRepo, op: &str) -> usize {
        repo.calls().iter().filter(|c| *c == op).count()
    }

    #[tokio::test]
    async fn join_pulls_and_applies() {
        let (_, id, _, bo) = joined().await;
        assert_eq!(
            bo.apply.paths(&id),
            [group::CLAUDE_MD, "agents/a.md", group::SHARED_SETTINGS],
            "the group file and the README stay in the repo"
        );
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "base\n");
        let groups = bo.engine.groups().await.unwrap();
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!((g.name.as_str(), g.host.as_str()), ("Friends", "ana"));
        assert_eq!(g.members, ["ana", "bo"]);
        assert!(g.last_sync_unix.is_some());
        assert_eq!(g.last_error, None);
        assert!(bo.engine.paths.base_dir(&id).join("head.json").is_file());
    }

    #[tokio::test]
    async fn join_refuses_public_repos_and_non_groups() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        repo.set_private(false);
        let bo = device(&world, "bo", "pc");
        assert!(matches!(
            bo.engine.join(&id).await,
            Err(Error::PublicRepo(_))
        ));

        let other = FakeRepo::with_files(GroupId::new("ana", "notes"), &[("CLAUDE.md", "x")]);
        other.add_collaborator("bo");
        world.add(&other);
        let err = bo
            .engine
            .join(&GroupId::new("ana", "notes"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a claude-sync group"), "{err}");
        assert!(bo.engine.groups().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn local_edits_are_pushed_only_after_share() {
        let (_, id, repo, bo) = joined().await;
        let before = commit_count(&repo);
        bo.apply.edit(&id, "agents/a.md", "mine\n");

        let report = bo.engine.sync_all().await.unwrap();
        assert!(!report.groups[0].pushed);
        assert_eq!(commit_count(&repo), before);
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "mine\n", "kept");

        bo.engine
            .resolve_edit(&edit(&id, "agents/a.md"), EditAction::Share)
            .await
            .unwrap();
        assert_eq!(repo.file("agents/a.md").unwrap(), "mine\n");
        let last = repo.commits().pop().unwrap();
        assert_eq!(last.message, "bo@pc: update agents/a.md");
        assert_eq!(last.author, "bo");
        assert!(bo.engine.load_status(&id).shared.is_empty());
        // Nothing left to push.
        bo.engine.sync_all().await.unwrap();
        assert_eq!(commit_count(&repo), before + 1);
    }

    #[tokio::test]
    async fn remote_changes_fast_forward() {
        let (_, id, repo, bo) = joined().await;
        repo.set_file("agents/a.md", "v2\n");
        repo.set_file("commands/go.md", "go\n");
        let report = bo.engine.sync_if_changed().await.unwrap();
        assert!(report.groups[0].pulled);
        assert!(!report.groups[0].pushed);
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "v2\n");
        assert_eq!(bo.apply.file(&id, "commands/go.md").unwrap(), "go\n");
        assert_eq!(bo.engine.load_base(&id).unwrap().head, repo.current_head());
    }

    #[tokio::test]
    async fn both_sides_change_different_files() {
        let (_, id, repo, bo) = joined().await;
        repo.set_file("agents/a.md", "theirs\n");
        bo.apply.edit(&id, group::CLAUDE_MD, "# Ours\n");
        bo.engine
            .resolve_edit(&edit(&id, group::CLAUDE_MD), EditAction::Share)
            .await
            .unwrap();
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), "# Ours\n");
        assert_eq!(repo.file("agents/a.md").unwrap(), "theirs\n");
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "theirs\n");
        assert_eq!(
            repo.commits().pop().unwrap().message,
            "bo@pc: update CLAUDE.md"
        );
    }

    #[tokio::test]
    async fn conflicts_keep_the_remote_and_save_the_local_copy() {
        let (_, id, repo, bo) = joined().await;
        bo.apply.edit(&id, "agents/a.md", "mine\n");
        repo.set_file("agents/a.md", "theirs\n");
        let report = bo.engine.sync_all().await.unwrap();
        let conflicts = &report.groups[0].conflicts;
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].path, "agents/a.md");
        let saved = PathBuf::from(&conflicts[0].saved_to);
        assert!(saved.starts_with(bo.engine.paths.conflicts_dir(&id)));
        assert!(saved.ends_with(Path::new("agents").join("a.md")));
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "mine\n");
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "theirs\n");
        assert_eq!(repo.file("agents/a.md").unwrap(), "theirs\n");
        let g = &bo.engine.groups().await.unwrap()[0];
        assert_eq!(g.conflicts, *conflicts);
    }

    #[tokio::test]
    async fn shared_settings_merge_by_key() {
        let (_, id, repo, bo) = joined().await;
        bo.apply
            .edit(&id, group::SHARED_SETTINGS, "{\"a\": 1, \"b\": 2}\n");
        repo.set_file(group::SHARED_SETTINGS, "{\"a\": 1, \"c\": 3}\n");
        bo.engine
            // The watcher reports one settings value; its file is what gets shared.
            .resolve_edit(&edit(&id, "settings.shared.json#/b"), EditAction::Share)
            .await
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&repo.file(group::SHARED_SETTINGS).unwrap()).unwrap();
        assert_eq!(value, serde_json::json!({"a": 1, "b": 2, "c": 3}));
        let local: serde_json::Value =
            serde_json::from_str(&bo.apply.file(&id, group::SHARED_SETTINGS).unwrap()).unwrap();
        assert_eq!(local, value);
    }

    #[tokio::test]
    async fn remote_edit_beats_a_shared_local_delete() {
        let (_, id, repo, bo) = joined().await;
        bo.apply.delete(&id, "agents/a.md");
        repo.set_file("agents/a.md", "theirs\n");
        let mut deleted = edit(&id, "agents/a.md");
        deleted.deleted = true;
        bo.engine
            .resolve_edit(&deleted, EditAction::Share)
            .await
            .unwrap();
        assert_eq!(repo.file("agents/a.md").unwrap(), "theirs\n");
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "theirs\n");
        let g = &bo.engine.groups().await.unwrap()[0];
        assert_eq!(g.conflicts.len(), 1);
        assert_eq!(g.conflicts[0].saved_to, "", "nothing to save for a delete");
    }

    #[tokio::test]
    async fn shared_delete_is_pushed_and_unshared_delete_is_held() {
        let (_, id, repo, bo) = joined().await;
        bo.apply.delete(&id, "agents/a.md");
        bo.engine.sync_all().await.unwrap();
        assert!(repo.file("agents/a.md").is_some());
        assert_eq!(bo.apply.file(&id, "agents/a.md"), None, "still a local edit");

        let mut deleted = edit(&id, "agents/a.md");
        deleted.deleted = true;
        bo.engine
            .resolve_edit(&deleted, EditAction::Share)
            .await
            .unwrap();
        assert_eq!(repo.file("agents/a.md"), None);
        assert_eq!(
            repo.commits().pop().unwrap().message,
            "bo@pc: remove agents/a.md"
        );
    }

    #[tokio::test]
    async fn push_races_are_retried() {
        let (_, id, repo, bo) = joined().await;
        let raced = Arc::new(Mutex::new(false));
        let flag = raced.clone();
        repo.before_commit(move |r| {
            let mut done = flag.lock().unwrap();
            if !*done {
                *done = true;
                r.set_file("commands/x.md", "x\n");
            }
        });
        bo.apply.edit(&id, group::CLAUDE_MD, "# Ours\n");
        bo.engine
            .resolve_edit(&edit(&id, group::CLAUDE_MD), EditAction::Share)
            .await
            .unwrap();
        assert!(*raced.lock().unwrap());
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), "# Ours\n");
        assert_eq!(repo.file("commands/x.md").unwrap(), "x\n");
        assert_eq!(bo.apply.file(&id, "commands/x.md").unwrap(), "x\n");
    }

    #[tokio::test]
    async fn push_races_give_up_after_three_retries() {
        let (_, id, repo, bo) = joined().await;
        let n = Arc::new(Mutex::new(0));
        let count = n.clone();
        repo.before_commit(move |r| {
            let mut n = count.lock().unwrap();
            *n += 1;
            r.set_file("commands/x.md", &format!("{n}\n"));
        });
        bo.apply.edit(&id, group::CLAUDE_MD, "# Ours\n");
        let commits_before = calls(&repo, "commit");
        let err = bo
            .engine
            .resolve_edit(&edit(&id, group::CLAUDE_MD), EditAction::Share)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PushRace), "{err}");
        assert_eq!(calls(&repo, "commit") - commits_before, 1 + PUSH_RETRIES);
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), "# Friends\n");
        let status = bo.engine.load_status(&id);
        assert!(status.last_error.is_some());
        assert!(
            status.shared.contains(group::CLAUDE_MD),
            "pushed by the next sync"
        );

        repo.clear_before_commit();
        bo.engine.sync_if_changed().await.unwrap();
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), "# Ours\n");
        assert_eq!(bo.engine.load_status(&id).last_error, None);
    }

    #[tokio::test]
    async fn secrets_block_the_push() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let mut bo = device(&world, "bo", "pc");
        bo.engine = bo.engine.with_scanner(fake_scanner);
        bo.engine.join(&id).await.unwrap();
        let before = commit_count(&repo);
        bo.apply.edit(&id, "agents/a.md", "ok\ntoken ghp_abc\n");
        let err = bo
            .engine
            .resolve_edit(&edit(&id, "agents/a.md"), EditAction::Share)
            .await
            .unwrap_err();
        match err {
            Error::SecretFound {
                path,
                line,
                pattern,
            } => assert_eq!(
                (path.as_str(), line, pattern.as_str()),
                ("agents/a.md", 2, "github-token")
            ),
            e => panic!("{e}"),
        }
        assert_eq!(commit_count(&repo), before);
        let status = bo.engine.load_status(&id);
        assert!(status.shared.is_empty(), "the user must share again");
        assert!(status.last_error.unwrap().contains("secret"));
    }

    #[tokio::test]
    async fn the_host_reconciles_collaborators() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo", "cy"]);
        // `bo` is in, `cy` was never invited, `zed` is not a member, `old` has a stale invitation.
        repo.remove_collaborator("cy").await.unwrap();
        repo.add_collaborator("zed");
        repo.invite("old").await.unwrap();
        let ana = device(&world, "ana", "mac");
        let status = ana.engine.join(&id).await.unwrap();
        assert_eq!(status.last_error, None);
        assert_eq!(status.host, "ana");
        assert_eq!(status.invited, ["cy"]);
        assert_eq!(repo.collaborator_logins(), ["bo"]);
        let pending: Vec<String> = repo
            .pending_invitations()
            .into_iter()
            .map(|(_, l)| l)
            .collect();
        assert_eq!(pending, ["cy"]);

        // A pending invitation is never sent again.
        ana.engine.sync_all().await.unwrap();
        assert_eq!(calls(&repo, "invite"), 2, "old (by the test) and cy");
        assert_eq!(ana.engine.groups().await.unwrap()[0].invited, ["cy"]);

        let err = ana.engine.remove_member(&id, "ANA").await.unwrap_err();
        assert!(err.to_string().contains("cannot be removed"), "{err}");

        ana.engine.remove_member(&id, "bo").await.unwrap();
        assert!(repo.collaborator_logins().is_empty());
        let gf = GroupFile::parse(repo.file(group::FILE).unwrap().as_bytes()).unwrap();
        assert_eq!(gf.members, ["ana", "cy"]);
        assert_eq!(
            repo.commits().pop().unwrap().message,
            "ana@mac: remove member bo"
        );

        ana.engine.add_member(&id, "dee").await.unwrap();
        let pending: Vec<String> = repo
            .pending_invitations()
            .into_iter()
            .map(|(_, l)| l)
            .collect();
        assert_eq!(pending, ["cy", "dee"]);
    }

    #[tokio::test]
    async fn members_only_edit_the_group_file() {
        let (_, id, repo, bo) = joined().await;
        bo.engine.add_member(&id, "cy").await.unwrap();
        let gf = GroupFile::parse(repo.file(group::FILE).unwrap().as_bytes()).unwrap();
        assert_eq!(gf.members, ["ana", "bo", "cy"]);
        assert_eq!(
            repo.commits().pop().unwrap().message,
            "bo@pc: add member cy"
        );
        assert_eq!(calls(&repo, "invite"), 0, "only the host invites");
        assert_eq!(calls(&repo, "collaborators"), 0);
        let g = &bo.engine.groups().await.unwrap()[0];
        assert_eq!(g.members, ["ana", "bo", "cy"]);
        assert_eq!(g.last_error, None);

        // Adding twice changes nothing.
        let before = commit_count(&repo);
        bo.engine.add_member(&id, "CY").await.unwrap();
        assert_eq!(commit_count(&repo), before);

        for bad in ["", "-cy", "cy-", "c--y", "c y", "a/b", &"x".repeat(40)] {
            assert!(bo.engine.add_member(&id, bad).await.is_err(), "{bad:?}");
        }
        let other = GroupId::new("zed", "group");
        assert!(
            bo.engine.add_member(&other, "cy").await.is_err(),
            "not joined"
        );
    }

    #[tokio::test]
    async fn lost_access_removes_the_group() {
        let (_, id, repo, bo) = joined().await;
        repo.remove_collaborator("bo").await.unwrap();
        let report = bo.engine.sync_all().await.unwrap();
        let g = &report.groups[0];
        assert!(g.lost_access);
        assert!(g.error.as_ref().unwrap().contains("access"));
        assert_eq!(g.apply.removed.len(), 3);
        assert_eq!(
            bo.apply.st.lock().unwrap().removed,
            std::slice::from_ref(&id)
        );
        assert!(bo.engine.groups().await.unwrap().is_empty());
        assert!(!bo.engine.paths.group_dir(&id).exists());
    }

    #[tokio::test]
    async fn other_errors_keep_the_group() {
        let (_, id, repo, bo) = joined().await;
        repo.fail_times("head", 502, 1);
        let report = bo.engine.sync_all().await.unwrap();
        assert!(!report.groups[0].lost_access);
        assert!(report.groups[0].error.as_ref().unwrap().contains("502"));
        let g = &bo.engine.groups().await.unwrap()[0];
        assert!(g.last_error.is_some());
        assert_eq!(bo.apply.paths(&id).len(), 3);
        // The next pass retries even though the remote did not change.
        bo.engine.sync_if_changed().await.unwrap();
        assert_eq!(bo.engine.groups().await.unwrap()[0].last_error, None);
    }

    #[tokio::test]
    async fn accepting_an_invitation_joins() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana"]);
        repo.invite("bo").await.unwrap();
        let bo = device(&world, "bo", "pc");
        let invitations = bo.engine.invitations().await.unwrap();
        assert_eq!(invitations.len(), 1);
        assert_eq!(invitations[0].repo, id);
        let status = bo
            .engine
            .accept_invitation(invitations[0].id)
            .await
            .unwrap();
        assert_eq!(status.id, id);
        assert_eq!(repo.collaborator_logins(), ["bo"]);
        assert_eq!(bo.apply.file(&id, group::CLAUDE_MD).unwrap(), "# Friends\n");
    }

    #[tokio::test]
    async fn create_group_commits_the_skeleton_and_joins() {
        let world = FakeGitHub::new();
        let ana = device(&world, "ana", "mac");
        let status = ana.engine.create_group("Friends", "").await.unwrap();
        let id = GroupId::new("ana", group::DEFAULT_REPO);
        assert_eq!(status.id, id);
        assert_eq!(
            (status.name.as_str(), status.host.as_str()),
            ("Friends", "ana")
        );
        assert_eq!(status.members, ["ana"]);
        assert_eq!(status.last_error, None);
        let repo = world.repo(&id).unwrap();
        let commits = repo.commits();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message, "ana@mac: create group");
        let gf = GroupFile::parse(repo.file(group::FILE).unwrap().as_bytes()).unwrap();
        assert_eq!(gf, GroupFile::new("Friends", "ana"));
        assert!(repo.file(group::README).unwrap().contains("Friends"));
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), "# Friends\n");
        assert_eq!(repo.file(group::SHARED_SETTINGS).unwrap(), "{}\n");
        assert_eq!(
            ana.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends\n"
        );

        let err = ana.engine.create_group("Again", "").await.unwrap_err();
        assert!(matches!(err, Error::GitHub { status: 422, .. }), "{err}");
        let named = ana.engine.create_group("", "crew").await.unwrap();
        assert_eq!(named.name, "crew");
    }

    #[tokio::test]
    async fn sync_if_changed_skips_unchanged_and_paused() {
        let (_, id, repo, bo) = joined().await;
        let files_at = calls(&repo, "files_at");
        // The first look sees the head joined with: no download.
        bo.engine.sync_if_changed().await.unwrap();
        let report = bo.engine.sync_if_changed().await.unwrap();
        assert!(report.groups.is_empty());
        assert_eq!(calls(&repo, "files_at"), files_at);

        let mut state = AppState::load(&bo.engine.paths).unwrap();
        state.paused = true;
        state.save(&bo.engine.paths).unwrap();
        repo.set_file("agents/a.md", "v2\n");
        assert!(bo.engine.sync_if_changed().await.unwrap().groups.is_empty());
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "base\n");
        bo.engine.sync_all().await.unwrap();
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "v2\n");
    }

    #[tokio::test]
    async fn collisions_are_reported_and_never_pushed() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let bo = device(&world, "bo", "pc");
        bo.apply.collide("agents/a.md");
        bo.engine.join(&id).await.unwrap();
        let before = commit_count(&repo);
        bo.apply.edit(&id, group::CLAUDE_MD, "# Ours\n");
        bo.engine
            .resolve_edit(&edit(&id, group::CLAUDE_MD), EditAction::Share)
            .await
            .unwrap();
        assert_eq!(commit_count(&repo), before + 1);
        assert_eq!(repo.file("agents/a.md").unwrap(), "base\n", "not deleted");
        let g = &bo.engine.groups().await.unwrap()[0];
        assert!(matches!(
            &g.collisions[..],
            [Collision::UserFile { path }] if path == "agents/a.md"
        ));
    }

    #[tokio::test]
    async fn newer_schemas_are_read_only() {
        let world = FakeGitHub::new();
        let id = GroupId::new("ana", group::DEFAULT_REPO);
        let repo = FakeRepo::with_files(
            id.clone(),
            &[
                (
                    group::FILE,
                    "{\"schema\": 99, \"name\": \"Future\", \"members\": [\"ana\", \"bo\"]}",
                ),
                (group::CLAUDE_MD, "# Future\n"),
            ],
        );
        repo.add_collaborator("bo");
        world.add(&repo);
        let bo = device(&world, "bo", "pc");
        let status = bo.engine.join(&id).await.unwrap();
        assert!(status.last_error.unwrap().contains("newer"));
        assert_eq!(status.name, "Future");
        assert_eq!(bo.apply.file(&id, group::CLAUDE_MD).unwrap(), "# Future\n");

        bo.apply.edit(&id, group::CLAUDE_MD, "# Ours\n");
        let err = bo
            .engine
            .resolve_edit(&edit(&id, group::CLAUDE_MD), EditAction::Share)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::SchemaTooNew(99)));
        assert_eq!(commit_count(&repo), 1);
        assert!(bo.engine.add_member(&id, "cy").await.is_err());
    }

    #[tokio::test]
    async fn sync_flags_filter_paths() {
        let world = FakeGitHub::new();
        let id = GroupId::new("ana", group::DEFAULT_REPO);
        let mut gf = GroupFile::new("Friends", "ana");
        gf.members.push("bo".into());
        gf.sync.agents = false;
        let json = String::from_utf8(gf.to_bytes()).unwrap();
        let repo = FakeRepo::with_files(
            id.clone(),
            &[
                (group::FILE, &json),
                ("agents/a.md", "a\n"),
                ("commands/c.md", "c\n"),
            ],
        );
        repo.add_collaborator("bo");
        world.add(&repo);
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&id).await.unwrap();
        assert_eq!(bo.apply.paths(&id), ["commands/c.md"]);
    }

    #[tokio::test]
    async fn share_skill_moves_the_folder_into_the_group() {
        let (_, id, repo, bo) = joined().await;
        let dir = bo.engine.paths.claude_home.join("skills").join("review");
        std::fs::create_dir_all(dir.join("refs")).unwrap();
        std::fs::write(dir.join("SKILL.md"), "# Review\n").unwrap();
        std::fs::write(dir.join("refs").join("style.md"), "style\n").unwrap();
        std::fs::write(dir.join(".DS_Store"), "junk").unwrap();

        bo.engine.share_skill(&id, "review").await.unwrap();
        assert_eq!(repo.file("skills/review/SKILL.md").unwrap(), "# Review\n");
        assert_eq!(repo.file("skills/review/refs/style.md").unwrap(), "style\n");
        assert_eq!(repo.file("skills/review/.DS_Store"), None);
        assert_eq!(
            repo.commits().pop().unwrap().message,
            "bo@pc: share skills/review"
        );
        assert!(!dir.exists(), "replaced by the group's copy");
        let backups: Vec<_> = std::fs::read_dir(bo.engine.paths.backups_dir())
            .unwrap()
            .collect();
        assert_eq!(backups.len(), 1);
        let backup = backups[0]
            .as_ref()
            .unwrap()
            .path()
            .join("skills")
            .join("review");
        assert!(backup.join("SKILL.md").is_file());
        assert_eq!(
            bo.apply.file(&id, "skills/review/SKILL.md").unwrap(),
            "# Review\n"
        );

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "again\n").unwrap();
        let err = bo.engine.share_skill(&id, "review").await.unwrap_err();
        assert!(err.to_string().contains("already has"), "{err}");
        for bad in ["", "..", "a/b", ".hidden"] {
            assert!(bo.engine.share_skill(&id, bad).await.is_err(), "{bad:?}");
        }
        assert!(bo.engine.share_skill(&id, "missing").await.is_err());
    }

    #[tokio::test]
    async fn share_skill_scans_for_secrets() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let mut bo = device(&world, "bo", "pc");
        bo.engine = bo.engine.with_scanner(fake_scanner);
        bo.engine.join(&id).await.unwrap();
        let dir = bo.engine.paths.claude_home.join("skills").join("leak");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "ghp_123\n").unwrap();
        let err = bo.engine.share_skill(&id, "leak").await.unwrap_err();
        assert!(matches!(err, Error::SecretFound { .. }), "{err}");
        assert!(dir.join("SKILL.md").is_file());
        assert_eq!(repo.file("skills/leak/SKILL.md"), None);
    }

    #[tokio::test]
    async fn undo_restores_the_group_version() {
        let (_, id, repo, bo) = joined().await;
        let before = commit_count(&repo);
        bo.apply.edit(&id, "agents/a.md", "mine\n");
        bo.engine
            .resolve_edit(&edit(&id, "agents/a.md"), EditAction::Undo)
            .await
            .unwrap();
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "base\n");
        assert_eq!(commit_count(&repo), before);
    }

    #[tokio::test]
    async fn keep_local_stops_managing_the_path() {
        let (_, id, repo, bo) = joined().await;
        let before = commit_count(&repo);
        bo.apply.edit(&id, "agents/a.md", "mine\n");
        bo.engine
            .resolve_edit(&edit(&id, "agents/a.md"), EditAction::KeepLocal)
            .await
            .unwrap();
        assert!(bo.apply.st.lock().unwrap().kept.contains("agents/a.md"));
        repo.set_file(group::CLAUDE_MD, "# New\n");
        bo.engine.sync_all().await.unwrap();
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "mine\n");
        assert_eq!(repo.file("agents/a.md").unwrap(), "base\n");
        assert_eq!(commit_count(&repo), before + 1, "only the test's commit");
    }

    #[tokio::test]
    async fn discover_recent_commits_and_leave() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let other = FakeRepo::with_files(
            GroupId::new("cy", "crew"),
            &[(group::FILE, &group_json("Crew", &["cy", "bo"]))],
        );
        other.add_collaborator("bo");
        world.add(&other);
        let bo = device(&world, "bo", "pc");
        let mut found = bo.engine.discover().await.unwrap();
        found.sort();
        assert_eq!(found, [id.clone(), GroupId::new("cy", "crew")]);

        bo.engine.join(&id).await.unwrap();
        assert_eq!(
            bo.engine.discover().await.unwrap(),
            [GroupId::new("cy", "crew")]
        );

        repo.set_file("agents/a.md", "v2\n");
        let commits = bo.engine.recent_commits(&id, 1).await.unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message, "update agents/a.md");

        bo.engine.leave(&id).await.unwrap();
        assert!(bo.engine.groups().await.unwrap().is_empty());
        assert!(bo.apply.paths(&id).is_empty());
        assert_eq!(repo.collaborator_logins(), ["bo"], "membership unchanged");
    }

    #[tokio::test]
    async fn two_devices_converge() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let ana = device(&world, "ana", "mac");
        let bo = device(&world, "bo", "pc");
        ana.engine.join(&id).await.unwrap();
        bo.engine.join(&id).await.unwrap();
        bo.apply.edit(&id, "agents/a.md", "from bo\n");
        bo.engine
            .resolve_edit(&edit(&id, "agents/a.md"), EditAction::Share)
            .await
            .unwrap();
        ana.engine.sync_if_changed().await.unwrap();
        assert_eq!(ana.apply.file(&id, "agents/a.md").unwrap(), "from bo\n");
        assert_eq!(repo.commits().pop().unwrap().author, "bo");
    }

    #[test]
    fn summaries() {
        let ch = |items: &[(&str, bool)]| -> Changes {
            items
                .iter()
                .map(|(p, keep)| (p.to_string(), keep.then(Vec::new)))
                .collect()
        };
        assert_eq!(summarize(&ch(&[("CLAUDE.md", true)])), "update CLAUDE.md");
        assert_eq!(
            summarize(&ch(&[
                ("skills/r/SKILL.md", true),
                ("skills/r/x.md", false)
            ])),
            "update skills/r"
        );
        assert_eq!(
            summarize(&ch(&[("CLAUDE.md", true), ("agents/a.md", false)])),
            "update CLAUDE.md; remove agents/a.md"
        );
        assert_eq!(
            summarize(&ch(&[("agents/a.md", false)])),
            "remove agents/a.md"
        );
        let many = ch(&[("a/1", true), ("a/2", true), ("a/3", true), ("a/4", false)]);
        assert_eq!(summarize(&many), "update 4 items");
    }

    #[test]
    fn logins() {
        for ok in ["a", "bo", "a-b", "A1", &"x".repeat(39)] {
            assert!(valid_login(ok), "{ok}");
        }
        for bad in ["", "-a", "a-", "a--b", "a_b", "a.b", &"x".repeat(40)] {
            assert!(!valid_login(bad), "{bad}");
        }
    }

    #[test]
    fn base_snapshots_round_trip_and_survive_a_crashed_swap() {
        let dir = tempfile::tempdir().unwrap();
        let world = FakeGitHub::new();
        let mut dev = device(&world, "bo", "pc");
        dev.engine.paths.state_dir = dir.path().into();
        let e = &dev.engine;
        let id = GroupId::new("ana", "g");
        assert_eq!(e.load_base(&id).unwrap(), Base::default());
        let base = Base {
            head: Head {
                commit: "c1".into(),
                tree: "t1".into(),
            },
            files: FileSet::from([
                ("CLAUDE.md".into(), b"x".to_vec()),
                ("skills/r/a/b.md".into(), b"y".to_vec()),
            ]),
        };
        e.save_base(&id, &base).unwrap();
        assert_eq!(e.load_base(&id).unwrap(), base);
        let mut next = base.clone();
        next.head.commit = "c2".into();
        next.files.remove("CLAUDE.md");
        e.save_base(&id, &next).unwrap();
        assert_eq!(e.load_base(&id).unwrap(), next);

        // Crash between the two renames: only `base.old` is left.
        let live = e.paths.base_dir(&id);
        std::fs::rename(&live, live.with_extension("old")).unwrap();
        assert_eq!(e.load_base(&id).unwrap(), next);
        e.save_base(&id, &base).unwrap();
        assert_eq!(e.load_base(&id).unwrap(), base);
        assert!(!live.with_extension("old").exists());
    }

    // End to end with the real apply layer, secret scanner and watcher.

    fn real_device(world: &FakeGitHub, login: &str) -> (Engine, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            claude_home: dir.path().join("claude"),
            state_dir: dir.path().join("state"),
        };
        let gh = GitHub::with_api(String::new(), "http://127.0.0.1:9");
        let engine = Engine::with_remotes(
            paths,
            gh,
            login.into(),
            "pc".into(),
            Arc::new(world.account(login)),
        );
        (engine, dir)
    }

    #[tokio::test]
    async fn integration_join_writes_into_claude_home() {
        let world = FakeGitHub::new();
        let (id, _) = group_repo(&world, &["ana", "bo"]);
        let (bo, _dir) = real_device(&world, "bo");
        let status = bo.join(&id).await.unwrap();
        assert_eq!(status.last_error, None);
        let home = &bo.paths.claude_home;
        assert_eq!(
            std::fs::read_to_string(home.join("agents").join("a.md")).unwrap(),
            "base\n"
        );
        assert!(home.join("CLAUDE.md").exists());
        assert!(!home.join(group::FILE).exists());
        assert!(bo.local_edits().unwrap().is_empty());
    }

    #[tokio::test]
    async fn integration_unshared_edit_survives_a_sync() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let (bo, _dir) = real_device(&world, "bo");
        bo.join(&id).await.unwrap();
        let before = repo.commits().len();
        let home = bo.paths.claude_home.clone();
        std::fs::write(home.join("agents").join("a.md"), "mine\n").unwrap();
        let copy = bo.paths.group_dir(&id).join(group::CLAUDE_MD);
        std::fs::write(&copy, "# Mine\n").unwrap();
        let settings = home.join("settings.json");
        std::fs::write(&settings, "{\"a\": 2}\n").unwrap();
        let listed = |e: &Engine| {
            let mut p: Vec<String> = e.local_edits().unwrap().into_iter().map(|e| e.path).collect();
            p.sort();
            p
        };
        let want = ["CLAUDE.md", "agents/a.md", "settings.shared.json#/a"];
        assert_eq!(listed(&bo), want);
        for _ in 0..2 {
            bo.sync_all().await.unwrap();
            assert_eq!(listed(&bo), want, "an unshared edit must stay a local edit");
        }
        assert_eq!(repo.commits().len(), before);
        assert_eq!(std::fs::read_to_string(home.join("agents").join("a.md")).unwrap(), "mine\n");
    }

    #[tokio::test]
    async fn integration_secret_blocks_the_push() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let (bo, _dir) = real_device(&world, "bo");
        bo.join(&id).await.unwrap();
        let before = repo.commits().len();
        let token = format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0".repeat(2));
        let agent = bo.paths.claude_home.join("agents").join("a.md");
        std::fs::write(&agent, format!("token: {token}\n")).unwrap();
        let edits = bo.local_edits().unwrap();
        let edit = edits.iter().find(|e| e.path == "agents/a.md").unwrap();
        let err = bo.resolve_edit(edit, EditAction::Share).await.unwrap_err();
        assert!(matches!(err, Error::SecretFound { .. }), "{err}");
        assert_eq!(repo.commits().len(), before);
    }

    #[tokio::test]
    async fn integration_lost_access_removes_files() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let (bo, _dir) = real_device(&world, "bo");
        bo.join(&id).await.unwrap();
        let agent = bo.paths.claude_home.join("agents").join("a.md");
        assert!(agent.exists());
        repo.remove_collaborator("bo").await.unwrap();
        let report = bo.sync_all().await.unwrap();
        assert!(report.groups[0].lost_access);
        assert!(!agent.exists());
    }
}
