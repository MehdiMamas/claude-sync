//! The facade the app uses. Everything the UI shows comes from here.
use crate::github::{GitHub, Invitation};
use crate::watch::{EditAction, LocalEdit};
use crate::{GroupId, Paths};
use serde::Serialize;

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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictInfo {
    pub path: String,
    /// Where the losing local copy was saved.
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub at_unix: u64,
}

pub struct Engine {
    pub paths: Paths,
    pub gh: GitHub,
    pub login: String,
    pub device: String,
}

impl Engine {
    pub fn new(paths: Paths, gh: GitHub, login: String, device: String) -> Self {
        Self { paths, gh, login, device }
    }
    /// One pass over every joined group: pull, merge, apply, push local changes, reconcile members
    /// if this user is the host, detect lost access. Errors per group go in the report, not `Err`.
    pub async fn sync_all(&self) -> crate::Result<SyncReport> {
        todo!()
    }
    /// Same, but only for groups whose remote head changed (ETag) or that have local edits to push.
    /// The 60 s loop calls this.
    pub async fn sync_if_changed(&self) -> crate::Result<SyncReport> {
        todo!()
    }
    pub async fn groups(&self) -> crate::Result<Vec<GroupStatus>> {
        todo!()
    }
    /// Create a private group repo (default name `claude-sync-data`) and join it.
    pub async fn create_group(&self, _name: &str, _repo: &str) -> crate::Result<GroupStatus> {
        todo!()
    }
    /// Pending invitations to repos with the `claude-sync-group` topic.
    pub async fn invitations(&self) -> crate::Result<Vec<Invitation>> {
        todo!()
    }
    /// Accept and set the group up on this device.
    pub async fn accept_invitation(&self, _invitation_id: u64) -> crate::Result<GroupStatus> {
        todo!()
    }
    /// Join a group repo this user already has access to.
    pub async fn join(&self, _id: &GroupId) -> crate::Result<GroupStatus> {
        todo!()
    }
    /// Group repos this user can access but has not joined on this device.
    pub async fn discover(&self) -> crate::Result<Vec<GroupId>> {
        todo!()
    }
    pub async fn add_member(&self, _id: &GroupId, _login: &str) -> crate::Result<()> {
        todo!()
    }
    pub async fn remove_member(&self, _id: &GroupId, _login: &str) -> crate::Result<()> {
        todo!()
    }
    /// Leave on this device: remove managed content. Does not change membership.
    pub async fn leave(&self, _id: &GroupId) -> crate::Result<()> {
        todo!()
    }
    pub fn local_edits(&self) -> crate::Result<Vec<LocalEdit>> {
        todo!()
    }
    pub async fn resolve_edit(&self, _edit: &LocalEdit, _action: EditAction) -> crate::Result<()> {
        todo!()
    }
    /// Share a local skill folder (`~/.claude/skills/<name>`) with a group.
    pub async fn share_skill(&self, _id: &GroupId, _skill: &str) -> crate::Result<()> {
        todo!()
    }
    pub async fn recent_commits(&self, _id: &GroupId, _limit: usize) -> crate::Result<Vec<CommitInfo>> {
        todo!()
    }
}
