//! In-memory `RemoteRepo` and `RemoteFactory` for tests (this crate's and the app's).
//!
//! `FakeGitHub` is a world of repos. `FakeGitHub::account(login)` is what one signed-in user
//! sees: repo handles that answer `404` once that user has no access, like GitHub does.
//! A `FakeRepo` made with `FakeRepo::new` has no viewer and can do anything (the test's view).
use super::{
    Changes, Collaborator, CommitFiles, Head, PathFilter, Proposal, ProposalFile, RemoteFactory,
    RemoteRepo, PROPOSAL_PREFIX,
};
use crate::engine::CommitInfo;
use crate::github::Invitation;
use crate::{Error, FileSet, GroupId};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static NEXT_INVITATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeCommit {
    pub sha: String,
    pub tree: String,
    pub parent: Option<String>,
    /// Every file in the repo at this commit, allowlisted or not.
    pub files: FileSet,
    pub message: String,
    pub author: String,
}

/// A pull request on the fake repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakePull {
    pub number: u64,
    pub branch: String,
    pub author: String,
    pub title: String,
    /// The default-branch commit the branch was made from.
    pub base: String,
    pub changes: Changes,
    /// `Open`, `Merged` or `Closed`.
    pub state: &'static str,
    /// `(login, approved)`, one per reviewer.
    pub reviews: Vec<(String, bool)>,
    pub comments: Vec<String>,
}

type Hook = Box<dyn FnMut(&FakeRepo) + Send>;

struct Failure {
    op: String,
    status: u16,
    times: Option<u32>,
}

struct State {
    /// The repo's name now; differs from a handle's `id` after `FakeGitHub::rename`.
    full_name: GroupId,
    /// Owners a transfer was asked for (GitHub waits for them to accept).
    transfers: Vec<String>,
    commits: Vec<FakeCommit>,
    owner: String,
    private: bool,
    topics: Vec<String>,
    collaborators: Vec<String>,
    /// Collaborators with read access only: commits fail with `403`.
    readers: Vec<String>,
    invitations: Vec<(u64, String)>,
    rulesets: Vec<String>,
    failures: Vec<Failure>,
    before_commit: Option<Hook>,
    calls: Vec<String>,
    pulls: Vec<FakePull>,
}

impl State {
    fn head(&self) -> Head {
        self.commits.last().map_or_else(Head::empty, |c| Head {
            commit: c.sha.clone(),
            tree: c.tree.clone(),
        })
    }
    fn files(&self) -> FileSet {
        self.commits
            .last()
            .map(|c| c.files.clone())
            .unwrap_or_default()
    }
    fn has_access(&self, login: &str) -> bool {
        self.owner.eq_ignore_ascii_case(login)
            || self
                .collaborators
                .iter()
                .any(|c| c.eq_ignore_ascii_case(login))
    }
    fn push(&mut self, files: FileSet, message: &str, author: &str) -> Head {
        let n = self.commits.len() + 1;
        let parent = self.commits.last().map(|c| c.sha.clone());
        self.commits.push(FakeCommit {
            sha: format!("c{n}"),
            tree: format!("t{n}"),
            parent,
            files,
            message: message.into(),
            author: author.into(),
        });
        self.head()
    }
}

fn apply_changes(mut files: FileSet, changes: &Changes) -> crate::Result<FileSet> {
    for (path, content) in changes {
        match content {
            Some(bytes) => {
                files.insert(path.clone(), bytes.clone());
            }
            None => {
                files.remove(path).ok_or_else(|| Error::GitHub {
                    status: 422,
                    message: format!("{path} is not in the tree"),
                })?;
            }
        }
    }
    Ok(files)
}

fn not_found() -> Error {
    Error::GitHub {
        status: 404,
        message: "Not Found".into(),
    }
}

/// An in-memory group repo. Clones share the repo; `as_user` makes a handle with a viewer.
#[derive(Clone)]
pub struct FakeRepo {
    id: GroupId,
    state: Arc<Mutex<State>>,
    viewer: Option<String>,
    /// Last head returned by `head_if_changed` through this handle (the ETag).
    seen: Arc<Mutex<Option<String>>>,
}

