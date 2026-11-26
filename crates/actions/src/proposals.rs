//! Proposals (Parity 36, 83, 84, 85): with the group setting `proposalMode` on, a change goes
//! to a pull request from `proposal/<login>-<stamp>`; another member approves it, the host's
//! approval (or the host's next sync, once a member approved) merges it.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{Event, ProposalFile, ProposalView};
use crate::msg;
use claude_sync_engine::plan::FileSet;
use claude_sync_engine::traits::Remote;
use claude_sync_github::api::Pull;
use claude_sync_github::Error as GhError;
use serde_json::json;
use std::collections::BTreeMap;

pub(crate) const PREFIX: &str = "proposal/";

impl Core {
    pub(crate) fn proposal_mode(&self, repo: &str) -> bool {
        crate::groups::cached(&self.db, repo).is_some_and(|g| g.proposal_mode)
    }

    pub(crate) fn set_proposal_mode(&mut self, repo: &str, on: bool) -> Result<Event> {
        self.require_host(repo)?;
        let word = if on { "on" } else { "off" };
        self.edit_group(repo, |g| {
            g.proposal_mode = on;
            Ok(format!("proposal mode {word}"))
        })?;
        Ok(Event::Done)
    }

    /// The change as a branch and a pull request; main and the base stay as they were.
    pub(crate) fn propose(&self, repo: &str, files: &FileSet, what: &str) -> Result<Event> {
        let me = self.me()?;
        let (o, n) = crate::groups::split(repo)?;
        let c = &self.client;
        let head = self
            .remote(repo)?
            .head(None)?
            .ok_or_else(|| Error::new(msg::not_a_group(repo)))?;
        let base = c.tree(o, n, &head.sha, false)?.sha;
        let mut entries = vec![];
        for (p, b) in files {
            let sha = c.create_blob(o, n, b)?;
            entries.push(json!({"path": p, "mode": "100644", "type": "blob", "sha": sha}));
        }
        let tree = c.create_tree(o, n, &base, &entries)?;
        let title = format!("{me}: {what}");
        let commit = c.create_commit(
            o,
            n,
            &format!("{me}@{}: {what}", self.env.device),
            &tree,
            &head.sha,
        )?;
        let stamp = chrono::DateTime::from_timestamp(self.now(), 0)
            .map(|t| t.format("%Y%m%d%H%M%S").to_string())
            .unwrap_or_default();
        let branch = format!("{PREFIX}{me}-{stamp}");
        c.create_ref(o, n, &format!("refs/heads/{branch}"), &commit)?;
        let pull = c.create_pull(o, n, &title, &branch, "main")?;
        Ok(Event::Proposed(pull.number))
    }

    fn open_proposal(&self, repo: &str, number: u64) -> Result<Pull> {
        let (o, n) = crate::groups::split(repo)?;
        let p = self
            .client
            .pull(o, n, number)
            .map_err(|_| Error::new(msg::not_open(number)))?;
        let ours = p.head.as_ref().is_some_and(|h| h.name.starts_with(PREFIX));
        if p.state != "open" || !ours {
            return Err(Error::new(msg::not_open(number)));
        }
        Ok(p)
    }

    /// Open proposals. A pull request is read again only when its head sha or `updated_at`
    /// moved since the last list.
    pub(crate) fn proposals(&mut self, repo: &str) -> Result<Vec<ProposalView>> {
        let me = self.me()?;
        let (o, n) = crate::groups::split(repo)?;
        let pulls: Vec<Pull> = self
            .client
            .open_pulls(o, n)?
            .into_iter()
            .filter(|p| p.head.as_ref().is_some_and(|h| h.name.starts_with(PREFIX)))
            .collect();
        let mut out = vec![];
        let mut seen = vec![];
        for p in pulls {
            let Some(h) = &p.head else { continue };
            let key = (h.sha.clone(), p.updated_at.clone().unwrap_or_default());
            seen.push(p.number);
            let ck = (repo.to_string(), p.number);
            let mut v = match self.proposal_cache.get(&ck) {
                Some((k, v)) if *k == key => v.clone(),
                _ => {
                    let v = self.build_view(o, n, &p)?;
                    self.proposal_cache.insert(ck, (key, v.clone()));
                    v
                }
            };
            v.mine = v.author == me;
            v.approved_by_me = v.approvals.contains(&me);
            out.push(v);
        }
        self.proposal_cache
            .retain(|(r, num), _| r != repo || seen.contains(num));
        Ok(out)
    }

