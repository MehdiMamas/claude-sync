//! One sync pass: lock, then each group in rank order (head, plan, execute, base last), the
//! settings merge and the CLAUDE.md block, then the personal repo, then SSH peers; notes,
//! conflicts and request counts are recorded at the end. The pass takes its remotes as
//! arguments, so a test drives it with fakes and a temp home.
//!
//! v2-Parity rows covered: 24, 25, 29, 37, 39, 43, 46 (with exec.rs and apply.rs).
use crate::apply;
use crate::disk::{FsDisk, Home, Scope};
use crate::exec::{Applied, Exec};
use claude_sync_engine::formats::group::SyncFlags;
use claude_sync_engine::paths::is_planned_path;
use claude_sync_engine::plan::{plan, FileSet, Opts, Plan, Scope as PlanScope};
use claude_sync_engine::traits::{Disk, Head, Remote};
use claude_sync_store::{blob_sha, Db, RequestRow, PERSONAL};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

/// `<state>/sync.lock`, as in v1: held for the whole pass, shared by the app and the CLI. A
/// lock file that cannot be created means running unlocked.
pub struct SyncLock(Option<File>);

#[derive(Debug, PartialEq, Eq)]
pub struct Busy;

impl SyncLock {
    pub fn acquire(path: &Path, wait: Duration) -> Result<SyncLock, Busy> {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let Ok(f) = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
        else {
            return Ok(SyncLock(None));
        };
        let start = Instant::now();
        loop {
            match f.try_lock() {
                Ok(()) => return Ok(SyncLock(Some(f))),
                Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < wait => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(std::fs::TryLockError::WouldBlock) => return Err(Busy),
                Err(_) => return Ok(SyncLock(None)),
            }
        }
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        if let Some(f) = &self.0 {
            let _ = f.unlock();
        }
    }
}

/// Review gate: gets the remote CLAUDE.md text, returns what may be imported.
pub type Gate = dyn Fn(Option<&[u8]>) -> Option<Vec<u8>>;
/// Committer date for a head sha (the `COMMIT` marker's second line).
pub type CommitDate = dyn Fn(&str) -> Option<String>;

pub struct GroupIn<'a> {
    pub repo: &'a str,
    /// This device hosts the group: it pushes.
    pub host: bool,
    pub flags: SyncFlags,
    /// The member switched the group off here.
    pub off: bool,
    pub remote: &'a dyn Remote,
    /// Review gating: gets the group's CLAUDE.md text as the remote has it and returns the text
    /// to place in the copy that is imported (the accepted text). `None`: copy as is.
    pub gate: Option<&'a Gate>,
    /// Committer date of a head sha, for the marketplace `COMMIT` file (`None`: sha only).
    pub commit_date: Option<&'a CommitDate>,
}

pub struct Pass<'a> {
    pub home: &'a Home,
    pub db: &'a Db,
    pub now: i64,
    /// Plugin pointers of packs that are on here (not edits, kept by the settings merge).
    pub pinned: BTreeSet<String>,
    /// Crash test: stop every executor before its n-th mutation.
    pub crash_at: Option<usize>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub busy: bool,
    pub groups: Vec<(String, Result<Applied, String>)>,
    pub personal: Option<Result<Applied, String>>,
    pub ssh: Option<Result<String, String>>,
    pub notifications: Vec<String>,
}

fn blobs(f: &FileSet) -> BTreeMap<String, String> {
    f.iter().map(|(p, b)| (p.clone(), blob_sha(b))).collect()
}