impl FakeRepo {
    /// An empty private repo owned by `id.owner`, with the group topic.
    pub fn new(id: GroupId) -> Self {
        let owner = id.owner.clone();
        let full_name = id.clone();
        Self {
            id,
            state: Arc::new(Mutex::new(State {
                full_name,
                transfers: Vec::new(),
                commits: Vec::new(),
                owner,
                private: true,
                topics: vec![crate::group::TOPIC.into()],
                collaborators: Vec::new(),
                readers: Vec::new(),
                invitations: Vec::new(),
                rulesets: Vec::new(),
                failures: Vec::new(),
                before_commit: None,
                calls: Vec::new(),
                pulls: Vec::new(),
            })),
            viewer: None,
            seen: Arc::default(),
        }
    }

    /// A repo whose first commit holds `files` (`(path, text)` pairs).
    pub fn with_files(id: GroupId, files: &[(&str, &str)]) -> Self {
        let repo = Self::new(id);
        let changes = files
            .iter()
            .map(|(p, c)| (p.to_string(), Some(c.as_bytes().to_vec())))
            .collect();
        repo.push(&changes, "initial");
        repo
    }

    /// Same repo seen by `login`: calls fail with `404` when `login` has no access, and
    /// collaborator management fails with `403` unless `login` owns the repo.
    pub fn as_user(&self, login: &str) -> Self {
        Self {
            id: self.id.clone(),
            state: self.state.clone(),
            viewer: Some(login.into()),
            seen: Arc::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Someone else pushes (no checks, no hooks). Returns the new head.
    pub fn push(&self, changes: &Changes, message: &str) -> Head {
        let owner = self.owner();
        self.push_as(&owner, changes, message)
    }

    /// `author` pushes from outside the app (a git clone, the web UI).
    pub fn push_as(&self, author: &str, changes: &Changes, message: &str) -> Head {
        let mut st = self.lock();
        let files = apply_changes(st.files(), changes).expect("fake push");
        st.push(files, message, author)
    }

    /// `author` writes one file from outside the app.
    pub fn set_file_as(&self, author: &str, path: &str, content: &str) -> Head {
        self.push_as(
            author,
            &BTreeMap::from([(path.to_string(), Some(content.as_bytes().to_vec()))]),
            &format!("update {path}"),
        )
    }

    /// Someone else writes one file.
    pub fn set_file(&self, path: &str, content: &str) -> Head {
        self.push(
            &BTreeMap::from([(path.to_string(), Some(content.as_bytes().to_vec()))]),
            &format!("update {path}"),
        )
    }

    /// Someone else deletes one file.
    pub fn remove_file(&self, path: &str) -> Head {
        self.push(
            &BTreeMap::from([(path.to_string(), None)]),
            &format!("remove {path}"),
        )
    }

    /// All files at the head, allowlisted or not.
    pub fn files(&self) -> FileSet {
        self.lock().files()
    }

    pub fn file(&self, path: &str) -> Option<String> {
        self.lock()
            .files()
            .get(path)
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }

    pub fn current_head(&self) -> Head {
        self.lock().head()
    }

    pub fn commits(&self) -> Vec<FakeCommit> {
        self.lock().commits.clone()
    }

    pub fn set_private(&self, private: bool) {
        self.lock().private = private;
    }

    pub fn set_topics(&self, topics: &[&str]) {
        self.lock().topics = topics.iter().map(|t| t.to_string()).collect();
    }

    pub fn topics(&self) -> Vec<String> {
        self.lock().topics.clone()
    }

    /// Owners `transfer` was asked for, oldest first.
    pub fn transfer_requests(&self) -> Vec<String> {
        self.lock().transfers.clone()
    }

    pub fn owner(&self) -> String {
        self.lock().owner.clone()
    }

    /// Give `login` access directly (as if they had accepted an invitation).
    /// Make `login` the repo's owner (the host) while the id keeps its name.
    pub fn set_owner(&self, login: &str) {
        let mut st = self.lock();
        st.collaborators.retain(|c| !c.eq_ignore_ascii_case(login));
        st.owner = login.into();
    }

    pub fn add_collaborator(&self, login: &str) {
        let mut st = self.lock();
        if !st.has_access(login) {
            st.collaborators.push(login.into());
        }
    }

    /// Give `login` read access only: they can pull, but commits fail with `403`.
    pub fn add_reader(&self, login: &str) {
        self.add_collaborator(login);
        self.lock().readers.push(login.into());
    }

    /// Collaborators other than the owner.
    pub fn collaborator_logins(&self) -> Vec<String> {
        self.lock().collaborators.clone()
    }

    /// `(invitation id, login)` of pending invitations.
    pub fn pending_invitations(&self) -> Vec<(u64, String)> {
        self.lock().invitations.clone()
    }

    /// The invitee accepts their pending invitation.
    pub fn accept_invitation_of(&self, login: &str) -> bool {
        let mut st = self.lock();
        let Some(pos) = st
            .invitations
            .iter()
            .position(|(_, l)| l.eq_ignore_ascii_case(login))
        else {
            return false;
        };
        let (_, l) = st.invitations.remove(pos);
        st.collaborators.push(l);
        true
    }

    /// Every later call of `op` (a `RemoteRepo` method name, or `"*"` for all) fails with
    /// `Error::GitHub { status }`.
    pub fn fail(&self, op: &str, status: u16) {
        self.lock().failures.push(Failure {
            op: op.into(),
            status,
            times: None,
        });
    }

    /// The next `times` calls of `op` fail.
    pub fn fail_times(&self, op: &str, status: u16, times: u32) {
        self.lock().failures.push(Failure {
            op: op.into(),
            status,
            times: Some(times),
        });
    }

    pub fn clear_failures(&self) {
        self.lock().failures.clear();
    }

    /// Run `hook` at the start of every `commit` call, before the parent check. Pushing from the
    /// hook simulates someone pushing between our fetch and our push.
    pub fn before_commit(&self, hook: impl FnMut(&FakeRepo) + Send + 'static) {
        self.lock().before_commit = Some(Box::new(hook));
    }

    pub fn clear_before_commit(&self) {
        self.lock().before_commit = None;
    }

    /// `RemoteRepo` calls made so far, in order (method names).
    /// Names of the rulesets created on the repo.
    pub fn rulesets(&self) -> Vec<String> {
        self.lock().rulesets.clone()
    }

    /// Every pull request, oldest first.
    pub fn pulls(&self) -> Vec<FakePull> {
        self.lock().pulls.clone()
    }

    /// `login` reviews pull request `number` from outside the app.
    pub fn review_as(&self, login: &str, number: u64, approved: bool) {
        let mut st = self.lock();
        let pr = st.pulls.iter_mut().find(|p| p.number == number).unwrap();
        pr.reviews.retain(|(l, _)| !l.eq_ignore_ascii_case(login));
        pr.reviews.push((login.into(), approved));
    }

    pub fn calls(&self) -> Vec<String> {
        self.lock().calls.clone()
    }

    /// Record the call, apply injected failures and the viewer's access.
    fn enter(&self, op: &str) -> crate::Result<std::sync::MutexGuard<'_, State>> {
        let mut st = self.lock();
        st.calls.push(op.into());
        if let Some(i) = st.failures.iter().position(|f| f.op == op || f.op == "*") {
            let status = st.failures[i].status;
            match &mut st.failures[i].times {
                Some(1) => {
                    st.failures.remove(i);
                }
                Some(n) => *n -= 1,
                None => {}
            }
            return Err(Error::GitHub {
                status,
                message: "injected failure".into(),
            });
        }
        if let Some(v) = &self.viewer {
            if !st.has_access(v) {
                return Err(not_found());
            }
        }
        Ok(st)
    }

    fn pushable(&self) -> bool {
        let st = self.lock();
        self.viewer
            .as_ref()
            .is_none_or(|v| !st.readers.iter().any(|r| r.eq_ignore_ascii_case(v)))
    }

    fn enter_as_owner(&self, op: &str) -> crate::Result<std::sync::MutexGuard<'_, State>> {
        let st = self.enter(op)?;
        match &self.viewer {
            Some(v) if !st.owner.eq_ignore_ascii_case(v) => Err(Error::GitHub {
                status: 403,
                message: "Must have admin rights to Repository.".into(),
            }),
            _ => Ok(st),
        }
    }
}

