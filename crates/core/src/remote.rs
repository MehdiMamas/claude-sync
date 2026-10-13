//! The engine's only view of a group repo (§6). `GitHubRepo` talks to the REST API;
//! `fake::FakeRepo` is in-memory for tests (and is public so the app's tests can use it).
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub commit: String,
    pub tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collaborator {
    Member(String),
    Invited { login: String, invitation_id: u64 },
}

#[async_trait]
pub trait RemoteRepo: Send + Sync {
    fn id(&self) -> &crate::GroupId;
    /// `None` when unchanged since the last call (ETag `304`).
    async fn head_if_changed(&self) -> crate::Result<Option<Head>>;
    async fn head(&self) -> crate::Result<Head>;
    /// All allowlisted files at a commit.
    async fn files_at(&self, commit: &str) -> crate::Result<crate::FileSet>;
    /// Commit `changes` (`None` = delete) on top of `parent` and move the branch,
    /// never forcing. `Err(Error::PushRace)` when the branch moved.
    async fn commit(
        &self,
        parent: &Head,
        changes: &std::collections::BTreeMap<String, Option<Vec<u8>>>,
        message: &str,
    ) -> crate::Result<Head>;
    async fn is_private(&self) -> crate::Result<bool>;
    async fn owner_login(&self) -> crate::Result<String>;
    async fn collaborators(&self) -> crate::Result<Vec<Collaborator>>;
    async fn invite(&self, login: &str) -> crate::Result<()>;
    async fn remove_collaborator(&self, login: &str) -> crate::Result<()>;
    async fn cancel_invitation(&self, invitation_id: u64) -> crate::Result<()>;
}

pub struct GitHubRepo {
    pub gh: crate::github::GitHub,
    pub id: crate::GroupId,
}

pub mod fake {
    //! In-memory `RemoteRepo`.
}
