//! One sync pass: lock, then each group in rank order (head, plan, execute, base last), the
//! settings merge and the CLAUDE.md block, then the personal repo, then SSH peers; notes,
//! conflicts and request counts are recorded at the end. The pass takes its remotes as
//! arguments, so a test drives it with fakes and a temp home.
//!
//! v2-Parity rows covered: 24, 25, 29, 37, 39, 43, 46 (with exec.rs and apply.rs).
use crate::apply;
use crate::disk::{FsDisk, Home, Scope};
use crate::exec::{Applied, Exec};
use claude_sync_engine::formats::group::{SyncFlags, FILE, MAX_PLUGIN_FILE};
use claude_sync_engine::paths::is_planned_path;
use claude_sync_engine::plan::{plan, FileSet, Opts, Plan, Scope as PlanScope};
use claude_sync_engine::traits::{Disk, Head, Remote, Store};
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
pub type Gate<'a> = dyn Fn(Option<&[u8]>) -> Option<Vec<u8>> + 'a;
/// Committer date for a head sha (the `COMMIT` marker's second line).
pub type CommitDate<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// Group paths a pack that is off keeps from being placed (a path covers what is under it).
/// Gets the files of the head when it moved, `None` when it did not (use what was kept).
pub type Keep<'a> = dyn Fn(Option<&FileSet>) -> BTreeSet<String> + 'a;

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
    pub gate: Option<&'a Gate<'a>>,
    /// Committer date of a head sha, for the marketplace `COMMIT` file (`None`: sha only).
    pub commit_date: Option<&'a CommitDate<'a>>,
    /// Pack gating (Parity 79): paths of packs that are not on, left out of the remote side so
    /// the plan takes away what was placed (an edited file stays).
    pub keep: Option<&'a Keep<'a>>,
}

pub struct Pass<'a> {
    pub home: &'a Home,
    pub db: &'a Db,
    pub now: i64,
    /// Plugin pointers of packs that are on here (not edits, kept by the settings merge).
    pub pinned: BTreeSet<String>,
    /// Crash test: stop every executor before its n-th mutation.
    pub crash_at: Option<usize>,
    /// Personal kinds this person switched off (`claudeMd`, `memory`, `state`, `settings`):
    /// their files leave the base and are neither pulled nor pushed until switched back on.
    pub skip_kinds: BTreeSet<String>,
    /// Called with the repo before each group is synced.
    pub on_group: Option<&'a dyn Fn(&str)>,
}

/// The personal kind a repo path belongs to; the marker, devices and machines files have none.
pub fn personal_kind(path: &str) -> Option<&'static str> {
    match path.split('/').next()? {
        "CLAUDE.md" => Some("claudeMd"),
        "settings.json" => Some("settings"),
        "memory" => Some("memory"),
        "state" => Some("state"),
        _ => None,
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub busy: bool,
    pub groups: Vec<(String, Result<Applied, String>)>,
    pub personal: Option<Result<Applied, String>>,
    pub ssh: Option<Result<String, String>>,
    pub notifications: Vec<String>,
}

/// A bundled-marketplace file over the cap is never placed (Parity 28).
fn oversize(path: &str, bytes: &[u8]) -> bool {
    path.starts_with("plugins/") && bytes.len() > MAX_PLUGIN_FILE
}