#[async_trait]
impl RemoteRepo for FakeRepo {
    fn id(&self) -> &GroupId {
        &self.id
    }

    async fn head_if_changed(&self) -> crate::Result<Option<Head>> {
        let head = self.enter("head_if_changed")?.head();
        let mut seen = self.seen.lock().unwrap();
        if seen.as_deref() == Some(head.commit.as_str()) {
            return Ok(None);
        }
        *seen = Some(head.commit.clone());
        Ok(Some(head))
    }

    async fn head(&self) -> crate::Result<Head> {
        Ok(self.enter("head")?.head())
    }

    async fn files_matching(&self, commit: &str, keep: PathFilter) -> crate::Result<FileSet> {
        let st = self.enter("files_at")?;
        if commit.is_empty() {
            return Ok(FileSet::new());
        }
        let c = st
            .commits
            .iter()
            .find(|c| c.sha == commit)
            .ok_or_else(not_found)?;
        Ok(c.files
            .iter()
            .filter(|(p, _)| keep(p))
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect())
    }

    async fn commit(&self, parent: &Head, changes: &Changes, message: &str) -> crate::Result<Head> {
        drop(self.enter("commit")?);
        if !self.pushable() {
            return Err(Error::GitHub {
                status: 403,
                message: "Permission denied".into(),
            });
        }
        let hook = self.lock().before_commit.take();
        if let Some(mut hook) = hook {
            hook(self);
            let mut st = self.lock();
            if st.before_commit.is_none() {
                st.before_commit = Some(hook);
            }
        }
        let mut st = self.lock();
        if st.head() != *parent {
            return Err(Error::PushRace);
        }
        let files = apply_changes(st.files(), changes)?;
        let author = self.viewer.clone().unwrap_or_else(|| st.owner.clone());
        Ok(st.push(files, message, &author))
    }

