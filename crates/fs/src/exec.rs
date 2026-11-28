//! The executor: applies a [`Plan`] to the disk, pushes, and commits the new base last.
//!
//! Order: (1) journal, (2) every copy that would be lost (conflict losers, edited files about
//! to be replaced) saved under `conflicts/` and `backups/`, (3) writes, (4) deletes, (5) push,
//! (6) the base, in one store transaction, (7) journal removed. Killed anywhere in between,
//! the base is still the old one, so the next pass re-plans: a path already written reads as
//! "same as the group", a path not yet written as "unchanged here", a push that landed as
//! "same as the repo". Nothing a user changed is deleted, nothing is applied twice.
//!
//! v2-Parity rows covered: 24, 25, 29, 39, 43.
use crate::atomic::{sha256_hex, write_atomic};
use crate::disk::FsDisk;
use claude_sync_engine::plan::{FileSet, Plan};
use claude_sync_engine::traits::{Disk, Head, IoError, Remote};
use claude_sync_store::{BaseRec, Db};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] IoError),
    #[error("{0}")]
    File(#[from] std::io::Error),
    #[error("{0}")]
    Store(#[from] claude_sync_store::Error),
    /// The test hook stopped the pass here, as a kill would.
    #[error("stopped for the crash test")]
    Crash,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Journal {
    started: i64,
    writes: Vec<String>,
    deletes: Vec<String>,
    pushes: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Applied {
    pub written: Vec<String>,
    pub removed: Vec<String>,
    /// Paths whose local copy lost to the group and was saved.
    pub conflicts: Vec<String>,
    pub pushed: Option<String>,
    /// An earlier pass was killed half way; this one finished it.
    pub recovered: bool,
}

pub struct Exec<'a> {
    pub disk: &'a FsDisk,
    pub db: &'a Db,
    /// `owner/repo` or `personal`.
    pub repo: &'a str,
    pub now: i64,
    /// Crash test: stop with `Error::Crash` before the n-th mutation.
    pub crash_at: Cell<Option<usize>>,
}

/// `YYYYMMDDTHHMMSSZ`, Windows-safe.
pub fn utc_stamp(now: i64) -> String {
    chrono::DateTime::from_timestamp(now, 0)
        .map(|t| t.format("%Y%m%dT%H%M%SZ").to_string())
        .unwrap_or_else(|| "00000000T000000Z".into())
}

impl<'a> Exec<'a> {
    pub fn new(disk: &'a FsDisk, db: &'a Db, repo: &'a str, now: i64) -> Self {
        Exec {
            disk,
            db,
            repo,
            now,
            crash_at: Cell::new(None),
        }
    }

    fn tick(&self) -> Result<()> {
        match self.crash_at.get() {
            Some(0) => Err(Error::Crash),
            Some(n) => {
                self.crash_at.set(Some(n - 1));
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn journal_path(&self) -> PathBuf {
        self.disk
            .home
            .state
            .join("journal")
            .join(format!("{}.json", self.disk.scope.dir()))
    }

    /// A journal left by a killed pass: report it and drop it (the plan is made again).
    pub fn recover(&self) -> Result<bool> {
        let p = self.journal_path();
        let had = p.exists();
        if had {
            std::fs::remove_file(&p)?;
        }
        Ok(had)
    }

    /// Keep a copy of `bytes` under `<state>/<kind>/<scope>/<stamp>/<rel>`.
    pub fn save_copy(&self, kind: &str, rel: &str, bytes: &[u8]) -> Result<()> {
        let to = self
            .disk
            .home
            .state
            .join(kind)
            .join(self.disk.scope.dir())
            .join(utc_stamp(self.now))
            .join(rel);
        // the first copy of a path in one stamp wins; the same content again is no new copy
        if to.exists() {
            return Ok(());
        }
        Ok(write_atomic(&to, bytes)?)
    }

    /// Apply `plan`. `base`: the old base files. `head`: what the remote read gave (`None`:
    /// 304). `parent`: the head sha a push goes onto.
    pub fn run(
        &self,
        plan: &Plan,
        base: &FileSet,
        head: Option<&Head>,
        parent: &str,
        remote: &dyn Remote,
        message: &str,
    ) -> Result<Applied> {
        let mut out = Applied {
            recovered: self.recover()?,
            ..Applied::default()
        };
        if head.is_none() && plan.is_noop() {
            return Ok(out); // 304 and nothing to do: the base stays as it is
        }
        let j = Journal {
            started: self.now,
            writes: plan.writes.keys().cloned().collect(),
            deletes: plan.deletes.iter().cloned().collect(),
            pushes: plan.pushes.keys().cloned().collect(),
        };
        let skip_journal = plan.is_noop();
        if !skip_journal {
            self.tick()?;
            write_atomic(
                &self.journal_path(),
                &serde_json::to_vec(&j).unwrap_or_default(),
            )?;
        }
        // 2. copies first: whatever a write or delete replaces and the user had changed
        for c in &plan.conflicts {
            if let Some(l) = &c.local {
                self.tick()?;
                self.save_copy("conflicts", &c.path, l)?;
            }
            self.db.add_conflict(self.repo, &c.path, self.now)?;
            out.conflicts.push(c.path.clone());
        }
        for (p, new) in &plan.writes {
            let Some(have) = self.disk.read(p)? else {
                continue;
            };
            if &have != new && base.get(p) != Some(&have) {
                self.tick()?;
                self.save_copy("backups", p, &have)?;
            }
        }
        // 3. writes, 4. deletes
        for (p, bytes) in &plan.writes {
            self.tick()?;
            self.disk.write(p, bytes)?;
            out.written.push(p.clone());
        }
        for p in &plan.deletes {
            self.tick()?;
            self.disk.delete(p)?;
            out.removed.push(p.clone());
        }
        // 5. push
        let mut new_head = head.map(|h| h.sha.clone()).unwrap_or_else(|| parent.into());
        let mut etag = head.and_then(|h| h.etag.clone());
        if !plan.pushes.is_empty() {
            self.tick()?;
            new_head = remote.push(parent, &plan.pushes, message)?;
            etag = None; // our own push moved the head: the next read is a full one
            out.pushed = Some(new_head.clone());
        }
        // 6. the base, last
        self.tick()?;
        let rec = BaseRec {
            head_sha: new_head,
            etag,
            files: plan.new_base.clone(),
        };
        self.db.save_base(self.repo, &rec, self.now)?;
        // 7. done
        if !skip_journal {
            let _ = std::fs::remove_file(self.journal_path());
        }
        Ok(out)
    }
}

/// A fingerprint of a file set, for tests and for "did anything change".
pub fn fingerprint(f: &FileSet) -> String {
    let mut s = String::new();
    for (p, b) in f {
        s.push_str(&format!("{p}={}\n", sha256_hex(b)));
    }
    sha256_hex(s.as_bytes())
}
