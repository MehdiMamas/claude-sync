//! Groups and members (Parity 4-9, 12-15). Group-file edits go through `Doc<GroupFile>` so only
//! the changed keys are written, then one commit with the retries v1 had.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::Event;
use crate::msg;
use claude_sync_engine::formats::group::{GroupFile, FILE, TOPIC};
use claude_sync_engine::formats::Doc;
use claude_sync_engine::traits::{Remote, Store};
use claude_sync_github::api::HOST_LOCK_NAME;
use claude_sync_github::remote::GithubRemote;
use claude_sync_store::Db;
use std::collections::BTreeMap;

pub(crate) fn split(repo: &str) -> Result<(&str, &str)> {
    repo.split_once('/')
        .filter(|(o, n)| !o.is_empty() && !n.is_empty() && !n.contains('/'))
        .ok_or_else(|| Error::new(format!("{repo:?} is not an owner/name repo")))
}

/// The group file as last read, kept in the store so the snapshot needs no network.
pub(crate) fn cached(db: &Db, repo: &str) -> Option<GroupFile> {
    let b = Store::read(db, &format!("group-file/{repo}"))
        .ok()
        .flatten()?;
    GroupFile::open(&b).ok().map(|d| d.value)
}

fn valid_login(l: &str) -> bool {
    !l.is_empty()
        && l.len() <= 39
        && !l.starts_with('-')
        && !l.ends_with('-')
        && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl Core {
    pub(crate) fn remote(&self, repo: &str) -> Result<GithubRemote> {
        let (o, n) = split(repo)?;
        Ok(GithubRemote::new(self.client.clone(), o, n, "main"))
    }

    /// The group file at the head; a repo without one is not a group.
    fn read_group(&self, repo: &str) -> Result<(String, Doc<GroupFile>)> {
        let r = self.remote(repo)?;
        let head = r
            .head(None)?
            .ok_or_else(|| Error::new(msg::not_a_group(repo)))?;
        let files = r.files(&head.sha, &BTreeMap::new())?;
        let bytes = files
            .get(FILE)
            .ok_or_else(|| Error::new(msg::not_a_group(repo)))?;
        let doc = GroupFile::open(bytes)?;
        Store::write(&self.db, &format!("group-file/{repo}"), bytes)?;
        Ok((head.sha, doc))
    }

    pub(crate) fn require_host(&self, repo: &str) -> Result<()> {
        let is_host = self
            .db
            .groups()?
            .into_iter()
            .any(|g| g.repo == repo && g.host);
        if is_host {
            return Ok(());
        }
        let host = repo.split('/').next().unwrap_or(repo);
        Err(Error::new(msg::host_only(host, repo)))
    }

    /// Change the group file and push it; retried when someone else pushed first.
    pub(crate) fn edit_group(
        &self,
        repo: &str,
        f: impl Fn(&mut GroupFile) -> Result<String>,
    ) -> Result<()> {
        let me = self.me()?;
        for _ in 0..3 {
            let (sha, mut doc) = self.read_group(repo)?;
            let summary = f(&mut doc.value)?;
            let bytes = doc.to_bytes()?;
            let mut push = BTreeMap::new();
            push.insert(FILE.to_string(), Some(bytes.clone()));
            let msg = format!("{me}@{}: {summary}", self.env.device);
            match self.remote(repo)?.push(&sha, &push, &msg) {
                Ok(_) => {
                    Store::write(&self.db, &format!("group-file/{repo}"), &bytes)?;
                    return Ok(());
                }
                Err(e) if e.0.contains("someone else pushed first") => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::new(msg::push_retries()))
    }

    pub(crate) fn create_group(&mut self, name: &str, repo: &str) -> Result<Event> {
        let me = self.me()?;
        let created = self
            .client
            .create_repo(repo, &format!("claude-sync group: {name}"))?;
        let (o, n) = split(&created.full_name)?;
        let (o, n) = (o.to_string(), n.to_string());
        self.client.set_topics(&o, &n, &[TOPIC])?;
        let lock = match self.client.rulesets(&o, &n) {
            Ok(names) if names.iter().any(|x| x == HOST_LOCK_NAME) => "on",
            _ => match self.client.create_ruleset(&o, &n) {
                Ok(()) => "on",
                Err(_) => "unavailable",
            },
        };
        let doc = Doc::new(GroupFile::new(name, &me));
        let bytes = doc.to_bytes()?;
        let r = GithubRemote::new(
            self.client.clone(),
            &o,
            &n,
            &created.default_branch.clone().unwrap_or("main".into()),
        );
        let parent = r.head(None)?.map(|h| h.sha).unwrap_or_default();
        let mut push = BTreeMap::new();
        push.insert(FILE.to_string(), Some(bytes.clone()));
        r.push(
            &parent,
            &push,
            &format!("{me}@{}: create group {name}", self.env.device),
        )?;
        Store::write(
            &self.db,
            &format!("group-file/{}", created.full_name),
            &bytes,
        )?;
        Store::write(
            &self.db,
            &format!("host-lock/{}", created.full_name),
            lock.as_bytes(),
        )?;
        self.db.add_group(&created.full_name, true)?;
        Ok(Event::Created {
            repo: created.full_name,
            lock: lock.into(),
        })
    }

    pub(crate) fn discover(&mut self) -> Result<Event> {
        let joined: Vec<String> = self
            .db
            .groups()?
            .into_iter()
            .map(|g| g.repo.to_lowercase())
            .collect();
        let found: Vec<String> = self
            .client
            .user_repos()?
            .into_iter()
            .filter(|r| {
                r.private
                    && r.topics
                        .as_ref()
                        .is_some_and(|t| t.iter().any(|x| x == TOPIC))
            })
            .map(|r| r.full_name)
            .filter(|n| !joined.contains(&n.to_lowercase()))
            .collect();
        self.discoverable = found.clone();
        Ok(Event::Discovered(found))
    }

    pub(crate) fn join(&mut self, repo: &str) -> Result<Event> {
        let (o, n) = split(repo)?;
        let info = self.client.repo(o, n)?;
        if !info.private {
            return Err(Error::new(msg::public_repo(&info.full_name)));
        }
        let (_, doc) = self.read_group(&info.full_name)?;
        let me = self.me()?;
        self.db
            .add_group(&info.full_name, o.eq_ignore_ascii_case(&me))?;
        self.discoverable
            .retain(|d| !d.eq_ignore_ascii_case(&info.full_name));
        let warning = if doc.value.sync.plugins {
            msg::PLUGINS_WARNING.to_string()
        } else {
            String::new()
        };
        Ok(Event::Joined {
            repo: info.full_name,
            warning,
        })
    }

    pub(crate) fn accept_invitation(&mut self, id: u64) -> Result<Event> {
        self.client.accept_invitation(id)?;
        self.invitations.retain(|i| i.id != id);
        Ok(Event::Done)
    }

    pub(crate) fn add_member(&mut self, repo: &str, login: &str) -> Result<Event> {
        if !valid_login(login) {
            return Err(Error::new(msg::not_a_login(login)));
        }
        self.require_host(repo)?;
        let (o, n) = split(repo)?;
        self.client.invite(o, n, login)?;
        self.edit_group(repo, |g| {
            if !g.has_member(login) {
                g.members.push(login.to_string());
            }
            Ok(format!("add {login}"))
        })?;
        Ok(Event::Message(msg::invited(login)))
    }

    pub(crate) fn remove_member(&mut self, repo: &str, login: &str) -> Result<Event> {
        self.require_host(repo)?;
        let (o, n) = split(repo)?;
        if o.eq_ignore_ascii_case(login) {
            return Err(Error::new(msg::host_cannot_be_removed(login, repo)));
        }
        self.edit_group(repo, |g| {
            g.members.retain(|m| !m.eq_ignore_ascii_case(login));
            Ok(format!("remove {login}"))
        })?;
        for i in self.client.repo_invitations(o, n).unwrap_or_default() {
            if i.invitee
                .as_ref()
                .is_some_and(|x| x.login.eq_ignore_ascii_case(login))
            {
                self.client.cancel_invitation(o, n, i.id)?;
            }
        }
        // a 404 means they were only invited
        match self.client.remove_collaborator(o, n, login) {
            Ok(()) => {}
            Err(e) if e.status() == Some(404) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(Event::Done)
    }

    pub(crate) fn transfer(&mut self, repo: &str, new_owner: &str) -> Result<Event> {
        self.require_host(repo)?;
        let me = self.me()?;
        if new_owner.eq_ignore_ascii_case(&me) {
            return Err(Error::new(msg::already_host(repo)));
        }
        let (_, doc) = self.read_group(repo)?;
        if !doc.value.has_member(new_owner) {
            return Err(Error::new(msg::not_a_member(new_owner, repo)));
        }
        let (o, n) = split(repo)?;
        self.client.transfer(o, n, new_owner)?;
        Ok(Event::Message(msg::transfer_asked(new_owner)))
    }

    pub(crate) fn leave(&mut self, repo: &str) -> Result<Event> {
        if self.db.groups()?.iter().any(|g| g.repo == repo && g.host) {
            return Err(Error::new(msg::host_cannot_leave(repo)));
        }
        self.unregister_group_jobs(repo)?;
        let now = self.now();
        let _ = claude_sync_fs::apply::unapply_settings(&self.home, &self.db, repo, now);
        let pass = claude_sync_fs::pass::Pass {
            home: &self.home,
            db: &self.db,
            now,
            pinned: Default::default(),
            crash_at: None,
            skip_kinds: Default::default(),
        };
        // delivered files go; a file the person changed stays (as a local file)
        pass.leave(repo).map_err(Error::new)?;
        crate::review::clear(&self.db, repo);
        claude_sync_fs::apply::write_claude_md_copy(&self.home, repo, None)?;
        self.db.remove_group(repo)?;
        let rest: Vec<(String, bool)> = self
            .db
            .groups()?
            .into_iter()
            .map(|g| (g.repo, g.off))
            .collect();
        claude_sync_fs::apply::update_block(&self.home, &rest, now)?;
        let mut c = self.choices();
        c.groups.remove(repo);
        c.save(&self.db)?;
        Ok(Event::Message(msg::left(
            repo.split('/').nth(1).unwrap_or(repo),
        )))
    }

    pub(crate) fn set_off(&mut self, repo: &str, off: bool) -> Result<Event> {
        self.db.set_group_off(repo, off)?;
        let ts = self.now() as u64 * 1000;
        let mut c = self.choices();
        c.group(repo, ts).off = off;
        c.save(&self.db)?;
        Ok(Event::Done)
    }

    pub(crate) fn reorder(&mut self, list: Vec<String>) -> Result<Event> {
        let mut have: Vec<String> = self.db.groups()?.into_iter().map(|g| g.repo).collect();
        let mut want = list.clone();
        have.sort();
        want.sort();
        if have != want {
            return Err(Error::new(msg::order_mismatch()));
        }
        self.db.set_group_order(&list)?;
        let mut c = self.choices();
        c.order.list = list;
        c.order.ts = self.now() as u64 * 1000;
        c.save(&self.db)?;
        Ok(Event::Done)
    }
}