    async fn propose(
        &self,
        parent: &Head,
        changes: &Changes,
        branch: &str,
        title: &str,
    ) -> crate::Result<u64> {
        drop(self.enter("propose")?);
        if !self.pushable() || !branch.starts_with(PROPOSAL_PREFIX) {
            return Err(Error::GitHub {
                status: 403,
                message: "Permission denied".into(),
            });
        }
        let mut st = self.lock();
        if !st.commits.iter().any(|c| c.sha == parent.commit) {
            return Err(not_found());
        }
        let author = self.viewer.clone().unwrap_or_else(|| st.owner.clone());
        let number = st.pulls.len() as u64 + 1;
        st.pulls.push(FakePull {
            number,
            branch: branch.into(),
            author,
            title: title.into(),
            base: parent.commit.clone(),
            changes: changes.clone(),
            state: "Open",
            reviews: Vec::new(),
            comments: Vec::new(),
        });
        Ok(number)
    }

    async fn proposals(&self) -> crate::Result<Vec<Proposal>> {
        let st = self.enter("proposals")?;
        let now = st.files();
        Ok(st
            .pulls
            .iter()
            .filter(|p| p.state == "Open")
            .map(|p| {
                let old = st
                    .commits
                    .iter()
                    .find(|c| c.sha == p.base)
                    .map(|c| c.files.clone())
                    .unwrap_or_default();
                let text = |b: Option<&Vec<u8>>| {
                    b.map(|b| String::from_utf8_lossy(b).into_owned())
                        .unwrap_or_default()
                };
                Proposal {
                    number: p.number,
                    branch: p.branch.clone(),
                    author: p.author.clone(),
                    title: p.title.clone(),
                    url: format!("https://github.com/{}/pull/{}", self.id, p.number),
                    files: p
                        .changes
                        .iter()
                        .filter(|(path, _)| crate::group::is_allowlisted(path))
                        .map(|(path, new)| ProposalFile {
                            path: path.clone(),
                            patch: format!("-{}\n+{}", text(old.get(path)), text(new.as_ref())),
                        })
                        .collect(),
                    approvals: p
                        .reviews
                        .iter()
                        .filter(|(l, ok)| *ok && !l.eq_ignore_ascii_case(&p.author))
                        .map(|(l, _)| l.clone())
                        .collect(),
                    needs_update: p.changes.keys().any(|path| now.get(path) != old.get(path)),
                }
            })
            .collect())
    }

    async fn approve_proposal(&self, number: u64) -> crate::Result<()> {
        let mut st = self.enter("approve_proposal")?;
        let me = self.viewer.clone().unwrap_or_else(|| st.owner.clone());
        let pr = st
            .pulls
            .iter_mut()
            .find(|p| p.number == number && p.state == "Open")
            .ok_or_else(not_found)?;
        if pr.author.eq_ignore_ascii_case(&me) {
            return Err(Error::GitHub {
                status: 422,
                message: "Can not approve your own pull request".into(),
            });
        }
        pr.reviews.retain(|(l, _)| !l.eq_ignore_ascii_case(&me));
        pr.reviews.push((me, true));
        Ok(())
    }

