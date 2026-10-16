//! The engine's only view of a group repo (§6). `GitHubRepo` talks to the REST API;
//! `fake::FakeRepo` is in-memory for tests (and is public so the app's tests can use it).
use crate::engine::CommitInfo;
use crate::github::{GitHub, Invitation};
use crate::{Error, FileSet, GroupId};
use async_trait::async_trait;
use base64::Engine as _;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

pub mod fake;

/// `Commit` changes: path -> new content, `None` = delete.
pub type Changes = BTreeMap<String, Option<Vec<u8>>>;
/// Which repo paths `files_matching` reads.
pub type PathFilter = fn(&str) -> bool;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct Head {
    pub commit: String,
    pub tree: String,
}

impl Head {
    /// The head of a repo with no commits yet.
    pub fn empty() -> Self {
        Self {
            commit: String::new(),
            tree: String::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.commit.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collaborator {
    Member(String),
    Invited { login: String, invitation_id: u64 },
}

/// Name of the ruleset that keeps everyone but the host from pushing to a group.
pub const HOST_LOCK_NAME: &str = "claude-sync: only the host updates the group";

#[async_trait]
pub trait RemoteRepo: Send + Sync {
    fn id(&self) -> &crate::GroupId;
    /// `None` when unchanged since the last call (ETag `304`).
    async fn head_if_changed(&self) -> crate::Result<Option<Head>>;
    /// `Head::empty()` for a repo with no commits.
    async fn head(&self) -> crate::Result<Head>;
    /// All group-allowlisted files at a commit.
    async fn files_at(&self, commit: &str) -> crate::Result<crate::FileSet> {
        self.files_matching(commit, crate::group::is_allowlisted)
            .await
    }
    /// The files at a commit whose path passes `keep` (the personal repo has its own list).
    async fn files_matching(&self, commit: &str, keep: PathFilter)
        -> crate::Result<crate::FileSet>;
    /// Commit `changes` (`None` = delete) on top of `parent` and move the branch,
    /// never forcing. `Err(Error::PushRace)` when the branch moved.
    /// `parent` may be `Head::empty()` to make the first commit of an empty repo.
    async fn commit(
        &self,
        parent: &Head,
        changes: &std::collections::BTreeMap<String, Option<Vec<u8>>>,
        message: &str,
    ) -> crate::Result<Head>;
    async fn is_private(&self) -> crate::Result<bool>;
    /// The signed-in account may push to the repo (not a read-only collaborator).
    async fn can_push(&self) -> crate::Result<bool> {
        Ok(true)
    }
    async fn owner_login(&self) -> crate::Result<String>;
    /// The repo's name right now. It differs from `id()` after a rename or transfer, which
    /// GitHub answers with a redirect from the old name (§10).
    async fn current_id(&self) -> crate::Result<crate::GroupId> {
        Ok(self.id().clone())
    }
    /// Ask GitHub to transfer the repo to `new_owner`. The new owner has to accept by email
    /// before anything changes.
    async fn transfer(&self, new_owner: &str) -> crate::Result<()>;
    async fn collaborators(&self) -> crate::Result<Vec<Collaborator>>;
    async fn invite(&self, login: &str) -> crate::Result<()>;
    /// Make sure GitHub refuses pushes to the default branch from everyone but repo admins
    /// (a ruleset named [`HOST_LOCK_NAME`]). Host only; may be refused by the plan.
    async fn ensure_host_lock(&self) -> crate::Result<()>;
    async fn remove_collaborator(&self, login: &str) -> crate::Result<()>;
    async fn cancel_invitation(&self, invitation_id: u64) -> crate::Result<()>;
    /// Newest first.
    async fn recent_commits(&self, _limit: usize) -> crate::Result<Vec<CommitInfo>> {
        Ok(Vec::new())
    }
    /// The commits after `from` up to `to`, oldest first, with their GitHub author login
    /// (used to tell this account's `CLAUDE.md` changes from other members', §7a).
    async fn commits_between(&self, _from: &str, _to: &str) -> crate::Result<Vec<CommitInfo>> {
        Err(Error::Invalid("commit history is not available".into()))
    }
    /// One file at a commit; `None` when the commit does not have it.
    async fn file_at(&self, _commit: &str, _path: &str) -> crate::Result<Option<Vec<u8>>> {
        Err(Error::Invalid("file history is not available".into()))
    }
}

/// Account-level calls and a way to open repos: what the engine needs beyond one repo.
/// `GitHubRemotes` in the app, `fake::FakeAccount` in tests.
#[async_trait]
pub trait RemoteFactory: Send + Sync {
    /// The same repo object for the same id, so ETag state survives between passes.
    fn open(&self, id: &GroupId) -> Arc<dyn RemoteRepo>;
    /// An empty private repo with the group topic on the signed-in account.
    async fn create_repo(&self, name: &str) -> crate::Result<GroupId>;
    /// Pending invitations to group repos.
    async fn invitations(&self) -> crate::Result<Vec<Invitation>>;
    /// Accept; returns the repo.
    async fn accept_invitation(&self, id: u64) -> crate::Result<GroupId>;
    /// Group repos this user can access.
    async fn group_repos(&self) -> crate::Result<Vec<GroupId>>;
    /// An empty private repo without the group topic (the personal memory repo), so it is
    /// never discovered or offered as a group.
    async fn create_personal_repo(&self, name: &str) -> crate::Result<GroupId>;
}

pub struct GitHubRemotes {
    gh: GitHub,
    repos: Mutex<HashMap<GroupId, Arc<GitHubRepo>>>,
}

impl GitHubRemotes {
    pub fn new(gh: GitHub) -> Self {
        Self {
            gh,
            repos: Mutex::default(),
        }
    }
}

#[async_trait]
impl RemoteFactory for GitHubRemotes {
    fn open(&self, id: &GroupId) -> Arc<dyn RemoteRepo> {
        let mut repos = self.repos.lock().unwrap();
        repos
            .entry(id.clone())
            .or_insert_with(|| Arc::new(GitHubRepo::new(self.gh.clone(), id.clone())))
            .clone()
    }
    async fn create_repo(&self, name: &str) -> crate::Result<GroupId> {
        self.gh.create_group_repo(name).await
    }
    async fn invitations(&self) -> crate::Result<Vec<Invitation>> {
        self.gh.invitations().await
    }
    async fn accept_invitation(&self, id: u64) -> crate::Result<GroupId> {
        self.gh.accept_invitation(id).await
    }
    async fn group_repos(&self) -> crate::Result<Vec<GroupId>> {
        self.gh.group_repos().await
    }
    async fn create_personal_repo(&self, name: &str) -> crate::Result<GroupId> {
        self.gh.create_private_repo(name).await
    }
}

pub struct GitHubRepo {
    pub gh: crate::github::GitHub,
    pub id: crate::GroupId,
    info: Mutex<Option<RepoInfo>>,
    etag: Mutex<Option<String>>,
    /// Heads this object read or made, newest last (at most `KNOWN_HEADS`).
    heads: Mutex<Vec<Head>>,
    /// Blob contents by SHA; blobs never change, so this only grows with the repo.
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

#[derive(Debug, Clone, Deserialize)]
struct RepoInfo {
    /// `owner/repo` as GitHub names it now.
    #[serde(default)]
    full_name: Option<String>,
    default_branch: String,
    private: bool,
    owner: Owner,
    /// The signed-in account's rights; GitHub sends them to an authenticated caller.
    #[serde(default)]
    permissions: Option<Permissions>,
}

#[derive(Debug, Clone, Deserialize)]
struct Permissions {
    #[serde(default)]
    push: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Owner {
    login: String,
}

#[derive(Deserialize)]
struct Sha {
    sha: String,
}

#[derive(Deserialize)]
struct CommitReply {
    sha: String,
    commit: CommitBody,
}

#[derive(Deserialize)]
struct CommitBody {
    tree: Sha,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// A `422` from moving a branch because it moved first: "Update is not a fast forward", or
/// "Reference cannot be updated" when the other push lands at the same moment.
fn is_ref_race(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("fast forward") || m.contains("cannot be updated")
}

/// Branch heads remembered per repo to spot stale reads.
const KNOWN_HEADS: usize = 64;

/// GitHub answers `409 Git Repository is empty` for commits and refs of an empty repo.
fn is_empty_repo(e: &Error) -> bool {
    matches!(e, Error::GitHub { status: 409, .. })
}

impl GitHubRepo {
    pub fn new(gh: GitHub, id: GroupId) -> Self {
        Self {
            gh,
            id,
            info: Mutex::default(),
            etag: Mutex::default(),
            heads: Mutex::default(),
            blobs: Mutex::default(),
        }
    }

    fn path(&self, rest: &str) -> String {
        format!("/repos/{}/{}{rest}", self.id.owner, self.id.repo)
    }

    async fn info(&self) -> crate::Result<RepoInfo> {
        if let Some(info) = self.info.lock().unwrap().clone() {
            return Ok(info);
        }
        let info: RepoInfo = self.gh.get(&self.path("")).await?;
        *self.info.lock().unwrap() = Some(info.clone());
        Ok(info)
    }

    fn remember(&self, head: &Head) {
        let mut heads = self.heads.lock().unwrap();
        if heads.last() != Some(head) {
            heads.push(head.clone());
            let extra = heads.len().saturating_sub(KNOWN_HEADS);
            heads.drain(..extra);
        }
    }

    /// GitHub can answer a read with a head older than one this object already saw (replica
    /// lag right after a push). Branches only move forward here, so an older known head, or
    /// "empty" after a commit, is stale: returns the newest known head instead.
    fn newer_than(&self, read: &Head) -> Option<Head> {
        let heads = self.heads.lock().unwrap();
        let newest = heads.last()?;
        let stale = if read.is_empty() {
            !newest.is_empty()
        } else {
            newest != read && heads.iter().any(|h| h.commit == read.commit)
        };
        stale.then(|| newest.clone())
    }

    async fn tree_of(&self, commit: &str) -> crate::Result<String> {
        let known = self
            .heads
            .lock()
            .unwrap()
            .iter()
            .find(|h| h.commit == commit)
            .map(|h| h.tree.clone());
        if let Some(tree) = known {
            return Ok(tree);
        }
        let c: CommitBody = self
            .gh
            .get(&self.path(&format!("/git/commits/{commit}")))
            .await?;
        Ok(c.tree.sha)
    }

    async fn blob(&self, sha: &str) -> crate::Result<Vec<u8>> {
        if let Some(b) = self.blobs.lock().unwrap().get(sha) {
            return Ok(b.clone());
        }
        #[derive(Deserialize)]
        struct Blob {
            content: String,
            encoding: String,
        }
        let blob: Blob = self
            .gh
            .get(&self.path(&format!("/git/blobs/{sha}")))
            .await?;
        let bytes = match blob.encoding.as_str() {
            "base64" => {
                let clean: String = blob
                    .content
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                b64()
                    .decode(clean)
                    .map_err(|e| Error::Invalid(format!("blob {sha}: {e}")))?
            }
            _ => blob.content.into_bytes(),
        };
        self.blobs.lock().unwrap().insert(sha.into(), bytes.clone());
        Ok(bytes)
    }

    /// First commit of an empty repo. The Git Data API refuses to create blobs until the repo
    /// has a commit, so one file goes through the Contents API.
    async fn first_commit(&self, path: &str, content: &[u8], message: &str) -> crate::Result<Head> {
        // The reply carries a git commit object (`tree.sha`), not a REST commit (`commit.tree.sha`).
        #[derive(Deserialize)]
        struct GitCommit {
            sha: String,
            tree: Sha,
        }
        #[derive(Deserialize)]
        struct Reply {
            commit: GitCommit,
        }
        let body = json!({ "message": message, "content": b64().encode(content) });
        let reply: Reply = match self
            .gh
            .send(Method::PUT, &self.path(&format!("/contents/{path}")), &body)
            .await
        {
            // The file exists: someone else made the first commit.
            Err(Error::GitHub {
                status: 409 | 422, ..
            }) => return Err(Error::PushRace),
            r => r?,
        };
        Ok(Head {
            commit: reply.commit.sha,
            tree: reply.commit.tree.sha,
        })
    }

    /// The branch head. `conditional`: `None` when unchanged since the last read (ETag `304`).
    /// Every read keeps the ETag, so after a pass that called `head()` the next
    /// `head_if_changed` is a free `304`.
    async fn fetch_head(&self, conditional: bool) -> crate::Result<Option<Head>> {
        let branch = self.info().await?.default_branch;
        let path = self.path(&format!("/commits/{branch}"));
        let etag = conditional
            .then(|| self.etag.lock().unwrap().clone())
            .flatten();
        let (read, etag) = match self
            .gh
            .get_if_changed::<CommitReply>(&path, etag.as_deref())
            .await
        {
            Ok(crate::github::Conditional::NotModified) => return Ok(None),
            Ok(crate::github::Conditional::Modified { value, etag }) => {
                let head = Head {
                    commit: value.sha,
                    tree: value.commit.tree.sha,
                };
                (head, etag)
            }
            Err(e) if is_empty_repo(&e) => (Head::empty(), None),
            Err(e) => return Err(e),
        };
        // A stale read is "unchanged"; its ETag is not kept so the next check asks again.
        if let Some(newest) = self.newer_than(&read) {
            return Ok((!conditional).then_some(newest));
        }
        let seen = self.heads.lock().unwrap().last() == Some(&read);
        *self.etag.lock().unwrap() = etag;
        self.remember(&read);
        // An empty repo has no ETag: compare with the last read instead.
        Ok((!conditional || !read.is_empty() || !seen).then_some(read))
    }
}

#[async_trait]
impl RemoteRepo for GitHubRepo {
    fn id(&self) -> &GroupId {
        &self.id
    }

    async fn head_if_changed(&self) -> crate::Result<Option<Head>> {
        self.fetch_head(true).await
    }

    async fn head(&self) -> crate::Result<Head> {
        Ok(self.fetch_head(false).await?.expect("unconditional"))
    }

    async fn files_matching(&self, commit: &str, keep: PathFilter) -> crate::Result<FileSet> {
        if commit.is_empty() {
            return Ok(FileSet::new());
        }
        #[derive(Deserialize)]
        struct Entry {
            path: String,
            #[serde(rename = "type")]
            kind: String,
            sha: String,
        }
        #[derive(Deserialize)]
        struct Tree {
            tree: Vec<Entry>,
            #[serde(default)]
            truncated: bool,
        }
        let tree_sha = self.tree_of(commit).await?;
        let tree: Tree = self
            .gh
            .get(&self.path(&format!("/git/trees/{tree_sha}?recursive=1")))
            .await?;
        if tree.truncated {
            return Err(Error::Invalid(format!("{} is too large to sync", self.id)));
        }
        let mut files = FileSet::new();
        for e in tree.tree {
            if e.kind == "blob" && keep(&e.path) {
                let bytes = self.blob(&e.sha).await?;
                files.insert(e.path, bytes);
            }
        }
        Ok(files)
    }

    async fn commit(&self, parent: &Head, changes: &Changes, message: &str) -> crate::Result<Head> {
        let mut parent = parent.clone();
        let mut changes = changes.clone();
        if parent.is_empty() {
            let first = changes
                .iter()
                .find_map(|(p, c)| c.as_ref().map(|c| (p.clone(), c.clone())));
            let Some((path, content)) = first else {
                return Err(Error::Invalid(
                    "the first commit of a repo needs a file".into(),
                ));
            };
            changes.remove(&path);
            changes.retain(|_, c| c.is_some());
            // When a second commit follows, name this one apart so history doesn't show
            // the same message twice (#44).
            let first_message = if changes.is_empty() {
                message.to_string()
            } else {
                format!("{message} (start the repo)")
            };
            parent = self.first_commit(&path, &content, &first_message).await?;
            self.remember(&parent);
            if changes.is_empty() {
                return Ok(parent);
            }
        }
        let mut entries = Vec::new();
        for (path, content) in &changes {
            let sha = match content {
                Some(bytes) => {
                    let body = json!({ "content": b64().encode(bytes), "encoding": "base64" });
                    let blob: Sha = self
                        .gh
                        .send(Method::POST, &self.path("/git/blobs"), &body)
                        .await?;
                    self.blobs
                        .lock()
                        .unwrap()
                        .insert(blob.sha.clone(), bytes.clone());
                    Value::String(blob.sha)
                }
                None => Value::Null,
            };
            entries.push(json!({ "path": path, "mode": "100644", "type": "blob", "sha": sha }));
        }
        let tree: Sha = self
            .gh
            .send(
                Method::POST,
                &self.path("/git/trees"),
                &json!({ "base_tree": parent.tree, "tree": entries }),
            )
            .await?;
        let body = json!({ "message": message, "tree": tree.sha, "parents": [parent.commit] });
        let commit: Sha = self
            .gh
            .send(Method::POST, &self.path("/git/commits"), &body)
            .await?;
        let branch = self.info().await?.default_branch;
        let body = json!({ "sha": commit.sha, "force": false });
        match self
            .gh
            .call(
                Method::PATCH,
                &self.path(&format!("/git/refs/heads/{branch}")),
                Some(&body),
            )
            .await
        {
            Err(Error::GitHub {
                status: 422,
                message,
            }) if is_ref_race(&message) => return Err(Error::PushRace),
            r => r?,
        }
        let head = Head {
            commit: commit.sha,
            tree: tree.sha,
        };
        self.remember(&head);
        Ok(head)
    }

    async fn is_private(&self) -> crate::Result<bool> {
        Ok(self.info().await?.private)
    }

    async fn can_push(&self) -> crate::Result<bool> {
        Ok(self.info().await?.permissions.is_none_or(|p| p.push))
    }

    async fn owner_login(&self) -> crate::Result<String> {
        Ok(self.info().await?.owner.login)
    }

    async fn current_id(&self) -> crate::Result<GroupId> {
        // Never from the cache: it is the one thing that changes under the old name. The GET
        // follows GitHub's redirect for a renamed or transferred repo.
        let info: RepoInfo = self.gh.get(&self.path("")).await?;
        let id = info
            .full_name
            .as_deref()
            .and_then(GroupId::parse)
            .unwrap_or_else(|| self.id.clone());
        *self.info.lock().unwrap() = Some(info);
        Ok(id)
    }

    async fn transfer(&self, new_owner: &str) -> crate::Result<()> {
        // POST is not redirected, so this always goes to the name the app holds; the engine
        // resolves the current name first.
        let body = json!({ "new_owner": new_owner });
        self.gh
            .call(Method::POST, &self.path("/transfer"), Some(&body))
            .await
    }

    async fn collaborators(&self) -> crate::Result<Vec<Collaborator>> {
        #[derive(Deserialize)]
        struct Inv {
            id: u64,
            invitee: Option<Owner>,
        }
        let members: Vec<Owner> = self
            .gh
            .get_pages(&self.path("/collaborators?per_page=100"))
            .await?;
        let invites: Vec<Inv> = self
            .gh
            .get_pages(&self.path("/invitations?per_page=100"))
            .await?;
        Ok(members
            .into_iter()
            .map(|m| Collaborator::Member(m.login))
            .chain(invites.into_iter().filter_map(|i| {
                Some(Collaborator::Invited {
                    login: i.invitee?.login,
                    invitation_id: i.id,
                })
            }))
            .collect())
    }

    async fn ensure_host_lock(&self) -> crate::Result<()> {
        #[derive(Deserialize)]
        struct Ruleset {
            name: String,
        }
        let have: Vec<Ruleset> = self.gh.get(&self.path("/rulesets")).await?;
        if have.iter().any(|r| r.name == HOST_LOCK_NAME) {
            return Ok(());
        }
        let body = json!({
            "name": HOST_LOCK_NAME,
            "target": "branch",
            "enforcement": "active",
            "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
            "rules": [{"type": "update"}, {"type": "deletion"}, {"type": "non_fast_forward"}],
            // Repository role 5 is admin.
            "bypass_actors": [{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always"}],
        });
        self.gh
            .call(Method::POST, &self.path("/rulesets"), Some(&body))
            .await
    }

    async fn invite(&self, login: &str) -> crate::Result<()> {
        // Members read; the ruleset (not this) is what stops personal repos from letting
        // collaborators push, since GitHub ignores the permission there.
        let body = json!({ "permission": "pull" });
        self.gh
            .call(
                Method::PUT,
                &self.path(&format!("/collaborators/{login}")),
                Some(&body),
            )
            .await
    }

    async fn remove_collaborator(&self, login: &str) -> crate::Result<()> {
        self.gh
            .call(
                Method::DELETE,
                &self.path(&format!("/collaborators/{login}")),
                None,
            )
            .await
    }

    async fn cancel_invitation(&self, invitation_id: u64) -> crate::Result<()> {
        self.gh
            .call(
                Method::DELETE,
                &self.path(&format!("/invitations/{invitation_id}")),
                None,
            )
            .await
    }

    async fn recent_commits(&self, limit: usize) -> crate::Result<Vec<CommitInfo>> {
        let branch = self.info().await?.default_branch;
        let path = self.path(&format!(
            "/commits?sha={branch}&per_page={}",
            limit.clamp(1, 100)
        ));
        let items: Vec<CommitItem> = match self.gh.get(&path).await {
            Err(e) if is_empty_repo(&e) => return Ok(Vec::new()),
            r => r?,
        };
        Ok(items.into_iter().map(CommitItem::info).collect())
    }

    async fn commits_between(&self, from: &str, to: &str) -> crate::Result<Vec<CommitInfo>> {
        #[derive(Deserialize)]
        struct Compare {
            commits: Vec<CommitItem>,
        }
        let c: Compare = self
            .gh
            .get(&self.path(&format!("/compare/{from}...{to}")))
            .await?;
        Ok(c.commits.into_iter().map(CommitItem::info).collect())
    }

    async fn file_at(&self, commit: &str, path: &str) -> crate::Result<Option<Vec<u8>>> {
        #[derive(Deserialize)]
        struct Contents {
            content: String,
            encoding: String,
        }
        let url = self.path(&format!("/contents/{path}?ref={commit}"));
        match self.gh.get::<Contents>(&url).await {
            Err(Error::GitHub { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
            Ok(c) if c.encoding == "base64" => decode_base64(&c.content).map(Some),
            Ok(_) => Err(Error::Invalid(format!(
                "{path} at {commit} is too large to read"
            ))),
        }
    }
}

/// A commit as `/commits` and `/compare` list it.
#[derive(Deserialize)]
struct CommitItem {
    sha: String,
    commit: CommitDetail,
    author: Option<Owner>,
}

#[derive(Deserialize)]
struct CommitDetail {
    message: String,
    author: Option<CommitPerson>,
}

#[derive(Deserialize)]
struct CommitPerson {
    name: Option<String>,
    date: Option<String>,
}

impl CommitItem {
    /// The GitHub login when GitHub knows the author, else the git author name.
    fn info(self) -> CommitInfo {
        let person = self.commit.author;
        CommitInfo {
            sha: self.sha,
            message: self.commit.message,
            author: self
                .author
                .map(|a| a.login)
                .or_else(|| person.as_ref().and_then(|p| p.name.clone()))
                .unwrap_or_default(),
            at_unix: person
                .and_then(|p| p.date)
                .and_then(|d| crate::state::parse_rfc3339(&d))
                .unwrap_or(0),
        }
    }
}

/// GitHub's base64 (wrapped in newlines).
fn decode_base64(content: &str) -> crate::Result<Vec<u8>> {
    let clean: String = content.chars().filter(|c| !c.is_whitespace()).collect();
    b64()
        .decode(clean)
        .map_err(|e| Error::Invalid(format!("bad base64 from GitHub: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wiremock::matchers::{body_json, body_partial_json, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup() -> (MockServer, GitHubRepo) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "default_branch": "main", "private": true, "owner": {"login": "o"}
            })))
            .mount(&server)
            .await;
        let gh =
            GitHub::with_api("tok".into(), server.uri()).with_retry_delay(Duration::from_millis(1));
        let repo = GitHubRepo::new(gh, GroupId::new("o", "r"));
        (server, repo)
    }

    fn commit_json(sha: &str, tree: &str) -> Value {
        json!({ "sha": sha, "commit": { "tree": { "sha": tree } } })
    }

    fn head(c: &str, t: &str) -> Head {
        Head {
            commit: c.into(),
            tree: t.into(),
        }
    }

    #[tokio::test]
    async fn head_uses_etags() {
        let (server, repo) = setup().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/commits/main"))
            .and(header("if-none-match", "\"e1\""))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/commits/main"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "\"e1\"")
                    .set_body_json(commit_json("c1", "t1")),
            )
            .mount(&server)
            .await;
        assert_eq!(
            repo.head_if_changed().await.unwrap(),
            Some(head("c1", "t1"))
        );
        assert_eq!(repo.head_if_changed().await.unwrap(), None);
        assert_eq!(repo.head().await.unwrap(), head("c1", "t1"));
        assert!(repo.is_private().await.unwrap());
        assert_eq!(repo.owner_login().await.unwrap(), "o");
    }

    #[tokio::test]
    async fn head_keeps_the_etag_for_the_next_check() {
        let (server, repo) = setup().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/commits/main"))
            .and(header("if-none-match", "\"e1\""))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/commits/main"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "\"e1\"")
                    .set_body_json(commit_json("c1", "t1")),
            )
            .mount(&server)
            .await;
        assert_eq!(repo.head().await.unwrap(), head("c1", "t1"));
        assert_eq!(repo.head_if_changed().await.unwrap(), None);
    }

    #[tokio::test]
    async fn empty_repo_has_an_empty_head() {
        let (server, repo) = setup().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/commits/main"))
            .respond_with(
                ResponseTemplate::new(409)
                    .set_body_json(json!({"message": "Git Repository is empty."})),
            )
            .mount(&server)
            .await;
        assert_eq!(repo.head_if_changed().await.unwrap(), Some(Head::empty()));
        assert_eq!(repo.head_if_changed().await.unwrap(), None);
        assert_eq!(repo.head().await.unwrap(), Head::empty());
        assert!(repo.files_at("").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn lost_access_is_a_404() {
        let (server, _) = setup().await;
        let gh = GitHub::with_api("tok".into(), server.uri());
        let gone = GitHubRepo::new(gh, GroupId::new("o", "gone"));
        assert!(matches!(
            gone.head().await,
            Err(Error::GitHub { status: 404, .. })
        ));
    }

    #[tokio::test]
    async fn can_push_follows_the_repos_permissions() {
        let (server, repo) = setup().await;
        // No `permissions` in the answer: assume yes, the push itself says otherwise.
        assert!(repo.can_push().await.unwrap());
        Mock::given(method("GET"))
            .and(path("/repos/o/ro"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "default_branch": "main", "private": true, "owner": {"login": "o"},
                "permissions": {"admin": false, "push": false, "pull": true}
            })))
            .mount(&server)
            .await;
        let gh = GitHub::with_api("tok".into(), server.uri());
        let read_only = GitHubRepo::new(gh, GroupId::new("o", "ro"));
        assert!(!read_only.can_push().await.unwrap());
    }

    #[tokio::test]
    async fn files_at_reads_allowlisted_blobs_once() {
        let (server, repo) = setup().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/git/commits/c1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"sha": "c1", "tree": {"sha": "t1"}})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/git/trees/t1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "truncated": false,
                "tree": [
                    {"path": "CLAUDE.md", "type": "blob", "sha": "b1"},
                    {"path": "README.md", "type": "blob", "sha": "b2"},
                    {"path": "skills", "type": "tree", "sha": "t2"},
                    {"path": "skills/x/SKILL.md", "type": "blob", "sha": "b1"},
                    {"path": ".github/workflows/x.yml", "type": "blob", "sha": "b3"},
                ]
            })))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/git/blobs/b1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"content": "aGVs\nbG8=\n", "encoding": "base64"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let want = FileSet::from([
            ("CLAUDE.md".to_string(), b"hello".to_vec()),
            ("skills/x/SKILL.md".to_string(), b"hello".to_vec()),
        ]);
        assert_eq!(repo.files_at("c1").await.unwrap(), want);
        assert_eq!(repo.files_at("c1").await.unwrap(), want);
    }

    #[tokio::test]
    async fn truncated_trees_are_refused() {
        let (server, repo) = setup().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/git/commits/c1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"sha": "c1", "tree": {"sha": "t1"}})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/git/trees/t1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"truncated": true, "tree": []})),
            )
            .mount(&server)
            .await;
        assert!(matches!(repo.files_at("c1").await, Err(Error::Invalid(_))));
    }

    async fn mount_commit_steps(server: &MockServer, parent: &Head) {
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/blobs"))
            .and(body_json(json!({"content": "bmV3", "encoding": "base64"})))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "b9"})))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/trees"))
            .and(body_json(json!({
                "base_tree": parent.tree,
                "tree": [
                    {"path": "CLAUDE.md", "mode": "100644", "type": "blob", "sha": "b9"},
                    {"path": "agents/old.md", "mode": "100644", "type": "blob", "sha": null},
                ]
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "t9"})))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/commits"))
            .and(body_json(json!({"message": "me@pc: update CLAUDE.md", "tree": "t9", "parents": [parent.commit]})))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "c9"})))
            .mount(server)
            .await;
    }

    fn changes() -> Changes {
        BTreeMap::from([
            ("CLAUDE.md".into(), Some(b"new".to_vec())),
            ("agents/old.md".into(), None),
        ])
    }

    #[tokio::test]
    async fn commit_moves_the_branch_without_force() {
        let (server, repo) = setup().await;
        let parent = head("c1", "t1");
        mount_commit_steps(&server, &parent).await;
        Mock::given(method("PATCH"))
            .and(path("/repos/o/r/git/refs/heads/main"))
            .and(body_json(json!({"sha": "c9", "force": false})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ref": "refs/heads/main"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let new = repo
            .commit(&parent, &changes(), "me@pc: update CLAUDE.md")
            .await
            .unwrap();
        assert_eq!(new, head("c9", "t9"));
    }

    #[tokio::test]
    async fn a_lagging_read_never_goes_back_to_an_older_head() {
        let (server, repo) = setup().await;
        let get_head = |c: &str, t: &str| {
            Mock::given(method("GET"))
                .and(path("/repos/o/r/commits/main"))
                .respond_with(ResponseTemplate::new(200).set_body_json(commit_json(c, t)))
        };
        get_head("c1", "t1").mount(&server).await;
        let parent = repo.head().await.unwrap();
        mount_commit_steps(&server, &parent).await;
        Mock::given(method("PATCH"))
            .and(path("/repos/o/r/git/refs/heads/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let new = repo
            .commit(&parent, &changes(), "me@pc: update CLAUDE.md")
            .await
            .unwrap();
        // GitHub still answers with the parent for a moment.
        assert_eq!(repo.head().await.unwrap(), new);
        assert_eq!(repo.head_if_changed().await.unwrap(), None);
        server.reset().await;
        get_head("c10", "t10").mount(&server).await;
        assert_eq!(
            repo.head_if_changed().await.unwrap(),
            Some(head("c10", "t10"))
        );
    }

    #[tokio::test]
    async fn non_fast_forward_is_a_push_race() {
        let (server, repo) = setup().await;
        let parent = head("c1", "t1");
        mount_commit_steps(&server, &parent).await;
        Mock::given(method("PATCH"))
            .and(path("/repos/o/r/git/refs/heads/main"))
            .respond_with(
                ResponseTemplate::new(422)
                    .set_body_json(json!({"message": "Update is not a fast forward"})),
            )
            .mount(&server)
            .await;
        let err = repo
            .commit(&parent, &changes(), "me@pc: update CLAUDE.md")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PushRace));
    }

    #[test]
    fn ref_race_messages() {
        assert!(is_ref_race("Update is not a fast forward"));
        assert!(is_ref_race("Reference cannot be updated"));
        assert!(!is_ref_race("Object does not exist"));
    }

    #[tokio::test]
    async fn first_commit_of_an_empty_repo_goes_through_contents() {
        let (server, repo) = setup().await;
        Mock::given(method("PUT"))
            .and(path("/repos/o/r/contents/CLAUDE.md"))
            .and(body_partial_json(
                json!({"content": "aGk=", "message": "init (start the repo)"}),
            ))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({"commit": {"sha": "c1", "tree": {"sha": "t1"}}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/blobs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "b2"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/trees"))
            .and(body_partial_json(json!({"base_tree": "t1"})))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "t2"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/commits"))
            .and(body_partial_json(
                json!({"parents": ["c1"], "message": "init"}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "c2"})))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/o/r/git/refs/heads/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let changes = BTreeMap::from([
            ("CLAUDE.md".into(), Some(b"hi".to_vec())),
            ("claude-sync.json".into(), Some(b"{}".to_vec())),
        ]);
        assert_eq!(
            repo.commit(&Head::empty(), &changes, "init").await.unwrap(),
            head("c2", "t2")
        );

        // Someone else made the first commit meanwhile.
        server.reset().await;
        Mock::given(method("PUT"))
            .respond_with(
                ResponseTemplate::new(422).set_body_json(json!({"message": "sha wasn't supplied"})),
            )
            .mount(&server)
            .await;
        let one = BTreeMap::from([("CLAUDE.md".into(), Some(b"hi".to_vec()))]);
        assert!(matches!(
            repo.commit(&Head::empty(), &one, "init").await,
            Err(Error::PushRace)
        ));
    }

    #[tokio::test]
    async fn members_and_invitations() {
        let (server, repo) = setup().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/collaborators"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!([{"login": "o"}, {"login": "f"}])),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/invitations"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"id": 7, "invitee": {"login": "p"}}])),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/repos/o/r/collaborators/q"))
            .and(body_json(json!({"permission": "pull"})))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 8})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/repos/o/r/collaborators/f"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/repos/o/r/invitations/7"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            repo.collaborators().await.unwrap(),
            [
                Collaborator::Member("o".into()),
                Collaborator::Member("f".into()),
                Collaborator::Invited {
                    login: "p".into(),
                    invitation_id: 7
                },
            ]
        );
        repo.invite("q").await.unwrap();
        repo.remove_collaborator("f").await.unwrap();
        repo.cancel_invitation(7).await.unwrap();
    }

    #[tokio::test]
    async fn ensure_host_lock_creates_the_ruleset_once() {
        let (server, repo) = setup().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/rulesets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/o/r/rulesets"))
            .and(body_partial_json(json!({
                "name": HOST_LOCK_NAME,
                "enforcement": "active",
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 1})))
            .expect(1)
            .mount(&server)
            .await;
        repo.ensure_host_lock().await.unwrap();
    }

    #[tokio::test]
    async fn ensure_host_lock_leaves_an_existing_ruleset_alone() {
        let (server, repo) = setup().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/rulesets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": 3, "name": "something else"},
                {"id": 4, "name": HOST_LOCK_NAME},
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/o/r/rulesets"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 1})))
            .expect(0)
            .mount(&server)
            .await;
        repo.ensure_host_lock().await.unwrap();
    }

    #[tokio::test]
    async fn recent_commits_newest_first() {
        let (server, repo) = setup().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/commits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"sha": "c2", "author": {"login": "f"},
                 "commit": {"message": "f@mac: update CLAUDE.md", "author": {"name": "F", "date": "2026-10-13T09:45:01Z"}}},
                {"sha": "c1", "author": null,
                 "commit": {"message": "init", "author": {"name": "Someone", "date": "bad"}}},
            ])))
            .mount(&server)
            .await;
        let got = repo.recent_commits(10).await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(
            (got[0].author.as_str(), got[0].at_unix),
            ("f", 1_791_884_701)
        );
        assert_eq!((got[1].author.as_str(), got[1].at_unix), ("Someone", 0));
    }

    #[tokio::test]
    async fn commits_between_and_file_at() {
        let (server, repo) = setup().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/compare/c1...c3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"commits": [
                {"sha": "c2", "author": {"login": "f"}, "commit": {"message": "m2", "author": {"name": "F"}}},
                {"sha": "c3", "author": null, "commit": {"message": "m3", "author": {"name": "G"}}},
            ]})))
            .mount(&server)
            .await;
        let got = repo.commits_between("c1", "c3").await.unwrap();
        let who: Vec<_> = got
            .iter()
            .map(|c| (c.sha.as_str(), c.author.as_str()))
            .collect();
        assert_eq!(who, [("c2", "f"), ("c3", "G")]);

        Mock::given(method("GET"))
            .and(path("/repos/o/r/contents/CLAUDE.md"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": "aGVs\nbG8K\n", "encoding": "base64"
            })))
            .mount(&server)
            .await;
        assert_eq!(
            repo.file_at("c2", "CLAUDE.md").await.unwrap().unwrap(),
            b"hello\n"
        );
        assert_eq!(repo.file_at("c2", "missing.md").await.unwrap(), None);
    }
}
