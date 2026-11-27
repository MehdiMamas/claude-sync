//! One sync of every joined group (Parity 20, 24, 25): the commands and the loop both come
//! here, the work is `fs::pass`. Pushing is only by Share (v1: only shared paths are pushed),
//! so a pass never sends a local edit.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::Event;
use crate::msg;
use claude_sync_engine::plan::FileSet;
use claude_sync_engine::traits::Store;
use claude_sync_fs::pass::{CommitDate, GroupIn, Keep, Pass};
use claude_sync_store::RequestRow;
use std::time::Duration;

impl Core {
    pub(crate) fn show(&self) {
        if let Some(tx) = &self.publish {
            let _ = tx.send(self.snapshot());
        }
    }

    /// Sync now. A person who paused may still press it; the loop is what pause stops.
    pub(crate) fn sync_now(&mut self) -> Result<Event> {
        self.me()?;
        if self.expired.is_some() {
            return Err(Error::new(msg::SESSION_EXPIRED));
        }
        let _ = self.retry_feedback();
        self.ssh_pass();
        self.personal_discover();
        let repos: Vec<String> = self.db.groups()?.into_iter().map(|g| g.repo).collect();
        for r in repos {
            self.merge_approved(&r);
        }
        self.syncing = true;
        self.show();
        let r = self.run_pass();
        self.jobs_after_pass();
        self.plugins_after_pass();
        self.syncing = false;
        self.note_expired();
        self.drop_lost_groups();
        r
    }

    pub(crate) fn run_pass(&self) -> Result<Event> {
        let c = self.choices();
        let recs = self.db.groups()?;
        let remotes = recs
            .iter()
            .map(|g| self.remote(&g.repo))
            .collect::<Result<Vec<_>>>()?;
        let flags: Vec<_> = recs
            .iter()
            .map(|g| {
                crate::groups::cached(&self.db, &g.repo)
                    .map(|f| f.sync)
                    .unwrap_or_default()
            })
            .collect();
        let gates: Vec<_> = recs.iter().map(|g| self.gate(&g.repo)).collect();
        let dates: Vec<Box<CommitDate<'_>>> = recs
            .iter()
            .map(|g| {
                let repo = g.repo.clone();
                Box::new(move |sha: &str| {
                    let (o, n) = repo.split_once('/')?;
                    let c = self
                        .client
                        .commits(o, n, "main", 1)
                        .ok()?
                        .into_iter()
                        .next()?;
                    (c.sha == sha).then(|| c.commit.committer?.date).flatten()
                }) as Box<CommitDate<'_>>
            })
            .collect();
        let keeps: Vec<Box<Keep<'_>>> = recs
            .iter()
            .map(|g| {
                let repo = g.repo.clone();
                Box::new(move |f: Option<&FileSet>| self.pack_skipped(&repo, f)) as Box<Keep<'_>>
            })
            .collect();
        let groups: Vec<GroupIn> = recs
            .iter()
            .zip(&remotes)
            .zip(flags)
            .zip(&gates)
            .zip(&dates)
            .zip(&keeps)
            .map(|(((((g, r), flags), gate), date), keep)| GroupIn {
                repo: &g.repo,
                host: false,
                flags,
                off: g.off || c.groups.get(&g.repo).is_some_and(|x| x.off),
                remote: r,
                gate: Some(&**gate),
                commit_date: Some(&**date),
                keep: Some(&**keep),
            })
            .collect();
        let personal = self.personal_before();
        let on_group = |repo: &str| {
            if let Ok(mut c) = self.current.lock() {
                *c = Some(repo.to_string());
            }
            self.show();
        };
        let pass = Pass {
            home: &self.home,
            db: &self.db,
            now: self.now(),
            pinned: self.pack_pinned(),
            crash_at: None,
            skip_kinds: self.pchoice().kinds_off,
            on_group: Some(&on_group),
        };
        let counts = || {
            self.client
                .take_counts()
                .into_iter()
                .map(|c| RequestRow {
                    caller: c.caller,
                    endpoint: c.endpoint,
                    status: c.result,
                    n: c.count,
                })
                .collect()
        };
        let rep = pass.run(
            &groups,
            personal
                .as_ref()
                .map(|r| r as &dyn claude_sync_engine::traits::Remote),
            None,
            Some(&counts),
            Duration::from_secs(10),
        );
        self.personal_after(&rep, personal.as_ref());
        if rep.busy {
            return Err(Error::new(
                "Another sync is running; try again in a moment.",
            ));
        }
        if let Ok(mut c) = self.current.lock() {
            *c = None;
        }
        let (mut files, mut failed, mut repos) = (0, 0, vec![]);
        for (repo, r) in rep.groups {
            match r {
                Ok(a) => {
                    let n = a.written.len() + a.removed.len();
                    files += n;
                    if n > 0 {
                        repos.push(repo.clone());
                    }
                    self.db.set_group_state(&repo, "ok")?;
                    let _ = Store::write(&self.db, &format!("failed-at/{repo}"), b"");
                }
                Err(e) => {
                    failed += 1;
                    self.db.set_group_state(&repo, &e)?;
                    let _ = Store::write(
                        &self.db,
                        &format!("failed-at/{repo}"),
                        self.now().to_string().as_bytes(),
                    );
                }
            }
        }
        Ok(Event::Synced {
            files,
            failed,
            repos,
        })
    }

    /// v1 `lose`: a group whose head GitHub refused (403 or 404) leaves this machine: its
    /// delivered files go (a hand-edited one stays) and one notification says so.
    fn drop_lost_groups(&mut self) {
        let lost: Vec<String> = self
            .db
            .groups()
            .unwrap_or_default()
            .into_iter()
            .filter(|g| g.state.starts_with(claude_sync_fs::pass::LOST_ACCESS))
            .map(|g| g.repo)
            .collect();
        for repo in lost {
            if self.leave(&repo).is_ok() {
                let _ = self.db.add_notification(
                    "group",
                    &msg::access_gone(&repo),
                    Some(&repo),
                    self.now(),
                );
            }
        }
    }
}