    async fn merge_proposal(&self, number: u64) -> crate::Result<()> {
        // The default branch is locked to the host (§5a), so a merge needs the owner.
        let mut st = self.enter_as_owner("merge_proposal")?;
        let i = st
            .pulls
            .iter()
            .position(|p| p.number == number && p.state == "Open")
            .ok_or_else(not_found)?;
        let pr = st.pulls[i].clone();
        let old = st
            .commits
            .iter()
            .find(|c| c.sha == pr.base)
            .map(|c| c.files.clone())
            .unwrap_or_default();
        let now = st.files();
        if pr.changes.keys().any(|path| now.get(path) != old.get(path)) {
            return Err(Error::Invalid(format!(
                "proposal #{number} needs an update before it can be merged"
            )));
        }
        let files = apply_changes(now, &pr.changes)?;
        st.push(files, &pr.title, &pr.author);
        st.pulls[i].state = "Merged";
        Ok(())
    }

    async fn decline_proposal(&self, number: u64, comment: &str) -> crate::Result<()> {
        let mut st = self.enter("decline_proposal")?;
        let pr = st
            .pulls
            .iter_mut()
            .find(|p| p.number == number && p.state == "Open")
            .ok_or_else(not_found)?;
        pr.comments.push(comment.into());
        pr.state = "Closed";
        Ok(())
    }

    async fn is_private(&self) -> crate::Result<bool> {
        Ok(self.enter("is_private")?.private)
    }

    async fn can_push(&self) -> crate::Result<bool> {
        drop(self.enter("can_push")?);
        Ok(self.pushable())
    }

    async fn owner_login(&self) -> crate::Result<String> {
        Ok(self.enter("owner_login")?.owner.clone())
    }

    async fn current_id(&self) -> crate::Result<GroupId> {
        Ok(self.enter("current_id")?.full_name.clone())
    }

    async fn transfer(&self, new_owner: &str) -> crate::Result<()> {
        self.enter_as_owner("transfer")?
            .transfers
            .push(new_owner.into());
        Ok(())
    }

    async fn collaborators(&self) -> crate::Result<Vec<Collaborator>> {
        let st = self.enter_as_owner("collaborators")?;
        let members = std::iter::once(&st.owner)
            .chain(&st.collaborators)
            .cloned()
            .map(Collaborator::Member);
        let invited = st
            .invitations
            .iter()
            .map(|(id, login)| Collaborator::Invited {
                login: login.clone(),
                invitation_id: *id,
            });
        Ok(members.chain(invited).collect())
    }

    async fn ensure_host_lock(&self) -> crate::Result<()> {
        let mut st = self.enter_as_owner("ensure_host_lock")?;
        if !st.rulesets.iter().any(|n| n == super::HOST_LOCK_NAME) {
            st.rulesets.push(super::HOST_LOCK_NAME.into());
        }
        Ok(())
    }

    async fn invite(&self, login: &str) -> crate::Result<()> {
        let mut st = self.enter_as_owner("invite")?;
        if !st.has_access(login)
            && !st
                .invitations
                .iter()
                .any(|(_, l)| l.eq_ignore_ascii_case(login))
        {
            st.invitations.push((
                NEXT_INVITATION.fetch_add(1, Ordering::Relaxed),
                login.into(),
            ));
        }
        Ok(())
    }

    async fn remove_collaborator(&self, login: &str) -> crate::Result<()> {
        let mut st = self.enter_as_owner("remove_collaborator")?;
        st.collaborators.retain(|c| !c.eq_ignore_ascii_case(login));
        Ok(())
    }

    async fn cancel_invitation(&self, invitation_id: u64) -> crate::Result<()> {
        let mut st = self.enter_as_owner("cancel_invitation")?;
        let before = st.invitations.len();
        st.invitations.retain(|(id, _)| *id != invitation_id);
        if st.invitations.len() == before {
            return Err(not_found());
        }
        Ok(())
    }