    fn build_view(&self, o: &str, n: &str, p: &Pull) -> Result<ProposalView> {
        let head = p.head.clone().map(|h| (h.sha, h.name)).unwrap_or_default();
        let author = p.user.as_ref().map(|u| u.login.clone()).unwrap_or_default();
        let files = self
            .client
            .pull_files(o, n, p.number)?
            .into_iter()
            .map(|f| ProposalFile {
                path: f.filename,
                patch: f.patch.unwrap_or_default(),
            })
            .collect();
        // only each person's newest review counts, on the current head, never the author's
        let mut newest: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
        for r in self.client.reviews(o, n, p.number)? {
            if let Some(u) = r.user {
                newest.insert(u.login, (r.state, r.commit_id));
            }
        }
        let approvals = newest
            .into_iter()
            .filter(|(l, (s, c))| {
                *l != author && s == "APPROVED" && c.as_deref().is_none_or(|c| c == head.0)
            })
            .map(|(l, _)| l)
            .collect();
        let needs_update = self.client.pull(o, n, p.number)?.mergeable == Some(false);
        Ok(ProposalView {
            number: p.number,
            branch: head.1,
            author,
            title: p.title.clone(),
            url: p.html_url.clone(),
            files,
            approvals,
            needs_update,
            mine: false,
            approved_by_me: false,
            head: head.0,
        })
    }

    fn is_host(&self, repo: &str) -> bool {
        self.db
            .groups()
            .is_ok_and(|g| g.iter().any(|g| g.repo == repo && g.host))
    }

    fn merge(&self, repo: &str, number: u64, head: &str) -> Result<()> {
        let (o, n) = crate::groups::split(repo)?;
        match self.client.merge_pull(o, n, number, Some(head)) {
            Ok(_) => Ok(()),
            Err(GhError::Github {
                status: 405 | 409, ..
            }) => Err(Error::new(msg::needs_update(number))),
            Err(e) => Err(e.into()),
        }
    }

    pub(crate) fn proposal_approve(&mut self, repo: &str, number: u64) -> Result<Event> {
        let me = self.me()?;
        let (o, n) = crate::groups::split(repo)?;
        let p = self.open_proposal(repo, number)?;
        if p.user.as_ref().is_some_and(|u| u.login == me) {
            return Err(Error::new(msg::own_proposal()));
        }
        if p.mergeable == Some(false) {
            return Err(Error::new(msg::needs_update(number)));
        }
        if self.is_host(repo) {
            let head = p.head.map(|h| h.sha).unwrap_or_default();
            self.merge(repo, number, &head)?;
        } else {
            self.client.approve(o, n, number)?;
        }
        Ok(Event::Done)
    }

    pub(crate) fn proposal_decline(&mut self, repo: &str, number: u64) -> Result<Event> {
        let me = self.me()?;
        let (o, n) = crate::groups::split(repo)?;
        self.open_proposal(repo, number)?;
        self.client
            .comment(o, n, number, &msg::declined_note(&me))?;
        self.client.close_pull(o, n, number)?;
        Ok(Event::Done)
    }

    /// Everything that is not mine, not behind main and not yet approved by me.
    pub(crate) fn proposals_accept_all(&mut self, repo: &str) -> Result<Event> {
        let mut first = None;
        for v in self.proposals(repo)? {
            if v.mine || v.needs_update || v.approved_by_me {
                continue;
            }
            if let Err(e) = self.proposal_approve(repo, v.number) {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(Event::Done), Err)
    }

    pub(crate) fn proposals_decline_all(&mut self, repo: &str) -> Result<Event> {
        let mut first = None;
        for v in self.proposals(repo)? {
            if let Err(e) = self.proposal_decline(repo, v.number) {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(Event::Done), Err)
    }

    /// The host's sync: merge a proposal another current member approved, unless behind main.
    pub(crate) fn merge_approved(&mut self, repo: &str) {
        if !self.is_host(repo) || !self.proposal_mode(repo) {
            return;
        }
        let Ok(list) = self.proposals(repo) else {
            return;
        };
        let members = crate::groups::cached(&self.db, repo)
            .map(|g| g.members)
            .unwrap_or_default();
        for v in list {
            let ok = v
                .approvals
                .iter()
                .any(|a| members.iter().any(|m| m.eq_ignore_ascii_case(a)));
            if !v.needs_update && ok {
                let _ = self.merge(repo, v.number, &v.head);
            }
        }
    }
}