impl Pass<'_> {
    /// One repo: read the head (304: nothing new), plan, execute.
    #[allow(clippy::too_many_arguments)]
    fn one(
        &self,
        repo: &str,
        disk: &FsDisk,
        remote: &dyn Remote,
        keep: &dyn Fn(&str) -> bool,
        scope: PlanScope,
        push: bool,
        raw: &mut Option<FileSet>,
    ) -> Result<(Applied, Plan), String> {
        let s = |e: &dyn std::fmt::Display| e.to_string();
        let base = self
            .db
            .load_base(repo, &self.home.state)
            .map_err(|e| s(&e))?;
        let etag = base.as_ref().and_then(|b| b.etag.clone());
        let head: Option<Head> = remote.head(etag.as_deref()).map_err(|e| s(&e))?;
        let base_files = base.as_ref().map(|b| b.files.clone()).unwrap_or_default();
        let remote_all = match &head {
            Some(h) => remote
                .files(&h.sha, &blobs(&base_files))
                .map_err(|e| s(&e))?,
            None => base_files.clone(),
        };
        let remote_files: FileSet = remote_all
            .iter()
            .filter(|(p, _)| keep(p))
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect();
        let known: BTreeSet<String> = base_files
            .keys()
            .chain(remote_files.keys())
            .cloned()
            .collect();
        let local = disk.scan(&known).map_err(|e| s(&e))?;
        let links: BTreeSet<String> = disk.links().map_err(|e| s(&e))?.into_iter().collect();
        let p = plan(
            &remote_files,
            &base_files,
            &local,
            &Opts { scope, push, links },
        );
        let parent = head
            .as_ref()
            .map(|h| h.sha.clone())
            .or_else(|| base.as_ref().map(|b| b.head_sha.clone()))
            .unwrap_or_default();
        let ex = Exec::new(disk, self.db, repo, self.now);
        ex.crash_at.set(self.crash_at);
        let applied = ex
            .run(
                &p,
                &base_files,
                head.as_ref(),
                &parent,
                remote,
                "claude-sync",
            )
            .map_err(|e| s(&e))?;
        if head.is_some() {
            *raw = Some(remote_all);
        }
        Ok((applied, p))
    }

    fn note(&self, rep: &mut Report, repo: Option<&str>, kind: &str, text: String) {
        let _ = self.db.add_notification(kind, &text, repo, self.now);
        rep.notifications.push(text);
    }

    fn group(&self, g: &GroupIn, rep: &mut Report) -> Result<Applied, String> {
        let disk = FsDisk::new(self.home, Scope::Group(g.repo.into()));
        let mut raw = None;
        let (applied, p) = self.one(
            g.repo,
            &disk,
            g.remote,
            &is_planned_path,
            PlanScope::Group(g.flags.clone()),
            g.host,
            &mut raw,
        )?;
        if let Some(all) = raw {
            let md = if g.flags.claude_md {
                all.get("CLAUDE.md").map(|b| b.as_slice())
            } else {
                None
            };
            let gated = g.gate.and_then(|f| f(md));
            let md = if g.gate.is_some() {
                gated.as_deref()
            } else {
                md
            };
            apply::write_claude_md_copy(self.home, g.repo, md).map_err(|e| e.to_string())?;
            if let Ok(Some(b)) = self.db.load_base(g.repo, &self.home.state) {
                let paths: BTreeSet<&String> = p.new_base.keys().collect();
                let date = g.commit_date.and_then(|f| f(&b.head_sha));
                apply::write_commit_markers(self.home, &paths, &b.head_sha, date.as_deref())
                    .map_err(|e| e.to_string())?;
            }
        }
        for c in &p.conflicts {
            self.note(
                rep,
                Some(g.repo),
                "conflict",
                format!(
                    "{}: your copy of {} was saved; the group's version was applied",
                    g.repo, c.path
                ),
            );
        }
        for (path, _) in &p.secrets {
            self.note(
                rep,
                Some(g.repo),
                "secret",
                format!(
                    "{}: {path} looks like it holds a secret and was not shared",
                    g.repo
                ),
            );
        }
        Ok(applied)
    }

    fn personal(&self, remote: &dyn Remote) -> Result<Applied, String> {
        let disk = FsDisk::new(self.home, Scope::Personal);
        let keep = |p: &str| disk.locate(p).is_some();
        let mut raw = None;
        let (a, _) = self.one(
            PERSONAL,
            &disk,
            remote,
            &keep,
            PlanScope::Personal,
            true,
            &mut raw,
        )?;
        Ok(a)
    }

    /// Groups in rank order, then personal, then `ssh`. `counts` yields the request counts to
    /// flush. A busy lock ends the pass with `busy`.
    pub fn run(
        &self,
        groups: &[GroupIn],
        personal: Option<&dyn Remote>,
        ssh: Option<&dyn Fn() -> Result<String, String>>,
        counts: Option<&dyn Fn() -> Vec<RequestRow>>,
        wait: Duration,
    ) -> Report {
        let mut rep = Report::default();
        let Ok(_lock) = SyncLock::acquire(&self.home.lock_file(), wait) else {
            rep.busy = true;
            return rep;
        };
        for g in groups.iter().filter(|g| !g.off) {
            let r = self.group(g, &mut rep);
            rep.groups.push((g.repo.to_string(), r));
        }
        // a killed pass stops here, as a kill would (crash test only)
        if self.crash_at.is_some() && rep.groups.iter().any(|(_, r)| r.is_err()) {
            return rep;
        }
        // settings: each group in rank order; a pointer an earlier group owns is not taken again
        let mut taken: BTreeMap<String, String> = BTreeMap::new();
        for g in groups {
            let r: Result<(), String> = if g.off {
                apply::unapply_settings(self.home, self.db, g.repo, self.now).map(|_| ())
            } else {
                let shared =
                    std::fs::read(apply::copy_path(self.home, g.repo, "settings.shared.json")).ok();
                let shared = shared.filter(|_| g.flags.settings);
                apply::apply_settings(
                    self.home,
                    self.db,
                    g.repo,
                    shared.as_deref(),
                    &taken,
                    &self.pinned,
                    self.now,
                )
                .map(|o| {
                    for n in o.notes {
                        let _ = self
                            .db
                            .add_notification("settings", &n, Some(g.repo), self.now);
                    }
                })
            };
            if let Err(e) = r {
                self.note(&mut rep, Some(g.repo), "settings", e);
            }
            let o = apply::load_owned(self.db, g.repo);
            for p in o.values.keys().chain(o.array_items.keys()) {
                taken.entry(p.clone()).or_insert_with(|| g.repo.to_string());
            }
        }
        let order: Vec<(String, bool)> =
            groups.iter().map(|g| (g.repo.to_string(), g.off)).collect();
        if let Err(e) = apply::update_block(self.home, &order, self.now) {
            self.note(
                &mut rep,
                None,
                "claude-md",
                format!("CLAUDE.md was not updated: {e}"),
            );
        }
        if let Some(r) = personal {
            rep.personal = Some(self.personal(r));
            if self.crash_at.is_some() && matches!(rep.personal, Some(Err(_))) {
                return rep;
            }
        }
        if let Some(f) = ssh {
            rep.ssh = Some(f());
        }
        if let Some(c) = counts {
            let _ = self.db.add_requests(&c());
        }
        rep
    }
}