    async fn recent_commits(&self, limit: usize) -> crate::Result<Vec<CommitInfo>> {
        let st = self.enter("recent_commits")?;
        Ok(st
            .commits
            .iter()
            .enumerate()
            .rev()
            .take(limit)
            .map(|(n, c)| CommitInfo {
                sha: c.sha.clone(),
                message: c.message.clone(),
                author: c.author.clone(),
                at_unix: n as u64 + 1,
            })
            .collect())
    }

    async fn commits_between(&self, from: &str, to: &str) -> crate::Result<Vec<CommitInfo>> {
        let st = self.enter("commits_between")?;
        let end = st
            .commits
            .iter()
            .position(|c| c.sha == to)
            .ok_or_else(not_found)?;
        let start = st
            .commits
            .iter()
            .position(|c| c.sha == from)
            .map_or(0, |i| i + 1);
        Ok(st.commits[start..=end]
            .iter()
            .enumerate()
            .map(|(n, c)| CommitInfo {
                sha: c.sha.clone(),
                message: c.message.clone(),
                author: c.author.clone(),
                at_unix: (start + n) as u64 + 1,
            })
            .collect())
    }

    async fn file_at(&self, commit: &str, path: &str) -> crate::Result<Option<Vec<u8>>> {
        let st = self.enter("file_at")?;
        let c = st
            .commits
            .iter()
            .find(|c| c.sha == commit)
            .ok_or_else(not_found)?;
        Ok(c.files.get(path).cloned())
    }

    async fn commit_files(&self, commit: &str) -> crate::Result<CommitFiles> {
        let st = self.enter("commit_files")?;
        let c = st
            .commits
            .iter()
            .find(|c| c.sha == commit)
            .ok_or_else(not_found)?;
        let before = match &c.parent {
            Some(p) => st
                .commits
                .iter()
                .find(|x| &x.sha == p)
                .map(|x| x.files.clone())
                .unwrap_or_default(),
            None => FileSet::default(),
        };
        let paths = c
            .files
            .keys()
            .chain(before.keys())
            .filter(|p| c.files.get(*p) != before.get(*p))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(CommitFiles {
            parent: c.parent.clone(),
            paths,
        })
    }
}

/// A world of fake repos shared by several fake accounts.
#[derive(Clone, Default)]
pub struct FakeGitHub {
    repos: Arc<Mutex<BTreeMap<GroupId, FakeRepo>>>,
}

impl FakeGitHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, repo: &FakeRepo) {
        let god = FakeRepo {
            viewer: None,
            seen: Arc::default(),
            ..repo.clone()
        };
        self.repos.lock().unwrap().insert(repo.id.clone(), god);
    }

    /// The repo is now called `new` (a rename or a finished transfer). The old name keeps
    /// reaching it, like GitHub's redirect. A new owner takes over as the host; the old one
    /// stays a collaborator, as GitHub keeps the former owner of a transferred repo.
    pub fn rename(&self, old: &GroupId, new: &GroupId) {
        let repo = self.repo(old).expect("no such repo");
        {
            let mut st = repo.lock();
            st.full_name = new.clone();
            if !st.owner.eq_ignore_ascii_case(&new.owner) {
                let before = std::mem::replace(&mut st.owner, new.owner.clone());
                st.collaborators
                    .retain(|c| !c.eq_ignore_ascii_case(&new.owner));
                st.collaborators.push(before);
            }
        }
        let alias = FakeRepo {
            id: new.clone(),
            ..repo
        };
        self.repos.lock().unwrap().insert(new.clone(), alias);
    }

    /// The test's (all-powerful) handle on a repo.
    pub fn repo(&self, id: &GroupId) -> Option<FakeRepo> {
        self.repos.lock().unwrap().get(id).cloned()
    }

    pub fn account(&self, login: &str) -> FakeAccount {
        FakeAccount {
            world: self.clone(),
            login: login.into(),
            handles: Mutex::default(),
        }
    }

    fn all(&self) -> Vec<FakeRepo> {
        self.repos.lock().unwrap().values().cloned().collect()
    }
}

/// What one signed-in user sees of a `FakeGitHub`.
pub struct FakeAccount {
    world: FakeGitHub,
    login: String,
    handles: Mutex<HashMap<GroupId, Arc<FakeRepo>>>,
}

impl FakeAccount {
    pub fn login(&self) -> &str {
        &self.login
    }
}

