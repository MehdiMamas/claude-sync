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
use crate::review::{self, ReviewState};
use crate::state::{now_unix, utc_stamp, write_atomic, write_json, AppState};
use crate::watch::{EditAction, LocalEdit};
use crate::{Error, FileSet, GroupId, Paths};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod claude_md;
mod conflicts;
mod copy;
mod revert;
pub use claude_md::{ClaudeMdShare, ClaudeMdShared};
pub use copy::{Copied, CopyFile, CopyPreview, CopyRequest, CopySetting, CopySyncs};
pub use revert::{RevertAction, RevertFile, RevertPlan};

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
    /// Joining placed or changed plugin files: open Claude Code sessions need
    /// `/reload-plugins`. Only set on the status a join returns.
    pub plugins_changed: bool,
    /// Changes other members made to the group's `CLAUDE.md` that wait for this user (§7a).
    pub review_pending: usize,
    /// "Review changes others make to CLAUDE.md" is on for this group.
    pub review_claude_md: bool,
    /// This user hosts the group (owns its repo): only the host's app changes the group.
    pub is_host: bool,
    /// The group's proposal mode: members' changes go to review as proposals (section 5c).
    pub proposal_mode: bool,
    /// `"on"`: GitHub enforces host-only pushes. `"unavailable"`: it can't on this plan.
    /// `None`: not known yet, or this user is not the host.
    pub host_lock: Option<String>,
    /// Turned off on this device: joined, but nothing of it is applied or synced here.
    pub disabled: bool,
}

/// What a change to a group came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Committed {
    /// Nothing to change.
    Nothing,
    /// Committed to the group's default branch.
    Pushed,
    /// Sent as proposal #n, waiting for approval (§5c).
    Proposed(u64),
}

/// An open proposal, for the group screen (§5c).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalView {
    pub number: u64,
    pub branch: String,
    pub author: String,
    pub title: String,
    pub url: String,
    pub files: Vec<crate::remote::ProposalFile>,
    pub approvals: Vec<String>,
    /// This user's approval is on it.
    pub approved_by_me: bool,
    /// This user made it.
    pub mine: bool,
    pub needs_update: bool,
}

/// The review of a group's `CLAUDE.md` (§7a), for the group screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeMdReview {
    /// Review is on for this device (its override, else the group's setting).
    pub review: bool,
    /// The group's "Auto-accept CLAUDE.md changes".
    pub auto_accept: bool,
    /// This device's choice; `None` follows the group.
    pub review_override: Option<bool>,
    /// Oldest first.
    pub pending: Vec<review::Item>,
    /// Changes the user declined (remembered, never asked again).
    pub declined: usize,
    /// The copy's accepted text differs from the group's (declined or pending changes).
    pub differs: bool,
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
    /// Groups whose repo was renamed or transferred; everything local follows to the new name.
    pub moved: Vec<MovedGroup>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MovedGroup {
    pub from: GroupId,
    pub to: GroupId,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSyncReport {
    pub id: GroupId,
    pub pulled: bool,
    pub pushed: bool,
    /// A change went out as a proposal, not a commit (§5c).
    pub proposed: bool,
    pub apply: crate::apply::ApplyReport,
    pub conflicts: Vec<ConflictInfo>,
    pub error: Option<String>,
    /// Access to the repo is gone; its content was removed from this machine.
    pub lost_access: bool,
    /// Changes to the group's `CLAUDE.md` that this pass put up for review (§7a).
    pub review: Vec<review::Item>,
}

/// Where the group repo keeps Before/Now notes for commits that could not carry them.
pub const CHANGE_NOTES_PATH: &str = "changes/before-now.json";

/// What a change did before and what it does now, for the Groups feed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChangeNote {
    pub before: Option<String>,
    pub now: Option<String>,
}

