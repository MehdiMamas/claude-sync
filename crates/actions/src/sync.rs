//! One sync of every joined group (Parity 20, 24, 25): the commands and the loop both come
//! here, the work is `fs::pass`. Pushing is only by Share (v1: only shared paths are pushed),
//! so a pass never sends a local edit.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::Event;
use crate::msg;
use claude_sync_fs::pass::{GroupIn, Pass};
use claude_sync_store::RequestRow;
use std::collections::BTreeSet;
use std::time::Duration;

impl Core {
    fn show(&self) {
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
        self.syncing = true;
        self.show();
        let r = self.run_pass();
        self.syncing = false;
        self.note_expired();
        r
    }

    fn run_pass(&self) -> Result<Event> {
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
        let groups: Vec<GroupIn> = recs
            .iter()
            .zip(&remotes)
            .zip(flags)
            .map(|((g, r), flags)| GroupIn {
                repo: &g.repo,
                host: false,
                flags,
                off: g.off || c.groups.get(&g.repo).is_some_and(|x| x.off),
                remote: r,
                gate: None,
                commit_date: None,
            })
            .collect();
        let pass = Pass {
            home: &self.home,
            db: &self.db,
            now: self.now(),
            pinned: BTreeSet::new(),
            crash_at: None,
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
        let rep = pass.run(&groups, None, None, Some(&counts), Duration::from_secs(10));
        if rep.busy {
            return Err(Error::new(
                "Another sync is running; try again in a moment.",
            ));
        }
        let (mut files, mut failed) = (0, 0);
        for (repo, r) in rep.groups {
            match r {
                Ok(a) => {
                    files += a.written.len() + a.removed.len();
                    self.db.set_group_state(&repo, "ok")?;
                }
                Err(e) => {
                    failed += 1;
                    self.db.set_group_state(&repo, &e)?;
                }
            }
        }
        Ok(Event::Synced { files, failed })
    }
}