#[async_trait]
impl RemoteFactory for FakeAccount {
    fn open(&self, id: &GroupId) -> Arc<dyn RemoteRepo> {
        let mut handles = self.handles.lock().unwrap();
        if let Some(h) = handles.get(id) {
            return h.clone();
        }
        match self.world.repo(id) {
            Some(repo) => {
                let h = Arc::new(repo.as_user(&self.login));
                handles.insert(id.clone(), h.clone());
                h
            }
            // No such repo: a handle no one can see into.
            None => {
                let ghost = FakeRepo::new(GroupId::new("", &id.repo)).as_user(&self.login);
                Arc::new(FakeRepo {
                    id: id.clone(),
                    ..ghost
                })
            }
        }
    }

    async fn create_repo(&self, name: &str) -> crate::Result<GroupId> {
        let id = GroupId::new(&self.login, name);
        if self.world.repo(&id).is_some() {
            return Err(Error::GitHub {
                status: 422,
                message: "name already exists on this account".into(),
            });
        }
        self.world.add(&FakeRepo::new(id.clone()));
        Ok(id)
    }

    async fn create_personal_repo(&self, name: &str) -> crate::Result<GroupId> {
        let id = self.create_repo(name).await?;
        let repo = self.world.repo(&id).expect("just created");
        repo.set_topics(&[]);
        Ok(id)
    }

    async fn invitations(&self) -> crate::Result<Vec<Invitation>> {
        Ok(self
            .world
            .all()
            .into_iter()
            .filter(|r| r.topics().iter().any(|t| t == crate::group::TOPIC))
            .flat_map(|r| {
                let owner = r.owner();
                r.pending_invitations()
                    .into_iter()
                    .filter(|(_, l)| l.eq_ignore_ascii_case(&self.login))
                    .map(move |(id, _)| Invitation {
                        id,
                        repo: r.id.clone(),
                        inviter: owner.clone(),
                    })
            })
            .collect())
    }

    async fn accept_invitation(&self, id: u64) -> crate::Result<GroupId> {
        for repo in self.world.all() {
            let mine = repo
                .pending_invitations()
                .iter()
                .any(|(i, l)| *i == id && l.eq_ignore_ascii_case(&self.login));
            if mine {
                repo.accept_invitation_of(&self.login);
                return Ok(repo.id.clone());
            }
        }
        Err(not_found())
    }