/// `{ "<commit sha>": { "before": "...", "now": "..." } }`. Anything else (bad JSON, wrong
/// shapes, entries with neither text) is skipped: the feed never fails over its notes.
pub fn parse_change_notes(bytes: &[u8]) -> BTreeMap<String, ChangeNote> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_slice(bytes) else {
        return BTreeMap::new();
    };
    let text = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|t| t.as_str())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    map.iter()
        .filter_map(|(sha, v)| {
            let note = ChangeNote {
                before: text(v, "before"),
                now: text(v, "now"),
            };
            (note.before.is_some() || note.now.is_some()).then(|| (sha.clone(), note))
        })
        .collect()
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
pub(crate) const PUSH_RETRIES: usize = 3;
/// Conflicts remembered per group for the UI.
pub(crate) const MAX_CONFLICTS: usize = 50;

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
    /// Take everything the group applied away again, keeping what a re-apply needs
    /// (Keep-local choices, bundled plugin folders).
    fn disable_group(&self, paths: &Paths, group: &GroupId) -> crate::Result<ApplyReport>;
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
    /// Write the group's `CLAUDE.md` copy as `copy`, with `accepted` as the version the app
    /// wrote (§7a). A copy that differs from `accepted` is a local edit.
    fn write_claude_md(
        &self,
        paths: &Paths,
        group: &GroupId,
        copy: &[u8],
        accepted: &[u8],
    ) -> crate::Result<()>;
    /// Paths the group ships that this device does not manage (kept local, collisions,
    /// unsafe). The engine uses the base version as their local side.
    fn excluded_paths(&self, _paths: &Paths, _group: &GroupId) -> crate::Result<BTreeSet<String>> {
        Ok(BTreeSet::new())
    }
    /// Paths and settings pointers (`settings.shared.json#/model`) this device keeps local
    /// for the group.
    fn kept_local(&self, _paths: &Paths, _group: &GroupId) -> crate::Result<BTreeSet<String>> {
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
    fn disable_group(&self, paths: &Paths, group: &GroupId) -> crate::Result<ApplyReport> {
        crate::apply::disable_group(paths, group)
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
    fn write_claude_md(
        &self,
        paths: &Paths,
        group: &GroupId,
        copy: &[u8],
        accepted: &[u8],
    ) -> crate::Result<()> {
        crate::apply::write_claude_md(paths, group, copy, accepted)
    }
    fn excluded_paths(&self, paths: &Paths, group: &GroupId) -> crate::Result<BTreeSet<String>> {
        crate::apply::excluded_paths(paths, group)
    }
    fn kept_local(&self, paths: &Paths, group: &GroupId) -> crate::Result<BTreeSet<String>> {
        Ok(crate::apply::Manifest::load(paths)?
            .groups
            .remove(&group.to_string())
            .map(|e| e.keep_local)
            .unwrap_or_default())
    }
}

/// The secret scan run on every file before it is committed (§9).
pub type Scanner = fn(&str) -> Vec<crate::secrets::Hit>;

pub struct Engine {
    pub paths: Paths,
    pub gh: GitHub,
    pub login: String,
    pub device: String,
    pub(crate) remotes: Arc<dyn RemoteFactory>,
    pub(crate) apply: Arc<dyn ApplyLayer>,
    pub(crate) scanner: Scanner,
    /// One operation at a time: syncs and user actions share the base snapshots.
    pub(crate) busy: tokio::sync::Mutex<()>,
    /// Groups whose host lock this run already ensured.
    pub(crate) locked: std::sync::Mutex<BTreeSet<GroupId>>,
    /// The user's home folder: personal memory keys are relative to it (`crate::personal`).
    pub(crate) user_home: Option<PathBuf>,
}

/// Last synced state of a group (§6): the remote head and its allowlisted files.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Base {
    pub(crate) head: Head,
    pub(crate) files: FileSet,
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
    /// Host only: `"on"` or `"unavailable"` once the ruleset was ensured.
    host_lock: Option<String>,
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
            locked: std::sync::Mutex::new(BTreeSet::new()),
            user_home: std::env::home_dir(),
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

    /// The home folder personal memory keys are made from (tests use a temp folder).
    pub fn with_user_home(mut self, home: PathBuf) -> Self {
        self.user_home = Some(home);
        self
    }

    /// One pass over every joined group: pull, merge, apply, push local changes, reconcile members
    /// if this user is the host, detect lost access. Errors per group go in the report, not `Err`.
    pub async fn sync_all(&self) -> crate::Result<SyncReport> {
        let _busy = self.busy.lock().await;
        let mut out = SyncReport {
            moved: self.follow_moves().await,
            ..SyncReport::default()
        };
        for id in AppState::load(&self.paths)?.groups {
            out.groups.push(self.sync_group(&id, None).await.0);
        }
        Ok(out)
    }

    /// Same, but only for groups whose remote head changed (ETag) or that have local edits to push.
    /// The 60 s loop calls this.
    pub async fn sync_if_changed(&self) -> crate::Result<SyncReport> {
        let _busy = self.busy.lock().await;
        if AppState::load(&self.paths)?.paused {
            return Ok(SyncReport::default());
        }
        let mut out = SyncReport {
            moved: self.follow_moves().await,
            ..SyncReport::default()
        };
        let state = AppState::load(&self.paths)?;
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

    /// Ask GitHub for each joined repo's current name; a renamed or transferred repo moves every
    /// local trace to the new id (`rename::migrate`, resumable). Errors (offline, no access) are
    /// left to the sync itself to report. The caller holds the busy lock.
    async fn follow_moves(&self) -> Vec<MovedGroup> {
        let mut moved = Vec::new();
        let Ok(state) = AppState::load(&self.paths) else {
            return moved;
        };
        for id in state.groups {
            let Ok(now) = self.remotes.open(&id).current_id().await else {
                continue;
            };
            if now.to_string().eq_ignore_ascii_case(&id.to_string()) {
                continue;
            }
            match crate::rename::migrate(&self.paths, &id, &now) {
                Ok(()) => moved.push(MovedGroup { from: id, to: now }),
                Err(e) => tracing::warn!("following {id} to {now} failed: {e}"),
            }
        }
        moved
    }

    /// Hand the group repo to another member (host only). GitHub emails the new owner, who must
    /// accept; until then nothing changes. Afterwards this user is a member, and every app follows
    /// the new name on its next sync.
    pub async fn transfer_group(&self, id: &GroupId, new_owner: &str) -> crate::Result<()> {
        if !valid_login(new_owner) {
            return Err(Error::Invalid(format!(
                "{new_owner:?} is not a GitHub login"
            )));
        }
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        self.require_host(id).await?;
        if new_owner.eq_ignore_ascii_case(&self.login) {
            return Err(Error::Invalid(format!("you already host {id}")));
        }
        let remote = self.remotes.open(id);
        let head = remote.head().await?;
        let files = remote.files_at(&head.commit).await?;
        let bytes = files
            .get(group::FILE)
            .ok_or_else(|| Error::Invalid(format!("{id} has no {}", group::FILE)))?;
        let gf = GroupFile::parse(bytes)?;
        if !gf.has_member(new_owner) {
            return Err(Error::Invalid(format!(
                "{new_owner} is not a member of {id}; add them first"
            )));
        }
        // The former host stays in `members`, so the new host's reconcile keeps their access.
        if !gf.has_member(&self.login) {
            self.edit_group_file(id, "keep former host as member", |gf| {
                Ok(gf.add_member(&self.login))
            })
            .await?;
        }
        remote.transfer(new_owner).await
    }

    /// Put the joined groups in `order` (first wins collisions, §7) and apply every group again
    /// in that order, so managed files, settings values and bundled marketplaces move to the new
    /// winner. `order` must list exactly the joined groups. The new order is saved before the
    /// apply, so a failed sync (offline) still keeps it; the error is in the report.
    pub async fn reorder_groups(&self, order: Vec<GroupId>) -> crate::Result<SyncReport> {
        let _busy = self.busy.lock().await;
        let mut state = AppState::load(&self.paths)?;
        let mut a: Vec<String> = state.groups.iter().map(|g| g.to_string()).collect();
        let mut b: Vec<String> = order.iter().map(|g| g.to_string()).collect();
        a.sort();
        b.sort();
        if a != b {
            return Err(Error::Invalid(
                "the new order must list exactly the joined groups".into(),
            ));
        }
        let mut out = SyncReport::default();
        if state.groups == order {
            return Ok(out);
        }
        state.groups = order.clone();
        state.save(&self.paths)?;
        for id in &order {
            out.groups.push(self.sync_group(id, None).await.0);
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

    /// Create a private group repo (default name `claude-sync-data`) and join it. Its
    /// `CLAUDE.md` is the creator's shared text (redacted), or a heading when there is none.
    pub async fn create_group(
        &self,
        name: &str,
        repo: &str,
        share: Option<&ClaudeMdShare>,
    ) -> crate::Result<GroupStatus> {
        let shared = share.and_then(|s| claude_md::group_text(&s.text));
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
                Some(
                    shared
                        .clone()
                        .unwrap_or_else(|| format!("# {name}\n"))
                        .into_bytes(),
                ),
            ),
            (group::SHARED_SETTINGS.to_string(), Some(b"{}\n".to_vec())),
        ]);
        self.scan(&files)?;
        remote
            .commit(&head, &files, &self.message("create group"))
            .await?;
        let status = self.join_locked(&id).await?;
        if let (Some(text), Some(true)) = (&shared, share.map(|s| s.remove_own)) {
            // The group exists either way; a failure here only leaves the text twice.
            if let Err(e) = self.remove_own_claude_md(&id, text) {
                tracing::warn!(group = %id, error = %e, "could not take the shared text out of CLAUDE.md");
            }
        }
        Ok(status)
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
        self.require_host(id).await?;
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
        self.require_host(id).await?;
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

    /// Turn a joined group off or back on on this device. Off takes everything it applied
    /// away (membership, Keep-local choices and the group's folder stay); on applies the last
    /// synced version again and syncs. Nothing is pushed while off.
    pub async fn set_group_enabled(
        &self,
        id: &GroupId,
        enabled: bool,
    ) -> crate::Result<ApplyReport> {
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        let mut state = AppState::load(&self.paths)?;
        if state.disabled.contains(id) != enabled {
            return Ok(ApplyReport::default());
        }
        if !enabled {
            let applied = self.apply.disable_group(&self.paths, id)?;
            state.disabled.insert(id.clone());
            state.save(&self.paths)?;
            return Ok(applied);
        }
        // The first pass applies what was last synced and treats it as the local side, so
        // the files that were taken away do not read as local deletions.
        let base = self.load_base(id)?;
        let mut files = FileSet::new();
        if base.files.contains_key(group::FILE) {
            let (gf, _) = read_group_file(id, &base.files)?;
            files = only(&base.files, |p| {
                group::is_allowlisted(p) && gf.syncs(p) && !group::is_repo_only(p)
            });
            if let Some(copy) =
                read_file_if_exists(&self.paths.group_dir(id).join(group::CLAUDE_MD))?
            {
                files.insert(group::CLAUDE_MD.into(), copy);
            }
        }
        let applied = self
            .apply
            .apply_group(&self.paths, id, &state.groups, &files)?;
        state.disabled.remove(id);
        state.save(&self.paths)?;
        self.sync_group(id, None).await;
        Ok(applied)
    }

    /// Leave on this device: remove managed content. Does not change membership.
    pub async fn leave(&self, id: &GroupId) -> crate::Result<ApplyReport> {
        let _busy = self.busy.lock().await;
        self.forget(id)
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
                self.require_host_or_proposals(id).await?;
                let local = self.apply.local_files(&self.paths, id)?;
                if let Some(bytes) = local.get(file).filter(|b| group::too_large(file, b.len())) {
                    return Err(Error::Invalid(format!(
                        "{file} is {} bytes; plugin files over {} MiB are not shared",
                        bytes.len(),
                        group::MAX_PLUGIN_FILE >> 20
                    )));
                }
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
                // The group's CLAUDE.md goes back to what this user accepted, not to the
                // group's text with the changes they declined (§7a).
                let accepted = match (file == group::CLAUDE_MD, self.load_review(id)?) {
                    (true, Some(st)) => Some(st.accepted.into_bytes()),
                    _ => None,
                };
                let base = self.load_base(id)?;
                let version = accepted
                    .as_deref()
                    .or(base.files.get(file).map(Vec::as_slice));
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
        self.require_host_or_proposals(id).await?;
        let dir = self.paths.claude_home.join("skills").join(skill);
        if !dir.join("SKILL.md").is_file() {
            return Err(Error::Invalid(format!(
                "{} has no SKILL.md, so it is not a skill",
                dir.display()
            )));
        }
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
        let committed = self
            .commit_or_propose(id, &format!("share skills/{skill}"), |gf, current| {
                if !gf.sync.skills {
                    return Err(Error::Invalid(format!("{id} does not sync skills")));
                }
                if current.keys().any(|p| p.starts_with(&prefix)) {
                    return Err(Error::Invalid(format!(
                        "{id} already has a skill named {skill}"
                    )));
                }
                Ok(Some(files.clone()))
            })
            .await?;
        if matches!(committed, Committed::Proposed(_)) {
            // Nothing is the group's yet: the folder stays until the proposal is merged.
            return Ok(());
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

    /// The group's Before/Now notes for past commits (`changes/before-now.json` on the default
    /// branch). Empty when the file is missing, unreadable or invalid.
    pub async fn change_notes(&self, id: &GroupId) -> BTreeMap<String, ChangeNote> {
        let remote = self.remotes.open(id);
        let Ok(head) = remote.recent_commits(1).await else {
            return BTreeMap::new();
        };
        let Some(newest) = head.first() else {
            return BTreeMap::new();
        };
        match remote.file_at(&newest.sha, CHANGE_NOTES_PATH).await {
            Ok(Some(bytes)) => parse_change_notes(&bytes),
            _ => BTreeMap::new(),
        }
    }

    /// Changes to the groups' `CLAUDE.md` waiting for this user, over all groups (§7a).
    pub fn claude_md_pending(&self) -> usize {
        AppState::load(&self.paths)
            .map(|s| {
                s.groups
                    .iter()
                    .map(|id| self.group_status(id).review_pending)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Changes others made to the group's `CLAUDE.md` that wait for this user (§7a).
    pub fn claude_md_review(&self, id: &GroupId) -> crate::Result<ClaudeMdReview> {
        let st = self.load_review(id)?.unwrap_or_default();
        let auto_accept = self.group_auto_accept(id);
        Ok(ClaudeMdReview {
            review: st.review.unwrap_or(!auto_accept),
            auto_accept,
            review_override: st.review,
            differs: st.accepted != st.group,
            declined: st.declined.len(),
            pending: st.pending,
        })
    }

    /// Accept (`accept`) or decline pending changes: `ids`, or all of them. Accepted changes
    /// go into the copy at once; declined ones never do and are not asked again. Nothing is
    /// pushed either way.
    pub async fn decide_claude_md(
        &self,
        id: &GroupId,
        ids: Option<&[String]>,
        accept: bool,
    ) -> crate::Result<ClaudeMdReview> {
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        let mut st = self.require_review(id)?;
        let before = st.accepted.clone();
        if accept {
            let applied = st.accept(ids);
            self.write_accepted(id, &mut st, &before, &applied, false)?;
        } else {
            st.decline(ids, now_unix());
            st.save(&self.review_file(id))?;
        }
        self.claude_md_review(id)
    }

    /// This device's choice on "Review changes others make to CLAUDE.md": `Some` overrides the
    /// group's setting, `None` follows it. When review ends up off, what is pending is accepted
    /// (the UI asks first) and later changes apply at once; past declines stay.
    pub async fn set_claude_md_review(
        &self,
        id: &GroupId,
        on: Option<bool>,
    ) -> crate::Result<ClaudeMdReview> {
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        let mut st = self.require_review(id)?;
        st.review = on;
        self.accept_if_unreviewed(id, &mut st)?;
        self.claude_md_review(id)
    }

    /// The group's "Auto-accept CLAUDE.md changes" (§7a): pushes `claude-sync.json`, then
    /// applies it here (accepting what is pending when review is now off). Needs push access.
    pub async fn set_group_auto_accept_claude_md(
        &self,
        id: &GroupId,
        on: bool,
    ) -> crate::Result<ClaudeMdReview> {
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        self.require_host(id).await?;
        let summary = format!("auto-accept CLAUDE.md {}", if on { "on" } else { "off" });
        self.edit_group_file(id, &summary, |gf| {
            let changed = gf.auto_accept_claude_md != on;
            gf.auto_accept_claude_md = on;
            Ok(changed)
        })
        .await?;
        // The snapshot is the group file's source here; the sync brings it in.
        self.sync_group(id, None).await;
        if let Some(mut st) = self.load_review(id)? {
            self.accept_if_unreviewed(id, &mut st)?;
        }
        self.claude_md_review(id)
    }

    /// The group's "Proposal mode" (§5c): with it on, every change to the group, the host's
    /// too, is a proposal that another member approves. Host only.
    pub async fn set_group_proposal_mode(&self, id: &GroupId, on: bool) -> crate::Result<()> {
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        self.require_host(id).await?;
        let summary = format!("proposal mode {}", if on { "on" } else { "off" });
        self.edit_group_file(id, &summary, |gf| {
            let changed = gf.proposal_mode != on;
            gf.proposal_mode = on;
            Ok(changed)
        })
        .await?;
        match self.sync_group(id, None).await {
            (_, Some(e)) => Err(e),
            (_, None) => Ok(()),
        }
    }

    /// The group's open proposals, for the group screen (§5c).
    pub async fn proposals(&self, id: &GroupId) -> crate::Result<Vec<ProposalView>> {
        self.require_joined(id)?;
        let me = &self.login;
        Ok(self
            .remotes
            .open(id)
            .proposals()
            .await?
            .into_iter()
            .map(|p| ProposalView {
                mine: p.author.eq_ignore_ascii_case(me),
                approved_by_me: p.approvals.iter().any(|a| a.eq_ignore_ascii_case(me)),
                number: p.number,
                branch: p.branch,
                author: p.author,
                title: p.title,
                url: p.url,
                files: p.files,
                approvals: p.approvals,
                needs_update: p.needs_update,
            })
            .collect())
    }

    /// Approve proposal `number`. Never your own. The host's approval of a member's proposal
    /// merges it; a member's approval of the host's is merged by the host's next sync.
    pub async fn approve_proposal(&self, id: &GroupId, number: u64) -> crate::Result<()> {
        let _busy = self.busy.lock().await;
        let p = self.find_proposal(id, number).await?;
        if p.mine {
            return Err(Error::Invalid(
                "you cannot approve your own proposal; another member has to".into(),
            ));
        }
        let remote = self.remotes.open(id);
        if self.require_host(id).await.is_ok() {
            if p.needs_update {
                return Err(Error::Invalid(format!(
                    "proposal #{number} needs an update before it can be merged"
                )));
            }
            remote.merge_proposal(number).await?;
            return match self.sync_group(id, None).await {
                (_, Some(e)) => Err(e),
                (_, None) => Ok(()),
            };
        }
        remote.approve_proposal(number).await
    }

    /// Decline proposal `number`: it is closed with a note and never merged.
    pub async fn decline_proposal(&self, id: &GroupId, number: u64) -> crate::Result<()> {
        let _busy = self.busy.lock().await;
        self.find_proposal(id, number).await?;
        let note = format!("Declined by {} in claude-sync.", self.login);
        self.remotes.open(id).decline_proposal(number, &note).await
    }

    async fn find_proposal(&self, id: &GroupId, number: u64) -> crate::Result<ProposalView> {
        self.proposals(id)
            .await?
            .into_iter()
            .find(|p| p.number == number)
            .ok_or_else(|| Error::Invalid(format!("proposal #{number} is not open")))
    }

    /// Accept what is pending when review is off for this device.
    fn accept_if_unreviewed(&self, id: &GroupId, st: &mut ReviewState) -> crate::Result<()> {
        let review = st.review.unwrap_or(!self.group_auto_accept(id));
        let before = st.accepted.clone();
        let applied = if review { Vec::new() } else { st.accept(None) };
        self.write_accepted(id, st, &before, &applied, false)
    }

    /// The group's `autoAcceptClaudeMd` as last synced; on when it can't be read.
    fn group_auto_accept(&self, id: &GroupId) -> bool {
        self.load_base(id)
            .ok()
            .and_then(|b| read_group_file(id, &b.files).ok())
            .is_none_or(|(gf, _)| gf.auto_accept_claude_md)
    }

    /// "Use the group's version": the copy becomes the group's `CLAUDE.md` again, pending and
    /// declined changes are forgotten. An edit of the copy is replaced (and backed up).
    pub async fn use_group_claude_md(&self, id: &GroupId) -> crate::Result<ClaudeMdReview> {
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        let mut st = self.require_review(id)?;
        let before = st.accepted.clone();
        st.reset();
        self.write_accepted(id, &mut st, &before, &[], true)?;
        let mut status = self.load_status(id);
        if status.shared.remove(group::CLAUDE_MD) {
            self.save_status(id, &status)?;
        }
        self.claude_md_review(id)
    }

    // ---- internals ----

    fn message(&self, summary: &str) -> String {
        format!("{}@{}: {summary}", self.login, self.device)
    }

    /// Only the host (the owner of the group repo) changes a group. Checked before any write.
    async fn require_host(&self, id: &GroupId) -> crate::Result<()> {
        let host = self.remotes.open(id).owner_login().await?;
        if host.eq_ignore_ascii_case(&self.login) {
            return Ok(());
        }
        let name = self.load_status(id).name;
        Err(Error::HostOnly {
            group: if name.is_empty() {
                id.repo.clone()
            } else {
                name
            },
            host,
        })
    }

    /// The group's `proposalMode` as last synced; off when it can't be read.
    fn group_proposal_mode(&self, id: &GroupId) -> bool {
        self.load_base(id)
            .ok()
            .and_then(|b| read_group_file(id, &b.files).ok())
            .is_some_and(|(gf, _)| gf.proposal_mode)
    }

    /// Like `require_host`, but with proposal mode on anyone with push access may write: their
    /// change goes to a proposal (§5c).
    async fn require_host_or_proposals(&self, id: &GroupId) -> crate::Result<()> {
        if self.group_proposal_mode(id) {
            return Ok(());
        }
        self.require_host(id).await
    }

    /// `proposal/<login>-<yyyymmddHHMMSS>` (UTC).
    fn proposal_branch(&self) -> String {
        let stamp: String = utc_stamp(now_unix())
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        format!("{}{}-{stamp}", crate::remote::PROPOSAL_PREFIX, self.login)
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
        let (report, _) = self.sync_group(id, Some(head)).await;
        Ok(GroupStatus {
            plugins_changed: report.apply.plugins_changed,
            ..self.group_status(id)
        })
    }

    /// Change `claude-sync.json` and commit it, retrying on push races. `false` when `edit`
    /// changed nothing.
    async fn edit_group_file(
        &self,
        id: &GroupId,
        summary: &str,
        edit: impl Fn(&mut GroupFile) -> crate::Result<bool>,
    ) -> crate::Result<bool> {
        self.require_host(id).await?;
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

    /// Commit what `change` makes of the group's files at its head, retrying when someone
    /// else pushed first. `change` gets the group file and those files; `None` commits
    /// nothing (`false`). Callers scan what they add. A push GitHub refuses (read access
    /// only) is said plainly.
    pub(crate) async fn commit_to_group(
        &self,
        id: &GroupId,
        summary: &str,
        change: impl FnMut(&GroupFile, &FileSet) -> crate::Result<Option<Changes>>,
    ) -> crate::Result<bool> {
        Ok(self.commit_or_propose(id, summary, change).await? != Committed::Nothing)
    }

    /// `commit_to_group`, saying whether the change landed or became a proposal (§5c).
    pub(crate) async fn commit_or_propose(
        &self,
        id: &GroupId,
        summary: &str,
        mut change: impl FnMut(&GroupFile, &FileSet) -> crate::Result<Option<Changes>>,
    ) -> crate::Result<Committed> {
        self.require_host_or_proposals(id).await?;
        let remote = self.remotes.open(id);
        let message = self.message(summary);
        let mut attempt = 0;
        loop {
            let head = remote.head().await?;
            let current = remote.files_at(&head.commit).await?;
            let (gf, read_only) = read_group_file(id, &current)?;
            if let Some(e) = read_only {
                return Err(e);
            }
            if !gf.proposal_mode {
                self.require_host(id).await?;
            }
            let Some(files) = change(&gf, &current)? else {
                return Ok(Committed::Nothing);
            };
            if gf.proposal_mode {
                let title = self.message(summary);
                return match remote
                    .propose(&head, &files, &self.proposal_branch(), &title)
                    .await
                {
                    Ok(number) => Ok(Committed::Proposed(number)),
                    Err(e) if is_lost_access(&e) => {
                        let host = remote
                            .owner_login()
                            .await
                            .unwrap_or_else(|_| id.owner.clone());
                        Err(no_push(&gf.name, &host))
                    }
                    Err(e) => Err(e),
                };
            }
            match remote.commit(&head, &files, &message).await {
                Ok(_) => return Ok(Committed::Pushed),
                Err(Error::PushRace) if attempt < PUSH_RETRIES => attempt += 1,
                Err(e) if is_lost_access(&e) => {
                    let host = remote
                        .owner_login()
                        .await
                        .unwrap_or_else(|_| id.owner.clone());
                    return Err(no_push(&gf.name, &host));
                }
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
            proposed: false,
            apply: ApplyReport::default(),
            conflicts: Vec::new(),
            error: None,
            lost_access: false,
            review: Vec::new(),
        };
        if AppState::load(&self.paths).is_ok_and(|s| s.disabled.contains(id)) {
            return (report, None);
        }
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
        let host = remote.owner_login().await?;
        let is_host = host.eq_ignore_ascii_case(&self.login);
        let (gf, read_only) = loop {
            let remote_files = if head == base.head {
                base.files.clone()
            } else {
                report.pulled = true;
                remote.files_at(&head.commit).await?
            };
            let (gf, read_only) = read_group_file(id, &remote_files)?;
            // Only the host's app writes to the group; with proposal mode on a member's
            // edits go out as proposals (§5c). Otherwise they stay local.
            let may = is_host || gf.proposal_mode;
            if !may {
                status.shared.clear();
            }
            let keep = |p: &str| group::is_allowlisted(p) && gf.syncs(p);
            let mut base_v = only(&base.files, keep);
            let mut remote_v = only(&remote_files, keep);
            let mut too_large = Vec::new();
            let (mut local, copy) = self.local_view(id, &base_v, status, keep, &mut too_large)?;
            // The group's CLAUDE.md goes through review (§7a), not the three-way merge.
            let md = if keep(group::CLAUDE_MD) {
                for files in [&mut base_v, &mut remote_v, &mut local] {
                    files.remove(group::CLAUDE_MD);
                }
                let shared = may && read_only.is_none() && status.shared.contains(group::CLAUDE_MD);
                let pass = self
                    .review_pull(
                        remote,
                        id,
                        &base,
                        &head,
                        &remote_files,
                        copy,
                        shared,
                        report,
                    )
                    .await?;
                Some(pass)
            } else {
                remove_file_if_exists(&self.review_file(id))?;
                None
            };
            let md_push = md.as_ref().and_then(|m| m.push.clone()).filter(|_| may);
            let merged = merge::three_way(&base_v, &local, &remote_v);

            // Save losing local copies before the apply overwrites them.
            report
                .conflicts
                .extend(self.save_conflicts(id, &merged.conflicts)?);
            let mut to_apply = merged.merged;
            to_apply.retain(|p, _| !group::is_repo_only(p));
            // A local edit that is not being shared stays a local edit: the apply gets the
            // group's version, so it leaves the edit in place instead of adopting it.
            for p in merged.to_push.keys() {
                if may && read_only.is_none() && status.shared.contains(p) {
                    continue;
                }
                match remote_v.get(p) {
                    Some(bytes) => to_apply.insert(p.clone(), bytes.clone()),
                    None => to_apply.remove(p),
                };
            }
            if let Some(text) = md.as_ref().and_then(|m| m.apply.clone()) {
                to_apply.insert(group::CLAUDE_MD.into(), text);
            }
            report.apply = self.apply.apply_group(&self.paths, id, &order, &to_apply)?;
            for note in too_large {
                if !report.apply.skipped.contains(&note) {
                    report.apply.skipped.push(note);
                }
            }
            status.collisions = report.apply.collisions.iter().map(Into::into).collect();
            base = Base {
                head: head.clone(),
                files: remote_files,
            };
            self.save_base(id, &base)?;

            status.shared.retain(|p| {
                merged.to_push.contains_key(p) || (p == group::CLAUDE_MD && md_push.is_some())
            });
            let mut push: Changes = merged
                .to_push
                .into_iter()
                .filter(|(p, _)| may && read_only.is_none() && status.shared.contains(p))
                .collect();
            if let Some(content) = md_push {
                push.insert(group::CLAUDE_MD.into(), content);
            }
            if push.is_empty() {
                break (gf, read_only);
            }
            self.scan(&push)?;
            if gf.proposal_mode {
                // The edit stays a local edit until its proposal is merged (§5c).
                let title = self.message(&summarize(&push));
                remote
                    .propose(&head, &push, &self.proposal_branch(), &title)
                    .await?;
                status.shared.retain(|p| !push.contains_key(p));
                report.proposed = true;
                break (gf, read_only);
            }
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
                    if let (Some(content), Some(md)) = (push.get(group::CLAUDE_MD), md) {
                        self.review_pushed(id, content.as_deref(), md.copy)?;
                    }
                    break (gf, read_only);
                }
                Err(Error::PushRace) if attempt < PUSH_RETRIES => {
                    attempt += 1;
                    head = remote.head().await?;
                }
                Err(e) => return Err(e),
            }
        };

        status.name = gf.name.clone();
        status.members = gf.members.clone();
        status.host = host.clone();
        if let Some(e) = read_only {
            return Err(e);
        }
        if is_host {
            if gf.proposal_mode {
                self.merge_approved(remote, &gf).await;
            }
            self.ensure_lock(remote, id, status).await;
            status.invited = self.reconcile(remote, &gf, &host).await?;
        } else {
            status.invited.clear();
            status.host_lock = None;
        }
        Ok(())
    }

    /// The host's own proposals only the host's app can merge (§5a): merge each one that
    /// another member approved and that still applies. Never an error; the next sync picks
    /// the merged files up.
    async fn merge_approved(&self, remote: &dyn RemoteRepo, gf: &GroupFile) {
        let open = match remote.proposals().await {
            Ok(open) => open,
            Err(e) => {
                tracing::warn!(error = %e, "could not list the group's proposals");
                return;
            }
        };
        for p in open {
            let mine = p.author.eq_ignore_ascii_case(&self.login);
            let approved = p.approvals.iter().any(|a| {
                !a.eq_ignore_ascii_case(&self.login)
                    && gf.members.iter().any(|m| m.eq_ignore_ascii_case(a))
            });
            if mine && approved && !p.needs_update {
                if let Err(e) = remote.merge_proposal(p.number).await {
                    tracing::warn!(proposal = p.number, error = %e, "could not merge a proposal");
                }
            }
        }
    }

    /// The host's app asks GitHub to refuse everyone else's pushes, once per group per run.
    /// GitHub may say no (a private repo on a free plan): never an error, only `host_lock`.
    async fn ensure_lock(&self, remote: &dyn RemoteRepo, id: &GroupId, status: &mut Status) {
        if !self.locked.lock().expect("lock").insert(id.clone()) {
            return;
        }
        status.host_lock = Some(match remote.ensure_host_lock().await {
            Ok(()) => "on".into(),
            Err(e) => {
                tracing::warn!(group = %id, error = %e, "GitHub would not lock the group to its host");
                "unavailable".into()
            }
        });
    }

    /// The local side of the merge. The apply layer reads managed files back; the engine
    /// supplies the rest from the base: repo-only files (group file, schedules), paths the apply layer did
    /// not write (collisions), paths missing locally that the user did not choose to delete
    /// (kept local, or not readable back), and plugin files grown over the size cap (noted
    /// in `too_large`). Also returns the group's `CLAUDE.md` copy as it is on disk, which
    /// review handles (§7a).
    fn local_view(
        &self,
        id: &GroupId,
        base: &FileSet,
        status: &Status,
        keep: impl Fn(&str) -> bool,
        too_large: &mut Vec<String>,
    ) -> crate::Result<(FileSet, LocalCopy)> {
        let mut local: FileSet = self
            .apply
            .local_files(&self.paths, id)?
            .into_iter()
            .filter(|(p, _)| keep(p) && !group::is_repo_only(p))
            .collect();
        let mut held = self.apply.excluded_paths(&self.paths, id)?;
        held.extend(status.collisions.iter().map(|c| c.path().to_string()));
        let copy = match held.contains(group::CLAUDE_MD) {
            true => LocalCopy::Held,
            false => LocalCopy::Copy(local.get(group::CLAUDE_MD).cloned()),
        };
        for (p, bytes) in &local {
            if group::too_large(p, bytes.len()) {
                too_large.push(group::too_large_note(p));
                held.insert(p.clone());
            }
        }
        let paths: BTreeSet<String> = base.keys().chain(&held).cloned().collect();
        for p in paths {
            let unshared_delete = !local.contains_key(&p) && !status.shared.contains(&p);
            if group::is_repo_only(&p) || held.contains(&p) || unshared_delete {
                match base.get(&p) {
                    Some(bytes) => local.insert(p, bytes.clone()),
                    None => local.remove(&p),
                };
            }
        }
        Ok((local, copy))
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
        state.disabled.remove(id);
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
        let review = self.load_review(id).ok().flatten();
        let or = |v: String, fallback: &str| {
            if v.is_empty() {
                fallback.to_string()
            } else {
                v
            }
        };
        let is_host = or(s.host.clone(), &id.owner).eq_ignore_ascii_case(&self.login);
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
            plugins_changed: false,
            review_pending: review.as_ref().map_or(0, |r| r.pending.len()),
            review_claude_md: review
                .and_then(|r| r.review)
                .unwrap_or(!self.group_auto_accept(id)),
            is_host,
            proposal_mode: self.group_proposal_mode(id),
            host_lock: s.host_lock,
            disabled: AppState::load(&self.paths).is_ok_and(|s| s.disabled.contains(id)),
        }
    }

    fn load_base(&self, id: &GroupId) -> crate::Result<Base> {
        load_base_dir(&self.paths.base_dir(id))
    }

    fn save_base(&self, id: &GroupId, base: &Base) -> crate::Result<()> {
        save_base_dir(&self.paths.base_dir(id), base)
    }

    fn review_file(&self, id: &GroupId) -> PathBuf {
        self.paths.group_dir(id).join(review::FILE)
    }

    fn load_review(&self, id: &GroupId) -> crate::Result<Option<ReviewState>> {
        ReviewState::load(&self.review_file(id))
    }

    fn require_review(&self, id: &GroupId) -> crate::Result<ReviewState> {
        self.load_review(id)?.ok_or_else(|| {
            Error::Invalid(format!("{id} has no CLAUDE.md on this device to review"))
        })
    }

    /// Save `st` and make the copy follow its accepted text: a copy that was in step takes it
    /// as it is; an edited copy gets the same `applied` changes and stays an edit (unless
    /// `replace`).
    fn write_accepted(
        &self,
        id: &GroupId,
        st: &mut ReviewState,
        before: &str,
        applied: &[review::Item],
        replace: bool,
    ) -> crate::Result<()> {
        st.save(&self.review_file(id))?;
        let copy = self
            .apply
            .local_files(&self.paths, id)?
            .remove(group::CLAUDE_MD)
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        let copy = match copy {
            Some(c) if c != before && !replace => review::apply_all(&c, applied),
            _ => st.accepted.clone(),
        };
        self.apply
            .write_claude_md(&self.paths, id, copy.as_bytes(), st.accepted.as_bytes())
    }

    /// The group's `CLAUDE.md` through review (§7a). The changes since the last pass are
    /// attributed to their authors: this account's apply to the accepted text, others' wait
    /// for the user (or apply too when review is off). The copy follows the accepted text and
    /// keeps an unshared edit. A shared edit is rebased onto the group's text, so what the
    /// user declined is never pushed back.
    #[allow(clippy::too_many_arguments)]
    async fn review_pull(
        &self,
        remote: &dyn RemoteRepo,
        id: &GroupId,
        base: &Base,
        head: &Head,
        remote_files: &FileSet,
        copy: LocalCopy,
        shared: bool,
        report: &mut GroupSyncReport,
    ) -> crate::Result<MdPass> {
        let old = text_of(base.files.get(group::CLAUDE_MD));
        let new_bytes = remote_files.get(group::CLAUDE_MD);
        let new = text_of(new_bytes);
        let mut st = match self.load_review(id)? {
            Some(st) => st,
            // First sight: a join takes the group's text as it is; an upgraded device the
            // version it already applied.
            None => ReviewState::new(if base.head.is_empty() { &new } else { &old }),
        };
        let steps = if st.group == new {
            Vec::new()
        } else if st.group == old {
            self.claude_md_steps(remote, &base.head, head, &new).await
        } else {
            vec![unknown_step(&new, head)]
        };
        let held = matches!(copy, LocalCopy::Held);
        let choice = st.review;
        let auto_accept =
            read_group_file(id, remote_files).map_or(true, |(gf, _)| gf.auto_accept_claude_md);
        let review_on = choice.unwrap_or(!auto_accept);
        // A copy kept local is not this device's concern: the group's text is accepted.
        st.review = Some(review_on && !held);
        let before = st.accepted.clone();
        let mut pulled = st.pull(&steps);
        // The group (or this device) went to auto-accept: what waited is accepted now.
        if !review_on && !held {
            pulled.applied.extend(st.accept(None));
        }
        st.review = choice;
        report.review.extend(pulled.new);

        let copy = match copy {
            LocalCopy::Copy(c) => c.map(|b| String::from_utf8_lossy(&b).into_owned()),
            LocalCopy::Held => None,
        };
        let current = match copy {
            // An unshared edit stays; the changes applied this pass go into it too.
            Some(c) if c != before => {
                let patched = review::apply_all(&c, &pulled.applied);
                if st.accepted != before {
                    self.apply.write_claude_md(
                        &self.paths,
                        id,
                        patched.as_bytes(),
                        st.accepted.as_bytes(),
                    )?;
                }
                Some(patched)
            }
            Some(_) => Some(st.accepted.clone()),
            None => None,
        };
        let push = match &current {
            _ if !shared || held => None,
            // A shared delete.
            None => new_bytes.map(|_| None),
            Some(c) => {
                let text = review::rebase(&st.accepted, c, &new);
                (text != new).then(|| Some(text.into_bytes()))
            }
        };
        st.save(&self.review_file(id))?;
        let apply = (new_bytes.is_some() || !st.accepted.is_empty())
            .then(|| st.accepted.clone().into_bytes());
        Ok(MdPass {
            apply,
            push,
            copy: current,
        })
    }

    /// The user's edit of the copy was pushed as `pushed`: it is the group's text now, and
    /// the copy is accepted as it is.
    fn review_pushed(
        &self,
        id: &GroupId,
        pushed: Option<&[u8]>,
        copy: Option<String>,
    ) -> crate::Result<()> {
        let mut st = self.load_review(id)?.unwrap_or_default();
        st.group = pushed
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        st.accepted = copy.unwrap_or_default();
        st.settle();
        st.save(&self.review_file(id))?;
        if pushed.is_some() {
            let c = st.accepted.as_bytes();
            self.apply.write_claude_md(&self.paths, id, c, c)?;
        }
        Ok(())
    }

    /// Who changed the group's `CLAUDE.md` between `from` and `to`, step by step. When the
    /// history cannot be read, every change counts as someone else's: nothing applies unasked.
    async fn claude_md_steps(
        &self,
        remote: &dyn RemoteRepo,
        from: &Head,
        to: &Head,
        new: &str,
    ) -> Vec<review::Step> {
        let commits = match remote.commits_between(&from.commit, &to.commit).await {
            Ok(c) if !c.is_empty() => c,
            Ok(_) => return vec![unknown_step(new, to)],
            Err(e) => {
                tracing::warn!(group = %remote.id(), error = %e, "could not read who changed CLAUDE.md");
                return vec![unknown_step(new, to)];
            }
        };
        let mine = |c: &CommitInfo| c.author.eq_ignore_ascii_case(&self.login);
        let step = |c: &CommitInfo, text: &str| review::Step {
            text: text.into(),
            mine: mine(c),
            author: c.author.clone(),
            commit: c.sha.clone(),
            message: c.message.clone(),
            at_unix: c.at_unix,
        };
        let authors: BTreeSet<String> = commits.iter().map(|c| c.author.to_lowercase()).collect();
        let last = commits.last().expect("not empty");
        if authors.len() == 1 {
            return vec![step(last, new)];
        }
        // Several authors: the last other member's commit takes the whole change.
        let theirs = || {
            let c = commits.iter().rev().find(|c| !mine(c)).unwrap_or(last);
            vec![review::Step {
                mine: false,
                ..step(c, new)
            }]
        };
        if commits.len() > MAX_HISTORY_WALK {
            return theirs();
        }
        let mut steps = Vec::new();
        for c in &commits {
            match remote.file_at(&c.sha, group::CLAUDE_MD).await {
                Ok(bytes) => steps.push(step(c, &text_of(bytes.as_ref()))),
                Err(e) => {
                    tracing::warn!(group = %remote.id(), error = %e, "could not read CLAUDE.md history");
                    return theirs();
                }
            }
        }
        if steps.last().map(|s| s.text.as_str()) != Some(new) {
            return theirs();
        }
        steps
    }
}

/// Commits read one by one to attribute `CLAUDE.md` changes when several people pushed.
const MAX_HISTORY_WALK: usize = 20;

/// The group's `CLAUDE.md` copy as the merge found it.
enum LocalCopy {
    /// Not managed on this device ("Keep local").
    Held,
    /// `None`: missing.
    Copy(Option<Vec<u8>>),
}

/// What review decided for one pass.
struct MdPass {
    /// The accepted text the apply gives the copy (`None`: no copy).
    apply: Option<Vec<u8>>,
    /// The user's shared edit, rebased onto the group's text (`Some(None)`: delete).
    push: Option<Option<Vec<u8>>>,
    /// The copy after this pass.
    copy: Option<String>,
}

fn text_of(bytes: Option<&Vec<u8>>) -> String {
    bytes
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default()
}

/// A change whose author is unknown: it waits for review.
fn unknown_step(text: &str, head: &Head) -> review::Step {
    review::Step {
        text: text.into(),
        mine: false,
        author: String::new(),
        commit: head.commit.clone(),
        message: String::new(),
        at_unix: now_unix(),
    }
}

fn read_file_if_exists(file: &Path) -> crate::Result<Option<Vec<u8>>> {
    match std::fs::read(file) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn remove_file_if_exists(file: &Path) -> crate::Result<()> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// `<dir>/head.json` and `<dir>/files/<path>`.
pub(crate) fn load_base_dir(dir: &Path) -> crate::Result<Base> {
    let dir = dir.to_path_buf();
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
pub(crate) fn save_base_dir(dir: &Path, base: &Base) -> crate::Result<()> {
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
        std::fs::rename(dir, &old)?;
    }
    std::fs::rename(&new, dir)?;
    remove_dir_if_exists(&old)
}

/// One file of a group's base snapshot (`base.old` after a crash in the middle of a save):
/// how the app reads repo-only files such as `schedules.json`. `None`: the group has none.
pub fn read_base_file(paths: &Paths, id: &GroupId, path: &str) -> crate::Result<Option<Vec<u8>>> {
    let dir = paths.base_dir(id);
    let Some(dir) = [dir.clone(), dir.with_extension("old")]
        .into_iter()
        .find(|d| d.join("head.json").is_file() || d.join("files").is_dir())
    else {
        return Ok(None);
    };
    match std::fs::read(join_rel(&dir.join("files"), path)) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
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

/// The error when GitHub refuses a push to a group the user can read.
pub(crate) fn no_push(group: &str, host: &str) -> Error {
    Error::Invalid(format!(
        "You can't push to {group}: your GitHub account can read it but not write to it. Ask its host ({host}) for write access."
    ))
}

pub(crate) fn is_lost_access(e: &Error) -> bool {
    matches!(
        e,
        Error::GitHub {
            status: 403 | 404,
            ..
        }
    )
}

pub(crate) fn only(files: &FileSet, keep: impl Fn(&str) -> bool) -> FileSet {
    files
        .iter()
        .filter(|(p, _)| keep(p))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}

pub(crate) fn with_changes(mut files: FileSet, changes: &Changes) -> FileSet {
    for (path, content) in changes {
        match content {
            Some(bytes) => files.insert(path.clone(), bytes.clone()),
            None => files.remove(path),
        };
    }
    files
}

/// `update CLAUDE.md, skills/review`, `remove agents/old.md` or `update 5 items`.
pub(crate) fn summarize(changes: &Changes) -> String {
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

pub(crate) fn join_rel(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

/// Every regular file under `root`, keyed by its relative path with `/`. Symlinks are skipped.
pub(crate) fn read_tree(root: &Path) -> crate::Result<FileSet> {
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

pub(crate) fn remove_dir_if_exists(dir: &Path) -> crate::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn change_notes_parse_leniently() {
        let got = parse_change_notes(
            br#"{"a1": {"before": " old ", "now": "new"}, "b2": {"now": "x"}, "c3": {}, "d4": 5, "e5": {"before": ""}}"#,
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got["a1"].before.as_deref(), Some("old"));
        assert_eq!(got["b2"].before, None);
        assert!(parse_change_notes(b"not json").is_empty());
        assert!(parse_change_notes(b"[1]").is_empty());
    }

    use super::*;
    use crate::remote::fake::{FakeGitHub, FakeRepo};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// `~/.claude` of one device, in memory. Paths in `collide` belong to the user and are never
    /// written; kept paths stop being managed. Like the real apply, a file edited locally is
    /// left alone while the group's version stays the one last applied.
    #[derive(Default)]
    pub(super) struct StubApply {
        st: Mutex<StubState>,
    }

    #[derive(Default)]
    struct StubState {
        local: HashMap<GroupId, FileSet>,
        /// The group's version of each file as last applied.
        applied: HashMap<GroupId, FileSet>,
        kept: BTreeSet<String>,
        /// What `keep_local` was called with, per group.
        kept_in: HashMap<GroupId, BTreeSet<String>>,
        collide: BTreeSet<String>,
        removed: Vec<GroupId>,
        disabled: Vec<GroupId>,
        /// The group order each `apply_group` call was given.
        orders: Vec<Vec<GroupId>>,
    }

    impl StubApply {
        pub(super) fn file(&self, id: &GroupId, path: &str) -> Option<String> {
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
            order: &[GroupId],
            files: &FileSet,
        ) -> crate::Result<ApplyReport> {
            let mut st = self.st.lock().unwrap();
            st.orders.push(order.to_vec());
            let mut report = ApplyReport::default();
            let old = st.local.remove(group).unwrap_or_default();
            let applied = st
                .applied
                .insert(group.clone(), files.clone())
                .unwrap_or_default();
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
        fn disable_group(&self, _: &Paths, group: &GroupId) -> crate::Result<ApplyReport> {
            let mut st = self.st.lock().unwrap();
            st.disabled.push(group.clone());
            st.applied.remove(group);
            let removed = st.local.remove(group).unwrap_or_default();
            Ok(ApplyReport {
                removed: removed.into_keys().collect(),
                ..Default::default()
            })
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
        fn keep_local(&self, _: &Paths, group: &GroupId, path: &str) -> crate::Result<()> {
            let mut st = self.st.lock().unwrap();
            st.kept_in
                .entry(group.clone())
                .or_default()
                .insert(path.into());
            if !path.starts_with(crate::apply::SETTINGS_POINTER_PREFIX) {
                st.kept.insert(path.into());
            }
            Ok(())
        }
        fn kept_local(&self, _: &Paths, group: &GroupId) -> crate::Result<BTreeSet<String>> {
            let st = self.st.lock().unwrap();
            Ok(st.kept_in.get(group).cloned().unwrap_or_default())
        }
        fn write_claude_md(
            &self,
            _: &Paths,
            group: &GroupId,
            copy: &[u8],
            accepted: &[u8],
        ) -> crate::Result<()> {
            let mut st = self.st.lock().unwrap();
            if st.kept.contains(group::CLAUDE_MD) {
                return Ok(());
            }
            let path = String::from(group::CLAUDE_MD);
            let local = st.local.entry(group.clone()).or_default();
            local.insert(path.clone(), copy.to_vec());
            let applied = st.applied.entry(group.clone()).or_default();
            applied.insert(path, accepted.to_vec());
            Ok(())
        }
        fn local_edits(&self, _: &Paths) -> crate::Result<Vec<LocalEdit>> {
            Ok(Vec::new())
        }
    }

    fn no_secrets(_: &str) -> Vec<crate::secrets::Hit> {
        Vec::new()
    }

    pub(super) fn fake_scanner(text: &str) -> Vec<crate::secrets::Hit> {
        text.lines()
            .enumerate()
            .filter(|(_, l)| l.contains("ghp_"))
            .map(|(n, _)| crate::secrets::Hit {
                line: n + 1,
                pattern: "github-token",
            })
            .collect()
    }

    pub(super) struct Device {
        pub(super) engine: Engine,
        pub(super) apply: Arc<StubApply>,
        _dir: tempfile::TempDir,
    }

    pub(super) fn device(world: &FakeGitHub, login: &str, device: &str) -> Device {
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
        // Most tests are about the review (§7a); the auto-accept ones turn it on.
        gf.auto_accept_claude_md = false;
        String::from_utf8(gf.to_bytes()).unwrap()
    }

    /// `ana/claude-sync-data` with `members` as collaborators and a few files.
    pub(super) fn group_repo(world: &FakeGitHub, members: &[&str]) -> (GroupId, FakeRepo) {
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
    pub(super) async fn joined() -> (FakeGitHub, GroupId, FakeRepo, Device) {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&id).await.unwrap();
        (world, id, repo, bo)
    }

    /// Like [`joined`], but `bo` hosts the group (owns the repo): only a host's app pushes.
    pub(super) async fn joined_host() -> (FakeGitHub, GroupId, FakeRepo, Device) {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        repo.set_owner("bo");
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
        let (_, id, repo, bo) = joined_host().await;
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
        let (_, id, repo, bo) = joined_host().await;
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

    /// Mark `path` as shared in `bo`'s saved status, as the Share button would have.
    fn mark_shared(bo: &Device, id: &GroupId, path: &str) {
        let mut status = bo.engine.load_status(id);
        status.shared.insert(path.into());
        bo.engine.save_status(id, &status).unwrap();
    }

    #[tokio::test]
    async fn member_sync_pushes_nothing_and_keeps_edit_local() {
        let (_, id, repo, bo) = joined().await;
        let before = commit_count(&repo);
        bo.apply.edit(&id, "agents/a.md", "mine\n");
        mark_shared(&bo, &id, "agents/a.md");

        let report = bo.engine.sync_all().await.unwrap();
        assert!(!report.groups[0].pushed);
        assert_eq!(commit_count(&repo), before);
        assert_eq!(repo.file("agents/a.md").unwrap(), "base\n");
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "mine\n", "kept");
        assert!(bo.engine.load_status(&id).shared.is_empty());
        let g = &bo.engine.groups().await.unwrap()[0];
        assert!(!g.is_host);
        assert_eq!(g.host_lock, None);
    }

    #[tokio::test]
    async fn member_cannot_change_the_group() {
        let (_, id, repo, bo) = joined().await;
        bo.apply.edit(&id, "agents/a.md", "mine\n");
        let before = commit_count(&repo);

        let err = bo.engine.add_member(&id, "cy").await.unwrap_err();
        assert!(matches!(err, Error::HostOnly { .. }), "{err}");
        let err = bo.engine.share_skill(&id, "pdf").await.unwrap_err();
        assert!(matches!(err, Error::HostOnly { .. }), "{err}");
        let err = bo
            .engine
            .resolve_edit(&edit(&id, "agents/a.md"), EditAction::Share)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::HostOnly { .. }), "{err}");
        assert_eq!(commit_count(&repo), before);
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "mine\n");
    }

    #[tokio::test]
    async fn host_still_pushes() {
        let (_, id, repo, bo) = joined_host().await;
        let before = commit_count(&repo);
        bo.apply.edit(&id, "agents/a.md", "mine\n");
        mark_shared(&bo, &id, "agents/a.md");

        let report = bo.engine.sync_all().await.unwrap();
        assert!(report.groups[0].pushed);
        assert_eq!(commit_count(&repo), before + 1);
        assert_eq!(repo.file("agents/a.md").unwrap(), "mine\n");
        assert!(bo.engine.groups().await.unwrap()[0].is_host);
    }

    #[tokio::test]
    async fn off_group_pushes_nothing_and_on_deletes_nothing() {
        let (_, id, repo, bo) = joined_host().await;

        bo.engine.set_group_enabled(&id, false).await.unwrap();
        assert!(bo.engine.groups().await.unwrap()[0].disabled);
        assert_eq!(bo.apply.file(&id, "agents/a.md"), None);
        repo.set_file(
            "agents/b.md",
            "from the group
",
        );
        let before = commit_count(&repo);
        let report = bo.engine.sync_all().await.unwrap();
        assert!(!report.groups[0].pushed && !report.groups[0].pulled);
        assert_eq!(commit_count(&repo), before);
        assert_eq!(
            bo.apply.file(&id, "agents/b.md"),
            None,
            "nothing applied while off"
        );

        bo.engine.set_group_enabled(&id, true).await.unwrap();
        assert!(!bo.engine.groups().await.unwrap()[0].disabled);
        assert_eq!(
            bo.apply.file(&id, "agents/a.md").unwrap(),
            "base
"
        );
        assert_eq!(
            bo.apply.file(&id, "agents/b.md").unwrap(),
            "from the group
"
        );
        assert_eq!(
            repo.file("agents/a.md").unwrap(),
            "base
"
        );
        assert_eq!(commit_count(&repo), before, "re-enabling pushed a deletion");
    }

    #[tokio::test]
    async fn host_lock_is_ensured_once_per_run() {
        let (_, _, repo, bo) = joined_host().await;
        bo.engine.sync_all().await.unwrap();
        bo.engine.sync_all().await.unwrap();
        assert_eq!(calls(&repo, "ensure_host_lock"), 1);
        assert_eq!(repo.rulesets(), [crate::remote::HOST_LOCK_NAME]);
        let g = &bo.engine.groups().await.unwrap()[0];
        assert_eq!(g.host_lock.as_deref(), Some("on"));
    }

    #[tokio::test]
    async fn lock_failure_never_fails_sync() {
        // The lock is first asked for while joining, so the refusal is set up before that.
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        repo.set_owner("bo");
        repo.fail("ensure_host_lock", 403);
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&id).await.unwrap();
        let report = bo.engine.sync_all().await.unwrap();
        assert_eq!(report.groups[0].error, None);
        let g = &bo.engine.groups().await.unwrap()[0];
        assert_eq!(g.host_lock.as_deref(), Some("unavailable"));
        assert_eq!(g.last_error, None);
        assert!(repo.rulesets().is_empty());
    }

    #[tokio::test]
    async fn shared_settings_merge_by_key() {
        let (_, id, repo, bo) = joined_host().await;
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
        let (_, id, repo, bo) = joined_host().await;
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
        let (_, id, repo, bo) = joined_host().await;
        bo.apply.delete(&id, "agents/a.md");
        bo.engine.sync_all().await.unwrap();
        assert!(repo.file("agents/a.md").is_some());
        assert_eq!(
            bo.apply.file(&id, "agents/a.md"),
            None,
            "still a local edit"
        );

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
        let (_, id, repo, bo) = joined_host().await;
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
        let (_, id, repo, bo) = joined_host().await;
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
        repo.set_owner("bo");
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
    async fn the_host_edits_the_group_file() {
        let (_, id, repo, bo) = joined_host().await;
        bo.engine.add_member(&id, "cy").await.unwrap();
        let gf = GroupFile::parse(repo.file(group::FILE).unwrap().as_bytes()).unwrap();
        assert_eq!(gf.members, ["ana", "bo", "cy"]);
        assert_eq!(
            repo.commits().pop().unwrap().message,
            "bo@pc: add member cy"
        );
        let pending: Vec<String> = repo
            .pending_invitations()
            .into_iter()
            .map(|(_, l)| l)
            .collect();
        assert_eq!(
            pending,
            ["ana", "cy"],
            "the host invites the new member (ana was never accepted)"
        );
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
    async fn reorder_saves_the_order_and_applies_every_group_in_it() {
        let (world, a, _, bo) = joined().await;
        let (b, _) = group_repo(&world, &["cy", "bo"]);
        bo.engine.join(&b).await.unwrap();
        assert_eq!(
            AppState::load(&bo.engine.paths).unwrap().groups,
            [a.clone(), b.clone()]
        );
        bo.apply.st.lock().unwrap().orders.clear();

        let report = bo
            .engine
            .reorder_groups(vec![b.clone(), a.clone()])
            .await
            .unwrap();
        assert_eq!(
            report
                .groups
                .iter()
                .map(|g| g.id.clone())
                .collect::<Vec<_>>(),
            [b.clone(), a.clone()]
        );
        assert_eq!(
            AppState::load(&bo.engine.paths).unwrap().groups,
            [b.clone(), a.clone()]
        );
        assert_eq!(
            bo.engine
                .groups()
                .await
                .unwrap()
                .iter()
                .map(|g| g.id.clone())
                .collect::<Vec<_>>(),
            [b.clone(), a.clone()]
        );
        let orders = bo.apply.st.lock().unwrap().orders.clone();
        assert!(!orders.is_empty());
        assert!(orders.iter().all(|o| *o == [b.clone(), a.clone()]));

        // Same order again: nothing to do. A list that is not the joined groups is refused.
        assert!(bo
            .engine
            .reorder_groups(vec![b.clone(), a.clone()])
            .await
            .unwrap()
            .groups
            .is_empty());
        assert!(bo.engine.reorder_groups(vec![a.clone()]).await.is_err());
        assert!(bo
            .engine
            .reorder_groups(vec![a.clone(), a.clone()])
            .await
            .is_err());
        assert_eq!(AppState::load(&bo.engine.paths).unwrap().groups, [b, a]);
    }

    #[tokio::test]
    async fn a_renamed_repo_is_followed_everywhere() {
        let (world, a, _, bo) = joined().await;
        let (b, _) = group_repo(&world, &["cy", "bo"]);
        bo.engine.join(&b).await.unwrap();
        let new = GroupId::new("ana", "renamed");
        world.rename(&a, &new);

        let report = bo.engine.sync_all().await.unwrap();
        assert_eq!(report.moved.len(), 1);
        assert_eq!((&report.moved[0].from, &report.moved[0].to), (&a, &new));
        // The order is kept: the renamed group stays first.
        assert_eq!(
            AppState::load(&bo.engine.paths).unwrap().groups,
            [new.clone(), b.clone()]
        );
        assert!(!bo.engine.paths.group_dir(&a).exists());
        assert!(bo.engine.paths.group_dir(&new).join("status.json").exists());
        assert_eq!(
            report
                .groups
                .iter()
                .map(|g| g.id.clone())
                .collect::<Vec<_>>(),
            [new.clone(), b.clone()]
        );

        // Already moved: nothing to do.
        assert!(bo.engine.sync_all().await.unwrap().moved.is_empty());
        assert!(bo.engine.sync_if_changed().await.unwrap().moved.is_empty());
    }

    #[tokio::test]
    async fn an_interrupted_move_finishes_on_the_next_run() {
        let (world, a, _, bo) = joined().await;
        let new = GroupId::new("ana", "renamed");
        world.rename(&a, &new);
        // The folder moved, then the app stopped before saving state.json.
        std::fs::rename(
            bo.engine.paths.group_dir(&a),
            bo.engine.paths.group_dir(&new),
        )
        .unwrap();

        let report = bo.engine.sync_all().await.unwrap();
        assert_eq!(report.moved.len(), 1);
        assert_eq!(
            AppState::load(&bo.engine.paths).unwrap().groups,
            std::slice::from_ref(&new)
        );
        assert!(bo.engine.paths.group_dir(&new).exists());
        assert!(bo
            .engine
            .group_status(&new)
            .members
            .contains(&"bo".to_string()));
    }

    #[tokio::test]
    async fn transferring_the_group_flips_the_host() {
        let (world, id, repo, bo) = joined_host().await;
        assert!(bo.engine.group_status(&id).is_host);

        // Only members can receive it; a stranger and yourself are refused.
        assert!(bo.engine.transfer_group(&id, "zed").await.is_err());
        assert!(bo.engine.transfer_group(&id, "bo").await.is_err());
        assert!(repo.transfer_requests().is_empty());

        bo.engine.transfer_group(&id, "ana").await.unwrap();
        assert_eq!(repo.transfer_requests(), ["ana"]);
        // Nothing changes until ana accepts.
        assert!(bo.engine.sync_all().await.unwrap().moved.is_empty());

        let new = GroupId::new("ana", "handed-over");
        world.rename(&id, &new);
        let report = bo.engine.sync_all().await.unwrap();
        assert_eq!(report.moved.len(), 1);
        let status = bo.engine.group_status(&new);
        assert!(!status.is_host);
        assert_eq!(status.host, "ana");
        // The former host stays a member, so ana's reconcile keeps them.
        assert!(status.members.contains(&"bo".to_string()));
        // A member can't transfer.
        assert!(bo.engine.transfer_group(&new, "bo").await.is_err());
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
        let status = ana.engine.create_group("Friends", "", None).await.unwrap();
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

        let err = ana
            .engine
            .create_group("Again", "", None)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::GitHub { status: 422, .. }), "{err}");
        let named = ana.engine.create_group("", "crew", None).await.unwrap();
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
        repo.set_owner("bo");
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
        repo.set_owner("bo");
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
        let (_, id, repo, bo) = joined_host().await;
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

        // Claude Code's `synced` folder holds account skills one level down: not a skill.
        let synced = bo.engine.paths.claude_home.join("skills").join("synced");
        std::fs::create_dir_all(synced.join("org").join("pdf")).unwrap();
        std::fs::write(synced.join("org").join("pdf").join("SKILL.md"), "pdf\n").unwrap();
        let err = bo.engine.share_skill(&id, "synced").await.unwrap_err();
        assert!(err.to_string().contains("no SKILL.md"), "{err}");
        assert!(!repo.files().keys().any(|p| p.starts_with("skills/synced/")));
    }

    #[tokio::test]
    async fn share_skill_scans_for_secrets() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        repo.set_owner("bo");
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

    // The group's CLAUDE.md through review (§7a).

    fn md(e: &Engine, id: &GroupId) -> ClaudeMdReview {
        e.claude_md_review(id).unwrap()
    }

    fn lines(items: &[review::Item]) -> Vec<(review::Kind, String, String)> {
        items
            .iter()
            .map(|i| (i.kind, i.lines.join("|"), i.author.clone()))
            .collect()
    }

    #[tokio::test]
    async fn others_claude_md_changes_wait_for_review() {
        let (_, id, repo, bo) = joined().await;
        repo.set_file(group::CLAUDE_MD, "Use tabs.\n");
        let report = bo.engine.sync_all().await.unwrap();
        assert_eq!(
            lines(&report.groups[0].review),
            [
                (review::Kind::Remove, "# Friends".into(), "ana".into()),
                (review::Kind::Add, "Use tabs.".into(), "ana".into()),
            ]
        );
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends\n",
            "neither the addition nor the removal applies unasked"
        );
        let g = &bo.engine.groups().await.unwrap()[0];
        assert_eq!((g.review_pending, g.review_claude_md), (2, true));
        assert_eq!(md(&bo.engine, &id).pending.len(), 2);
        // Asked once: the next sync reports nothing new and pushes nothing.
        let before = commit_count(&repo);
        let again = bo.engine.sync_all().await.unwrap();
        assert!(again.groups[0].review.is_empty());
        assert_eq!(commit_count(&repo), before);
    }

    #[tokio::test]
    async fn own_claude_md_changes_apply_at_once() {
        let (world, id, repo, bo) = joined_host().await;
        // From a git clone under the same account.
        repo.set_file_as("BO", group::CLAUDE_MD, "# Friends\nMine.\n");
        let report = bo.engine.sync_all().await.unwrap();
        assert!(report.groups[0].review.is_empty());
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends\nMine.\n"
        );
        // From another device of the same account.
        let mac = device(&world, "bo", "mac");
        mac.engine.join(&id).await.unwrap();
        mac.apply.edit(&id, group::CLAUDE_MD, "# Friends\n");
        mac.engine
            .resolve_edit(&edit(&id, group::CLAUDE_MD), EditAction::Share)
            .await
            .unwrap();
        assert_eq!(repo.file(group::CLAUDE_MD).unwrap(), "# Friends\n");
        let report = bo.engine.sync_all().await.unwrap();
        assert!(report.groups[0].review.is_empty());
        assert_eq!(bo.apply.file(&id, group::CLAUDE_MD).unwrap(), "# Friends\n");
    }

    #[tokio::test]
    async fn accept_applies_and_decline_keeps_without_asking_again() {
        let (_, id, repo, bo) = joined().await;
        repo.set_file(group::CLAUDE_MD, "# Friends\nUse tabs.\nBe brief.\n");
        bo.engine.sync_all().await.unwrap();
        let pending = md(&bo.engine, &id).pending;
        assert_eq!(pending.len(), 1, "one block of two lines");
        let before = commit_count(&repo);
        let ids = [pending[0].id.clone()];
        let after = bo
            .engine
            .decide_claude_md(&id, Some(&ids), true)
            .await
            .unwrap();
        assert!(after.pending.is_empty() && !after.differs);
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends\nUse tabs.\nBe brief.\n"
        );

        // Ana removes a line and adds one; bo declines both.
        repo.set_file(group::CLAUDE_MD, "# Friends\nBe brief.\nNo emoji.\n");
        bo.engine.sync_all().await.unwrap();
        assert_eq!(md(&bo.engine, &id).pending.len(), 2);
        let after = bo.engine.decide_claude_md(&id, None, false).await.unwrap();
        assert!(after.pending.is_empty() && after.differs);
        assert_eq!(after.declined, 2);
        let mine = "# Friends\nUse tabs.\nBe brief.\n";
        assert_eq!(bo.apply.file(&id, group::CLAUDE_MD).unwrap(), mine);
        for _ in 0..2 {
            let report = bo.engine.sync_all().await.unwrap();
            assert!(report.groups[0].review.is_empty(), "not asked again");
        }
        assert_eq!(bo.apply.file(&id, group::CLAUDE_MD).unwrap(), mine);
        assert_eq!(commit_count(&repo), before + 1, "only ana's commit");

        // A new change is asked about; the declined ones stay declined.
        repo.set_file(
            group::CLAUDE_MD,
            "# Friends\nBe brief.\nNo emoji.\nShort PRs.\n",
        );
        let report = bo.engine.sync_all().await.unwrap();
        assert_eq!(
            lines(&report.groups[0].review),
            [(review::Kind::Add, "Short PRs.".into(), "ana".into())]
        );
    }

    #[tokio::test]
    async fn declined_differences_are_never_pushed() {
        let (_, id, repo, bo) = joined_host().await;
        repo.set_file_as("ana", group::CLAUDE_MD, "# Friends\nUse tabs.\n");
        bo.engine.sync_all().await.unwrap();
        bo.engine.decide_claude_md(&id, None, false).await.unwrap();
        // Bo edits their copy (without ana's line) and shares the edit.
        bo.apply
            .edit(&id, group::CLAUDE_MD, "# Friends\nBo's rule.\n");
        bo.engine
            .resolve_edit(&edit(&id, group::CLAUDE_MD), EditAction::Share)
            .await
            .unwrap();
        assert_eq!(
            repo.file(group::CLAUDE_MD).unwrap(),
            "# Friends\nBo's rule.\nUse tabs.\n",
            "only bo's own line is pushed; ana's declined line stays in the group"
        );
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends\nBo's rule.\n"
        );
        let before = commit_count(&repo);
        let report = bo.engine.sync_all().await.unwrap();
        assert!(report.groups[0].review.is_empty() && !report.groups[0].pushed);
        assert_eq!(commit_count(&repo), before);
    }

    #[tokio::test]
    async fn review_off_applies_others_changes_again() {
        let (_, id, repo, bo) = joined().await;
        repo.set_file(group::CLAUDE_MD, "# Friends\nUse tabs.\n");
        bo.engine.sync_all().await.unwrap();
        let off = bo
            .engine
            .set_claude_md_review(&id, Some(false))
            .await
            .unwrap();
        assert!(
            !off.review && off.pending.is_empty(),
            "pending ones are accepted"
        );
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends\nUse tabs.\n"
        );
        repo.set_file(group::CLAUDE_MD, "Use tabs.\n");
        let report = bo.engine.sync_all().await.unwrap();
        assert_eq!(report.groups[0].review.len(), 0);
        assert_eq!(bo.apply.file(&id, group::CLAUDE_MD).unwrap(), "Use tabs.\n");
        assert!(!bo.engine.groups().await.unwrap()[0].review_claude_md);
    }

    /// Set (or, with `None`, remove) `autoAcceptClaudeMd` in the repo's group file.
    fn set_auto_accept(repo: &FakeRepo, on: Option<bool>) {
        let mut v: serde_json::Value =
            serde_json::from_str(&repo.file(group::FILE).unwrap()).unwrap();
        match on {
            Some(b) => v["autoAcceptClaudeMd"] = b.into(),
            None => {
                v.as_object_mut().unwrap().remove("autoAcceptClaudeMd");
            }
        }
        repo.set_file(group::FILE, &serde_json::to_string_pretty(&v).unwrap());
    }

    /// Turn `proposalMode` on or off in the repo's group file.
    fn set_proposal_mode(repo: &FakeRepo, on: bool) {
        let mut v: serde_json::Value =
            serde_json::from_str(&repo.file(group::FILE).unwrap()).unwrap();
        v["proposalMode"] = on.into();
        repo.set_file(group::FILE, &serde_json::to_string_pretty(&v).unwrap());
    }

    /// `bo` has joined with proposal mode on and edited `agents/a.md`, shared.
    async fn proposing_member() -> (FakeGitHub, GroupId, FakeRepo, Device) {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        set_proposal_mode(&repo, true);
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&id).await.unwrap();
        bo.apply.edit(&id, "agents/a.md", "mine\n");
        mark_shared(&bo, &id, "agents/a.md");
        (world, id, repo, bo)
    }

    #[tokio::test]
    async fn proposal_mode_off_still_host_only() {
        let (_, id, repo, bo) = joined().await;
        let err = bo
            .engine
            .set_group_proposal_mode(&id, true)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::HostOnly { .. }), "{err}");
        bo.apply.edit(&id, "agents/a.md", "mine\n");
        mark_shared(&bo, &id, "agents/a.md");
        let report = bo.engine.sync_all().await.unwrap();
        assert!(!report.groups[0].pushed && !report.groups[0].proposed);
        let err = bo.engine.share_skill(&id, "pdf").await.unwrap_err();
        assert!(matches!(err, Error::HostOnly { .. }), "{err}");
        assert!(repo.pulls().is_empty());
    }

    #[tokio::test]
    async fn member_edit_becomes_a_proposal() {
        let (_, id, repo, bo) = proposing_member().await;
        let before = commit_count(&repo);

        let report = bo.engine.sync_all().await.unwrap();
        assert!(report.groups[0].proposed);
        assert!(!report.groups[0].pushed);
        assert_eq!(commit_count(&repo), before, "main is untouched");
        assert_eq!(repo.file("agents/a.md").unwrap(), "base\n");
        let pulls = repo.pulls();
        assert_eq!(pulls.len(), 1);
        let branch = &pulls[0].branch;
        let stamp = branch.strip_prefix("proposal/bo-").expect(branch);
        assert_eq!(stamp.len(), 14, "{branch}");
        assert!(stamp.bytes().all(|b| b.is_ascii_digit()), "{branch}");
        assert_eq!(pulls[0].state, "Open");
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "mine\n", "kept");

        let again = bo.engine.sync_all().await.unwrap();
        assert!(!again.groups[0].proposed, "not proposed twice");
        assert_eq!(repo.pulls().len(), 1);
    }

    #[tokio::test]
    async fn host_approves_member_proposal_and_it_merges() {
        let (world, id, repo, bo) = proposing_member().await;
        bo.engine.sync_all().await.unwrap();
        let ana = device(&world, "ana", "pc");
        ana.engine.join(&id).await.unwrap();
        let before = commit_count(&repo);

        let open = ana.engine.proposals(&id).await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].author, "bo");
        assert!(!open[0].mine);
        ana.engine
            .approve_proposal(&id, open[0].number)
            .await
            .unwrap();
        assert_eq!(repo.file("agents/a.md").unwrap(), "mine\n");
        assert_eq!(commit_count(&repo), before + 1, "squash-merged");
        assert_eq!(repo.pulls()[0].state, "Merged");
        assert!(ana.engine.proposals(&id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn member_approval_lets_host_sync_merge_host_proposal() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        set_proposal_mode(&repo, true);
        let ana = device(&world, "ana", "pc");
        ana.engine.join(&id).await.unwrap();
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&id).await.unwrap();
        ana.apply.edit(&id, "agents/a.md", "hosts\n");
        mark_shared(&ana, &id, "agents/a.md");
        let before = commit_count(&repo);

        let report = ana.engine.sync_all().await.unwrap();
        assert!(report.groups[0].proposed && !report.groups[0].pushed);
        assert_eq!(commit_count(&repo), before);
        let number = repo.pulls()[0].number;
        let err = ana.engine.approve_proposal(&id, number).await.unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "{err}");
        ana.engine.sync_all().await.unwrap();
        assert_eq!(repo.pulls()[0].state, "Open", "no approval yet");

        bo.engine.approve_proposal(&id, number).await.unwrap();
        assert_eq!(
            repo.file("agents/a.md").unwrap(),
            "base\n",
            "not merged by bo"
        );
        ana.engine.sync_all().await.unwrap();
        assert_eq!(repo.pulls()[0].state, "Merged");
        assert_eq!(repo.file("agents/a.md").unwrap(), "hosts\n");
        assert_eq!(commit_count(&repo), before + 1);
    }

    #[tokio::test]
    async fn decline_closes_the_proposal_with_a_comment() {
        let (world, id, repo, bo) = proposing_member().await;
        bo.engine.sync_all().await.unwrap();
        let ana = device(&world, "ana", "pc");
        ana.engine.join(&id).await.unwrap();
        let number = repo.pulls()[0].number;

        ana.engine.decline_proposal(&id, number).await.unwrap();
        let pull = &repo.pulls()[0];
        assert_eq!(pull.state, "Closed");
        assert_eq!(pull.comments.len(), 1, "{:?}", pull.comments);
        assert_eq!(repo.file("agents/a.md").unwrap(), "base\n");
        assert!(ana.engine.proposals(&id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn own_proposal_cannot_be_approved() {
        let (_, id, repo, bo) = proposing_member().await;
        bo.engine.sync_all().await.unwrap();
        let number = repo.pulls()[0].number;
        let err = bo.engine.approve_proposal(&id, number).await.unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "{err}");
        assert!(repo.pulls()[0].reviews.is_empty());
    }

    #[tokio::test]
    async fn stale_proposal_needs_update_and_is_not_merged() {
        let (world, id, repo, bo) = proposing_member().await;
        bo.engine.sync_all().await.unwrap();
        let number = repo.pulls()[0].number;
        // The same file changes on main after the proposal was made.
        repo.set_file("agents/a.md", "newer\n");
        let ana = device(&world, "ana", "pc");
        ana.engine.join(&id).await.unwrap();

        let open = ana.engine.proposals(&id).await.unwrap();
        assert!(open[0].needs_update);
        let err = ana.engine.approve_proposal(&id, number).await.unwrap_err();
        assert!(err.to_string().contains("needs an update"), "{err}");
        assert_eq!(repo.pulls()[0].state, "Open");
        assert_eq!(repo.file("agents/a.md").unwrap(), "newer\n");
    }

    #[tokio::test]
    async fn only_host_can_set_proposal_mode() {
        let (_, id, _, bo) = joined().await;
        let err = bo
            .engine
            .set_group_proposal_mode(&id, true)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::HostOnly { .. }), "{err}");

        let (_, id, repo, host) = joined_host().await;
        let mode = |repo: &FakeRepo| {
            GroupFile::parse(repo.file(group::FILE).unwrap().as_bytes())
                .unwrap()
                .proposal_mode
        };
        host.engine
            .set_group_proposal_mode(&id, true)
            .await
            .unwrap();
        assert!(mode(&repo));
        host.engine
            .set_group_proposal_mode(&id, false)
            .await
            .unwrap();
        assert!(!mode(&repo));
    }

    #[tokio::test]
    async fn share_skill_in_proposal_mode_keeps_the_folder() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        set_proposal_mode(&repo, true);
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&id).await.unwrap();
        let dir = bo.engine.paths.claude_home.join("skills").join("review");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "# Review\n").unwrap();
        let before = commit_count(&repo);

        bo.engine.share_skill(&id, "review").await.unwrap();
        assert_eq!(commit_count(&repo), before);
        assert_eq!(repo.file("skills/review/SKILL.md"), None);
        assert_eq!(repo.pulls().len(), 1);
        assert!(dir.join("SKILL.md").is_file(), "kept until merged");
        let backups = std::fs::read_dir(bo.engine.paths.backups_dir())
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(backups, 0, "no backup move");
    }
    #[tokio::test]
    async fn an_absent_auto_accept_key_applies_others_changes_at_once() {
        let (_, id, repo, bo) = joined().await;
        set_auto_accept(&repo, None);
        repo.set_file(
            group::CLAUDE_MD,
            "# Friends
Use tabs.
",
        );
        let report = bo.engine.sync_all().await.unwrap();
        assert!(report.groups[0].review.is_empty());
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends
Use tabs.
"
        );
        let m = md(&bo.engine, &id);
        assert!(m.auto_accept && !m.review && m.review_override.is_none());
        assert!(m.pending.is_empty());
    }

    #[tokio::test]
    async fn auto_accept_off_in_the_group_keeps_changes_pending() {
        let (_, id, repo, bo) = joined().await;
        set_auto_accept(&repo, Some(false));
        repo.set_file(
            group::CLAUDE_MD,
            "# Friends
Use tabs.
",
        );
        bo.engine.sync_all().await.unwrap();
        let m = md(&bo.engine, &id);
        assert!(!m.auto_accept && m.review);
        assert_eq!(m.pending.len(), 1);
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends
"
        );
    }

    #[tokio::test]
    async fn a_device_override_wins_over_the_group() {
        let (_, id, repo, bo) = joined().await;
        set_auto_accept(&repo, Some(true));
        bo.engine.sync_all().await.unwrap();
        let m = bo
            .engine
            .set_claude_md_review(&id, Some(true))
            .await
            .unwrap();
        assert!(m.review && m.auto_accept && m.review_override == Some(true));
        repo.set_file(
            group::CLAUDE_MD,
            "# Friends
Use tabs.
",
        );
        bo.engine.sync_all().await.unwrap();
        assert_eq!(md(&bo.engine, &id).pending.len(), 1);
        // Following the group again accepts what waited.
        let m = bo.engine.set_claude_md_review(&id, None).await.unwrap();
        assert!(!m.review && m.pending.is_empty());
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends
Use tabs.
"
        );
    }

    #[tokio::test]
    async fn turning_the_group_on_accepts_what_waits_on_the_next_sync() {
        let (_, id, repo, bo) = joined().await;
        repo.set_file(
            group::CLAUDE_MD,
            "# Friends
Use tabs.
",
        );
        bo.engine.sync_all().await.unwrap();
        assert_eq!(md(&bo.engine, &id).pending.len(), 1);
        set_auto_accept(&repo, Some(true));
        let report = bo.engine.sync_all().await.unwrap();
        assert!(report.groups[0].review.is_empty());
        let m = md(&bo.engine, &id);
        assert!(m.pending.is_empty() && !m.review);
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends
Use tabs.
"
        );
    }

    #[tokio::test]
    async fn setting_the_group_auto_accept_commits_the_group_file() {
        let (_, id, repo, bo) = joined_host().await;
        repo.set_file(
            group::CLAUDE_MD,
            "# Friends
Use tabs.
",
        );
        bo.engine.sync_all().await.unwrap();
        let m = bo
            .engine
            .set_group_auto_accept_claude_md(&id, true)
            .await
            .unwrap();
        assert_eq!(
            repo.commits().last().unwrap().message,
            "bo@pc: auto-accept CLAUDE.md on"
        );
        let gf = GroupFile::parse(repo.file(group::FILE).unwrap().as_bytes()).unwrap();
        assert!(gf.auto_accept_claude_md);
        assert!(m.auto_accept && !m.review && m.pending.is_empty());
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends
Use tabs.
"
        );
        let n = commit_count(&repo);
        bo.engine
            .set_group_auto_accept_claude_md(&id, true)
            .await
            .unwrap();
        assert_eq!(commit_count(&repo), n, "nothing changed, nothing pushed");
        bo.engine
            .set_group_auto_accept_claude_md(&id, false)
            .await
            .unwrap();
        assert_eq!(
            repo.commits().last().unwrap().message,
            "bo@pc: auto-accept CLAUDE.md off"
        );
    }

    #[tokio::test]
    async fn a_member_cannot_change_the_group_auto_accept() {
        let (_, id, repo, bo) = joined().await;
        let n = commit_count(&repo);
        let err = bo
            .engine
            .set_group_auto_accept_claude_md(&id, true)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::HostOnly { .. }), "{err}");
        assert_eq!(commit_count(&repo), n);
    }

    #[tokio::test]
    async fn an_unshared_edit_stays_while_accepted_changes_go_into_it() {
        let (_, id, repo, bo) = joined().await;
        bo.apply
            .edit(&id, group::CLAUDE_MD, "# Friends\nMy note.\n");
        repo.set_file(group::CLAUDE_MD, "Top.\n# Friends\n");
        bo.engine.sync_all().await.unwrap();
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends\nMy note.\n"
        );
        bo.engine.decide_claude_md(&id, None, true).await.unwrap();
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "Top.\n# Friends\nMy note.\n"
        );
        // Undo goes back to what bo accepted.
        bo.engine
            .resolve_edit(&edit(&id, group::CLAUDE_MD), EditAction::Undo)
            .await
            .unwrap();
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "Top.\n# Friends\n"
        );
    }

    #[tokio::test]
    async fn a_line_deleted_in_one_group_stays_in_the_other() {
        let world = FakeGitHub::new();
        let text = "# Rules\nAlways run the tests.\n";
        let a = GroupId::new("ana", group::DEFAULT_REPO);
        let b = GroupId::new("cy", "crew");
        for (id, members) in [(&a, ["ana", "bo"]), (&b, ["cy", "bo"])] {
            let json = group_json("G", &members);
            let repo = FakeRepo::with_files(
                id.clone(),
                &[(group::FILE, &json), (group::CLAUDE_MD, text)],
            );
            repo.add_collaborator("bo");
            world.add(&repo);
        }
        let bo = device(&world, "bo", "pc");
        bo.engine.join(&a).await.unwrap();
        bo.engine.join(&b).await.unwrap();
        world
            .repo(&b)
            .unwrap()
            .set_file(group::CLAUDE_MD, "# Rules\n");
        let report = bo.engine.sync_all().await.unwrap();
        let of = |id: &GroupId| {
            report
                .groups
                .iter()
                .find(|g| &g.id == id)
                .map(|g| lines(&g.review))
                .unwrap_or_default()
        };
        assert!(of(&a).is_empty());
        assert_eq!(
            of(&b),
            [(
                review::Kind::Remove,
                "Always run the tests.".into(),
                "cy".into()
            )]
        );
        bo.engine.decide_claude_md(&b, None, false).await.unwrap();
        bo.engine.sync_all().await.unwrap();
        assert_eq!(bo.apply.file(&a, group::CLAUDE_MD).unwrap(), text);
        assert_eq!(bo.apply.file(&b, group::CLAUDE_MD).unwrap(), text);
        assert_eq!(
            world.repo(&a).unwrap().file(group::CLAUDE_MD).unwrap(),
            text
        );
        assert_eq!(md(&bo.engine, &a).pending.len(), 0);
    }

    #[tokio::test]
    async fn several_authors_are_told_apart() {
        let (_, id, repo, bo) = joined().await;
        repo.set_file_as("bo", group::CLAUDE_MD, "# Friends\nBo's line.\n");
        repo.set_file(group::CLAUDE_MD, "# Friends\nBo's line.\nAna's line.\n");
        let report = bo.engine.sync_all().await.unwrap();
        assert_eq!(
            lines(&report.groups[0].review),
            [(review::Kind::Add, "Ana's line.".into(), "ana".into())]
        );
        assert_eq!(
            bo.apply.file(&id, group::CLAUDE_MD).unwrap(),
            "# Friends\nBo's line.\n"
        );
    }

    #[tokio::test]
    async fn integration_declined_changes_are_not_local_edits() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        let (bo, _dir) = real_device(&world, "bo");
        bo.join(&id).await.unwrap();
        let copy = bo.paths.group_dir(&id).join(group::CLAUDE_MD);
        repo.set_file(group::CLAUDE_MD, "# Friends\nUse tabs.\n");
        bo.sync_all().await.unwrap();
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "# Friends\n");
        assert!(bo.local_edits().unwrap().is_empty());
        bo.decide_claude_md(&id, None, false).await.unwrap();
        bo.sync_all().await.unwrap();
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "# Friends\n");
        assert!(
            bo.local_edits().unwrap().is_empty(),
            "a declined change is not the user's edit"
        );
        let file = bo.paths.group_dir(&id).join(review::FILE);
        let st = ReviewState::load(&file).unwrap().unwrap();
        assert_eq!(st.declined.len(), 1);

        repo.set_file(group::CLAUDE_MD, "# Friends\nUse tabs.\nBe brief.\n");
        bo.sync_all().await.unwrap();
        bo.decide_claude_md(&id, None, true).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&copy).unwrap(),
            "# Friends\nBe brief.\n"
        );
        assert!(bo.local_edits().unwrap().is_empty());
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
        repo.set_owner("bo");
        repo.add_collaborator("ana");
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
            let mut p: Vec<String> = e
                .local_edits()
                .unwrap()
                .into_iter()
                .map(|e| e.path)
                .collect();
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
        assert_eq!(
            std::fs::read_to_string(home.join("agents").join("a.md")).unwrap(),
            "mine\n"
        );
    }

    #[tokio::test]
    async fn integration_plugins_round_trip() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        repo.set_file(
            "plugins/mp/.claude-plugin/marketplace.json",
            r#"{"name":"mp","plugins":[{"name":"p","source":"./p"}]}"#,
        );
        repo.set_file("plugins/mp/p/hooks/hooks.json", "{}\n");
        let (ana, _a) = real_device(&world, "ana");
        let (bo, _b) = real_device(&world, "bo");
        ana.join(&id).await.unwrap();
        let mp = |e: &Engine| e.paths.marketplaces_dir().join("mp");
        assert!(mp(&ana).join("p/hooks/hooks.json").exists());

        // Ana edits a plugin file and adds one; both are local edits until shared.
        std::fs::write(mp(&ana).join("p/hooks/hooks.json"), "{\"v\":2}\n").unwrap();
        std::fs::write(mp(&ana).join("p/hooks/run.ts"), "export {}\n").unwrap();
        // A file over the cap is listed, but never shared.
        std::fs::write(
            mp(&ana).join("p/big.bin"),
            vec![b'x'; group::MAX_PLUGIN_FILE + 1],
        )
        .unwrap();
        let edits = ana.local_edits().unwrap();
        let paths: Vec<&str> = edits.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "plugins/mp/p/big.bin",
                "plugins/mp/p/hooks/hooks.json",
                "plugins/mp/p/hooks/run.ts"
            ]
        );
        let err = ana
            .resolve_edit(&edits[0], EditAction::Share)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("1 MiB"), "{err}");
        for e in &edits[1..] {
            ana.resolve_edit(e, EditAction::Share).await.unwrap();
        }
        assert_eq!(
            repo.file("plugins/mp/p/hooks/hooks.json").unwrap(),
            "{\"v\":2}\n"
        );
        assert_eq!(
            repo.file("plugins/mp/p/hooks/run.ts").unwrap(),
            "export {}\n"
        );
        assert!(repo.file("plugins/mp/p/big.bin").is_none());
        let report = ana.sync_all().await.unwrap();
        assert!(repo.file("plugins/mp/p/big.bin").is_none());
        assert!(report.groups[0]
            .apply
            .skipped
            .iter()
            .any(|n| n.contains("plugins/mp/p/big.bin")));

        // Bo gets the files and the marketplace entry for its own copy.
        assert!(bo.join(&id).await.unwrap().plugins_changed);
        assert_eq!(
            std::fs::read_to_string(mp(&bo).join("p/hooks/run.ts")).unwrap(),
            "export {}\n"
        );
        let settings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bo.paths.claude_home.join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            settings["extraKnownMarketplaces"]["mp"]["source"],
            serde_json::json!({"source": "directory", "path": std::path::absolute(mp(&bo)).unwrap().to_string_lossy()})
        );
        assert!(bo.local_edits().unwrap().is_empty());

        // A later change reaches Bo with the reload hint; a sync with nothing new has none.
        repo.set_file("plugins/mp/p/hooks/run.ts", "export const v = 3\n");
        let report = bo.sync_all().await.unwrap();
        assert!(report.groups[0].apply.plugins_changed);
        let report = bo.sync_all().await.unwrap();
        assert!(!report.groups[0].apply.plugins_changed);

        // Removing the marketplace from the repo removes it on Bo.
        let gone: Changes = repo
            .files()
            .into_keys()
            .filter(|p| p.starts_with("plugins/"))
            .map(|p| (p, None))
            .collect();
        repo.push(&gone, "drop mp");
        let report = bo.sync_all().await.unwrap();
        assert!(report.groups[0].apply.plugins_changed);
        assert!(!mp(&bo).exists());
        let settings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bo.paths.claude_home.join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings.pointer("/extraKnownMarketplaces/mp"), None);

        // Leaving removes Ana's copy, with the reload hint; her never-shared file stays.
        assert!(ana.leave(&id).await.unwrap().plugins_changed);
        assert!(!mp(&ana).join("p/hooks/hooks.json").exists());
        assert!(mp(&ana).join("p/big.bin").exists());
    }

    #[tokio::test]
    async fn integration_secret_blocks_the_push() {
        let world = FakeGitHub::new();
        let (id, repo) = group_repo(&world, &["ana", "bo"]);
        repo.set_owner("bo");
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

    fn last_message(repo: &FakeRepo) -> String {
        repo.commits().last().unwrap().message.clone()
    }

    #[tokio::test]
    async fn revert_a_modified_file() {
        let (_, id, repo, bo) = joined_host().await;
        let target = repo.set_file_as("ana", "agents/a.md", "v2\n").commit;
        let before = commit_count(&repo);

        let plan = bo.engine.revert_commit(&id, &target).await.unwrap();
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].action, RevertAction::Restore);
        assert_eq!(commit_count(&repo), before + 1, "a new commit, no rewrite");
        assert_eq!(repo.file("agents/a.md").unwrap(), "base\n");
        assert_eq!(
            last_message(&repo),
            format!("bo@pc: revert {target} (update agents/a.md)")
        );
        assert_eq!(bo.apply.file(&id, "agents/a.md").unwrap(), "base\n");
    }

    #[tokio::test]
    async fn revert_an_added_file_deletes_it() {
        let (_, id, repo, bo) = joined_host().await;
        let target = repo.set_file_as("ana", "agents/new.md", "hi\n").commit;
        let plan = bo.engine.revert_commit(&id, &target).await.unwrap();
        assert_eq!(plan.files[0].action, RevertAction::Delete);
        assert_eq!(repo.file("agents/new.md"), None);
        assert_eq!(repo.file("agents/a.md").unwrap(), "base\n", "others stay");
    }

    #[tokio::test]
    async fn revert_a_deleted_file_restores_it() {
        let (_, id, repo, bo) = joined_host().await;
        let target = repo.remove_file("agents/a.md").commit;
        let plan = bo.engine.revert_commit(&id, &target).await.unwrap();
        assert_eq!(plan.files[0].action, RevertAction::Recreate);
        assert_eq!(repo.file("agents/a.md").unwrap(), "base\n");
    }

    #[tokio::test]
    async fn revert_is_host_only() {
        let (_, id, repo, bo) = joined().await;
        let target = repo.set_file_as("ana", "agents/a.md", "v2\n").commit;
        let before = commit_count(&repo);
        let err = bo.engine.revert_commit(&id, &target).await.unwrap_err();
        assert!(matches!(err, Error::HostOnly { .. }), "{err}");
        let err = bo.engine.revert_preview(&id, &target).await.unwrap_err();
        assert!(matches!(err, Error::HostOnly { .. }), "{err}");
        assert_eq!(commit_count(&repo), before);
        assert_eq!(repo.file("agents/a.md").unwrap(), "v2\n");
    }

    #[tokio::test]
    async fn revert_refuses_a_file_that_changed_again() {
        let (_, id, repo, bo) = joined_host().await;
        let target = repo.set_file_as("ana", "agents/a.md", "v2\n").commit;
        repo.set_file_as("cy", "agents/a.md", "v3\n");
        let later = repo.current_head().commit;
        let before = commit_count(&repo);

        let plan = bo.engine.revert_preview(&id, &target).await.unwrap();
        assert!(plan.files[0].blocked.is_some());
        let err = bo.engine.revert_commit(&id, &target).await.unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains(&format!(
                "agents/a.md changed again after this commit, in {later} by cy"
            )),
            "{text}"
        );
        assert_eq!(commit_count(&repo), before, "nothing committed");
        assert_eq!(repo.file("agents/a.md").unwrap(), "v3\n");

        // Reverting the later commit first, then this one, works.
        bo.engine.revert_commit(&id, &later).await.unwrap();
        assert_eq!(repo.file("agents/a.md").unwrap(), "v2\n");
    }

    #[tokio::test]
    async fn revert_retries_after_a_push_race() {
        let (_, id, repo, bo) = joined_host().await;
        let target = repo.set_file_as("ana", "agents/a.md", "v2\n").commit;
        let mut once = true;
        repo.before_commit(move |r| {
            if std::mem::take(&mut once) {
                r.set_file_as("cy", "skills/x/SKILL.md", "x\n");
            }
        });
        bo.engine.revert_commit(&id, &target).await.unwrap();
        assert_eq!(repo.file("agents/a.md").unwrap(), "base\n");
        assert_eq!(repo.file("skills/x/SKILL.md").unwrap(), "x\n");
    }
}