/// The sync switches as the head's group file has them now (a pass never runs on last time's
/// switches); the kept ones when the head did not move or the file does not read.
fn fresh_flags(all: Option<&FileSet>, kept: &SyncFlags) -> SyncFlags {
    all.and_then(|a| a.get(FILE))
        .and_then(|b| claude_sync_engine::formats::group::GroupFile::open(b).ok())
        .map(|d| d.value.sync)
        .unwrap_or_else(|| kept.clone())
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
        skip: Option<&Keep>,
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
        let base_files: FileSet = base
            .as_ref()
            .map(|b| b.files.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|(p, _)| keep(p))
            .collect();
        let remote_all = match &head {
            Some(h) => remote
                .files(&h.sha, &blobs(&base_files))
                .map_err(|e| s(&e))?,
            None => base_files.clone(),
        };
        let scope = match scope {
            PlanScope::Group(kept) => {
                PlanScope::Group(fresh_flags(head.is_some().then_some(&remote_all), &kept))
            }
            other => other,
        };
        let skipped = skip.map(|f| f(head.is_some().then_some(&remote_all)));
        let off = |p: &str| {
            skipped.as_ref().is_some_and(|s| {
                s.iter().any(|x| {
                    p == x
                        || p.strip_prefix(x.as_str())
                            .is_some_and(|r| r.starts_with('/'))
                })
            })
        };
        let remote_files: FileSet = remote_all
            .iter()
            .filter(|(p, b)| keep(p) && !off(p) && !oversize(p, b))
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect();
        let known: BTreeSet<String> = base_files
            .keys()
            .chain(remote_files.keys())
            .cloned()
            .collect();
        let mut local = disk.scan(&known).map_err(|e| s(&e))?;
        local.retain(|p, _| keep(p));
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
            g.keep,
            PlanScope::Group(g.flags.clone()),
            g.host,
            &mut raw,
        )?;
        if let Some(all) = raw {
            // the group file as read, so the flags of the next pass need no network
            if let Some(b) = all.get(FILE) {
                let _ = Store::write(self.db, &format!("group-file/{}", g.repo), b);
            }
            // repo-only, never applied: the jobs of the group are read from this copy
            // the files the size cap kept out, for the plugin warnings
            let big: Vec<&str> = all
                .iter()
                .filter(|(p, b)| oversize(p, b))
                .map(|(p, _)| p.as_str())
                .collect();
            let _ = Store::write(
                self.db,
                &format!("plugin-skipped/{}", g.repo),
                big.join("\n").as_bytes(),
            );
            let sched = all.get("schedules.json").map(|b| b.as_slice());
            let _ = Store::write(
                self.db,
                &format!("schedules/{}", g.repo),
                sched.unwrap_or_default(),
            );
            let md = if fresh_flags(Some(&all), &g.flags).claude_md {
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
                let has_mp = paths.iter().any(|p| p.starts_with("plugins/"));
                let date = g
                    .commit_date
                    .filter(|_| has_mp)
                    .and_then(|f| f(&b.head_sha));
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

    /// Leaving a group: its delivered files go, except any the user changed (those stay, as
    /// local files), and so do the `COMMIT` markers of its marketplaces. Returns the paths
    /// removed.
    pub fn leave(&self, repo: &str) -> Result<Vec<String>, String> {
        let s = |e: &dyn std::fmt::Display| e.to_string();
        let disk = FsDisk::new(self.home, Scope::Group(repo.into()));
        let base = self
            .db
            .load_base(repo, &self.home.state)
            .map_err(|e| s(&e))?
            .map(|b| b.files)
            .unwrap_or_default();
        let known: BTreeSet<String> = base.keys().cloned().collect();
        let local = disk.scan(&known).map_err(|e| s(&e))?;
        let links: BTreeSet<String> = disk.links().map_err(|e| s(&e))?.into_iter().collect();
        let opts = Opts {
            scope: PlanScope::Group(SyncFlags::default()),
            push: false,
            links,
        };
        let p = plan(&FileSet::new(), &base, &local, &opts);
        let mut gone = vec![];
        for d in &p.deletes {
            disk.delete(d).map_err(|e| s(&e))?;
            gone.push(d.clone());
        }
        let mps: BTreeSet<&str> = base
            .keys()
            .filter_map(|p| p.strip_prefix("plugins/")?.split('/').next())
            .collect();
        for mp in mps {
            let f = self.home.state.join("marketplaces").join(mp).join("COMMIT");
            if f.exists() {
                std::fs::remove_file(&f).map_err(|e| s(&e))?;
            }
        }
        Ok(gone)
    }

    fn personal(&self, remote: &dyn Remote, rep: &mut Report) -> Result<Applied, String> {
        let disk = FsDisk::new(self.home, Scope::Personal);
        let keep = |p: &str| {
            disk.locate(p).is_some()
                && !personal_kind(p).is_some_and(|k| self.skip_kinds.contains(k))
        };
        let mut raw = None;
        let (a, p) = self.one(
            PERSONAL,
            &disk,
            remote,
            &keep,
            None,
            PlanScope::Personal,
            true,
            &mut raw,
        )?;
        // what stays on this device, as the window lists it (Parity 44)
        let mut stays = p.notes.clone();
        stays.extend(p.secrets.iter().map(|(f, h)| {
            format!(
                "{f}: not synced, line {} looks like a secret ({})",
                h.line, h.pattern
            )
        }));
        let _ = Store::write(
            self.db,
            "personal/stays",
            &serde_json::to_vec(&stays).unwrap_or_default(),
        );
        for c in &p.conflicts {
            self.note(
                rep,
                None,
                "conflict",
                format!(
                    "personal: your copy of {} was saved; the other device's version was applied",
                    c.path
                ),
            );
        }
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
            if let Some(f) = self.on_group {
                f(g.repo);
            }
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
                // the switches the group file has now: group() just stored it
                let set = Store::read(self.db, &format!("group-file/{}", g.repo))
                    .ok()
                    .flatten()
                    .map(|b| FileSet::from([(FILE.to_string(), b)]));
                let shared = shared.filter(|_| fresh_flags(set.as_ref(), &g.flags).settings);
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
            let r = self.personal(r, &mut rep);
            rep.personal = Some(r);
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