    async fn group_repos(&self) -> crate::Result<Vec<GroupId>> {
        Ok(self
            .world
            .all()
            .into_iter()
            .filter(|r| {
                let st = r.lock();
                st.private
                    && st.has_access(&self.login)
                    && st.topics.iter().any(|t| t == crate::group::TOPIC)
            })
            .map(|r| r.id.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> GroupId {
        GroupId::new("host", "claude-sync-data")
    }

    fn change(path: &str, text: &str) -> Changes {
        BTreeMap::from([(path.to_string(), Some(text.as_bytes().to_vec()))])
    }

    #[tokio::test]
    async fn commits_and_reads_only_allowlisted_files() {
        let repo = FakeRepo::new(id());
        assert_eq!(repo.head().await.unwrap(), Head::empty());
        let mut first = change("CLAUDE.md", "hi");
        first.insert("README.md".into(), Some(b"readme".to_vec()));
        let h1 = repo.commit(&Head::empty(), &first, "init").await.unwrap();
        assert_eq!(
            repo.files_at(&h1.commit)
                .await
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["CLAUDE.md"]
        );
        assert_eq!(repo.file("README.md").as_deref(), Some("readme"));
        let h2 = repo
            .commit(&h1, &BTreeMap::from([("CLAUDE.md".into(), None)]), "rm")
            .await
            .unwrap();
        assert!(repo.files_at(&h2.commit).await.unwrap().is_empty());
        assert_eq!(
            repo.files_at(&h1.commit).await.unwrap().len(),
            1,
            "old commits stay readable"
        );
    }

    #[tokio::test]
    async fn stale_parent_is_a_push_race() {
        let repo = FakeRepo::with_files(id(), &[("CLAUDE.md", "a")]);
        let h = repo.head().await.unwrap();
        repo.set_file("CLAUDE.md", "theirs");
        let err = repo
            .commit(&h, &change("CLAUDE.md", "mine"), "m")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PushRace));
    }

    #[tokio::test]
    async fn before_commit_hook_simulates_a_concurrent_push() {
        let repo = FakeRepo::with_files(id(), &[("CLAUDE.md", "a")]);
        let mut left = 1;
        repo.before_commit(move |r| {
            if left > 0 {
                left -= 1;
                r.set_file("agents/x.md", "theirs");
            }
        });
        let h = repo.head().await.unwrap();
        assert!(matches!(
            repo.commit(&h, &change("CLAUDE.md", "b"), "m").await,
            Err(Error::PushRace)
        ));
        let h = repo.head().await.unwrap();
        repo.commit(&h, &change("CLAUDE.md", "b"), "m")
            .await
            .unwrap();
        assert_eq!(repo.file("agents/x.md").as_deref(), Some("theirs"));
        assert_eq!(repo.file("CLAUDE.md").as_deref(), Some("b"));
    }

    #[tokio::test]
    async fn head_if_changed_is_per_handle() {
        let repo = FakeRepo::with_files(id(), &[("CLAUDE.md", "a")]);
        assert!(repo.head_if_changed().await.unwrap().is_some());
        assert!(repo.head_if_changed().await.unwrap().is_none());
        let other = repo.as_user("host");
        assert!(other.head_if_changed().await.unwrap().is_some());
        repo.set_file("CLAUDE.md", "b");
        assert_eq!(repo.head_if_changed().await.unwrap().unwrap().commit, "c2");
    }

    #[tokio::test]
    async fn failure_injection() {
        let repo = FakeRepo::with_files(id(), &[("CLAUDE.md", "a")]);
        repo.fail_times("head", 500, 1);
        assert!(matches!(
            repo.head().await,
            Err(Error::GitHub { status: 500, .. })
        ));
        assert!(repo.head().await.is_ok());
        repo.fail("*", 404);
        assert!(matches!(
            repo.files_at("c1").await,
            Err(Error::GitHub { status: 404, .. })
        ));
        repo.clear_failures();
        assert!(repo.files_at("c1").await.is_ok());
        assert_eq!(repo.calls(), ["head", "head", "files_at", "files_at"]);
    }

    #[tokio::test]
    async fn access_follows_collaborators_and_invitations() {
        let world = FakeGitHub::new();
        let repo = FakeRepo::with_files(id(), &[("claude-sync.json", "{}")]);
        world.add(&repo);
        let host = repo.as_user("Host");
        let friend = repo.as_user("friend");
        assert!(matches!(
            friend.head().await,
            Err(Error::GitHub { status: 404, .. })
        ));

        host.invite("Friend").await.unwrap();
        host.invite("friend").await.unwrap();
        assert_eq!(repo.pending_invitations().len(), 1, "no second invitation");
        let account = world.account("friend");
        let invs = account.invitations().await.unwrap();
        assert_eq!(invs.len(), 1);
        assert_eq!(invs[0].repo, id());
        assert_eq!(invs[0].inviter, "host");
        assert!(account.group_repos().await.unwrap().is_empty());

        assert_eq!(account.accept_invitation(invs[0].id).await.unwrap(), id());
        assert!(friend.head().await.is_ok());
        assert_eq!(account.group_repos().await.unwrap(), [id()]);
        assert!(matches!(
            friend.invite("x").await,
            Err(Error::GitHub { status: 403, .. })
        ));
        assert_eq!(
            host.collaborators().await.unwrap(),
            [
                Collaborator::Member("host".into()),
                Collaborator::Member("Friend".into())
            ]
        );

        host.remove_collaborator("FRIEND").await.unwrap();
        assert!(matches!(
            friend.head().await,
            Err(Error::GitHub { status: 404, .. })
        ));
    }

    #[tokio::test]
    async fn accounts_create_repos_and_open_missing_ones() {
        let world = FakeGitHub::new();
        let me = world.account("me");
        let id = me.create_repo("claude-sync-data").await.unwrap();
        assert_eq!(id, GroupId::new("me", "claude-sync-data"));
        assert!(me.create_repo("claude-sync-data").await.is_err());
        let repo = me.open(&id);
        assert_eq!(repo.head().await.unwrap(), Head::empty());
        assert!(repo.is_private().await.unwrap());
        let missing = me.open(&GroupId::new("nobody", "x"));
        assert!(matches!(
            missing.head().await,
            Err(Error::GitHub { status: 404, .. })
        ));
        assert_eq!(missing.id(), &GroupId::new("nobody", "x"));
    }
}
