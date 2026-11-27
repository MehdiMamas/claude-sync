//! The state the actor owns, and the dispatch from [`Command`] to the code that does it.
use crate::actor::Msg;
use crate::choices::Choices;
use crate::error::{Error, Result};
use crate::model::*;
use crate::msg;
use crate::schedule::{Facts, Schedule};
use claude_sync_engine::traits::Store;
use claude_sync_fs::Home;
use claude_sync_github::{secrets::SecretStore, Client, Config};
use claude_sync_store::Db;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::mpsc::WeakSender;

/// Everything outside the process the actor needs, injected so tests use temp folders and a
/// fake GitHub.
#[derive(Clone)]
pub struct Env {
    /// Treated as the person's home: `.claude`, `.claude-sync` and `.ssh` sit under it.
    pub home: std::path::PathBuf,
    pub cfg: Config,
    /// Client id of the GitHub App; empty: no App mode in this build.
    pub app_client_id: String,
    pub secrets: Secrets,
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// `login@device` signature for commit messages.
    pub device: String,
    /// The process list and the kill for the v1 daemon hand-over.
    pub procs: Arc<dyn crate::handover::Procs>,
}

/// The keychain (or a test map) behind one clonable handle.
#[derive(Clone)]
pub struct Secrets(pub Arc<dyn SecretStore + Send + Sync>);

impl SecretStore for Secrets {
    fn get(&self, a: &str) -> claude_sync_github::Result<Option<String>> {
        self.0.get(a)
    }
    fn set(&self, a: &str, s: &str) -> claude_sync_github::Result<()> {
        self.0.set(a, s)
    }
    fn delete(&self, a: &str) -> claude_sync_github::Result<()> {
        self.0.delete(a)
    }
}

pub struct Core {
    pub(crate) env: Env,
    pub(crate) home: Home,
    pub(crate) db: Db,
    pub(crate) client: Client,
    pub(crate) login: Option<String>,
    pub(crate) app_mode: bool,
    pub(crate) app_mode_pending: bool,
    pub(crate) app_uncovered: Vec<String>,
    pub(crate) signin: SignInState,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) tx: Option<WeakSender<Msg>>,
    pub(crate) expired: Option<String>,
    pub(crate) schedule: Schedule,
    pub(crate) invitations: Vec<InvitationView>,
    pub(crate) discoverable: Vec<crate::model::DiscoverView>,
    /// Host groups: logins whose invitation GitHub still holds (read with the invitations).
    pub(crate) pending_invites: std::collections::HashMap<String, Vec<String>>,
    /// The group the pass is syncing now.
    pub(crate) current: std::sync::Mutex<Option<String>>,
    pub(crate) syncing: bool,
    /// Groups whose `claude` calls run in a task now (only the actor changes state).
    pub(crate) plugins_running: std::collections::HashSet<String>,
    pub(crate) plugin_backoff: std::collections::HashMap<String, crate::plugins::Backoff>,
    pub(crate) jobs_env: crate::jobs::JobsEnv,
    /// Lets a long command show its state before it ends (the actor sets it).
    pub(crate) publish: Option<tokio::sync::watch::Sender<Snapshot>>,
    /// (repo, number) to the (head sha, updated_at) a proposal view was built on, and the view.
    pub(crate) proposal_cache: std::collections::HashMap<(String, u64), ProposalCached>,
    /// Unix seconds of the last request-count flush (Parity 23: once a minute and on exit).
    last_flush: i64,
    /// Set when the v1 import was skipped because `daemon.lock` was held by another process.
    daemon_note: Option<InstallNote>,
    /// The file watcher (Parity 24); alive while this is set.
    pub(crate) watch: Option<claude_sync_fs::watch::Watch>,
    /// The watcher saw a change; the next command or tick looks at the edits.
    pub(crate) dirty: Arc<AtomicBool>,
    /// Per group: the local edits seen last (the snapshot shows them, v1 `last_edits`).
    pub(crate) edits: std::collections::HashMap<String, Vec<EditView>>,
    /// Invitation ids already announced since start (v1 `known_invitations`).
    known_invitations: std::collections::HashSet<u64>,
}

/// v1 §7: hand edits are looked at once the files have been quiet this long.
const WATCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) type ProposalCached = ((String, String), ProposalView);

impl Drop for Core {
    fn drop(&mut self) {
        self.flush_requests();
    }
}

impl Core {
    /// What the v1 import left to say: the files it could not read and those it did not know.
    fn install_note(&self) -> Option<InstallNote> {
        let Some(r) = self.db.import_report().ok().flatten() else {
            return self.daemon_note.clone();
        };
        let total = u32::try_from(r.imported.len() + r.skipped.len() + r.unknown.len()).ok()?;
        let files: Vec<String> = r
            .skipped
            .into_iter()
            .map(|s| s.file)
            .chain(r.unknown)
            .collect();
        (!files.is_empty()).then_some(InstallNote::Skipped { files, total })
    }

    /// Opens the core; the first start also imports the v1 state, under `daemon.lock`.
    pub fn open(env: Env) -> Result<Core> {
        Self::open_with(env, false)
    }

    /// [`open`](Self::open) for a caller that already holds `daemon.lock` (the daemon).
    pub fn open_locked(env: Env) -> Result<Core> {
        Self::open_with(env, true)
    }

    /// Import the v1 state once. Without `held`, hand over from a v1 daemon first (v2-Cutover),
    /// then take `daemon.lock` and write `daemon.owner` for the import. A v1 daemon that does
    /// not stop in 30 s leaves a note, nothing imported and nothing written.
    fn import_v1_once(&mut self, held: bool) {
        use crate::handover::*;
        if self.db.import_report().ok().flatten().is_some() || self.db.has_import().unwrap_or(false)
        {
            return;
        }
        self.daemon_note = None;
        let (state, claude) = (self.home.state.clone(), self.home.claude.clone());
        let mut killed = false;
        let lock = if held {
            None
        } else {
            let exe = std::env::current_exe().ok();
            killed = match free_daemon_lock(
                &state,
                &*self.env.procs,
                (std::process::id(), exe.as_deref()),
                WAIT,
                POLL,
            ) {
                Handover::Free { killed } => killed,
                // the live v2 daemon imports
                Handover::V2Holds => return,
                Handover::Stuck { pid } => {
                    self.daemon_note = Some(InstallNote::Daemon {
                        pid,
                        version: "1".into(),
                    });
                    return;
                }
            };
            let f = std::fs::File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(state.join("daemon.lock"));
            // The lock was free a moment ago. A process another thread is spawning holds a copy
            // of every open file until it execs, and so holds a flock too: wait that out, or the
            // import is skipped and a later one overwrites the choices made in between.
            let got = f.ok().filter(|f| {
                (0..50).any(|_| {
                    f.try_lock().is_ok() || {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        false
                    }
                })
            });
            match got {
                Some(f) => {
                    let _ = write_owner(&state, &own_owner(env!("CARGO_PKG_VERSION")));
                    Some(f)
                }
                None => return,
            }
        };
        let r = claude_sync_store::import_v1(
            &self.db,
            &state,
            &claude,
            &claude_sync_store::DaemonLockHeld::claim(),
        );
        if let Err(e) = r {
            tracing::error!("v1 import failed: {e}");
        }
        if let Some(f) = lock {
            let _ = std::fs::remove_file(owner_path(&state));
            let _ = f.unlock();
            // a SIGTERM exit is clean, so the unit's Restart=on-failure would not start v2
            if killed && self.env.procs.unit_installed() {
                self.env.procs.restart_unit();
            }
        }
    }

    fn open_with(env: Env, held: bool) -> Result<Core> {
        let home = Home::from_env(&env.home);
        let home_path = env.home.clone();
        std::fs::create_dir_all(&home.state)?;
        let db = Db::open(&home.state.join("claude-sync.db"))?;
        let client = Client::new(env.cfg.clone())?;
        let mut c = Core {
            env,
            home,
            db,
            client,
            login: None,
            app_mode: false,
            app_mode_pending: false,
            app_uncovered: Vec::new(),
            signin: SignInState::SignedOut,
            cancel: Arc::default(),
            tx: None,
            expired: None,
            schedule: Schedule::default(),
            invitations: vec![],
            discoverable: vec![],
            pending_invites: Default::default(),
            current: Default::default(),
            syncing: false,
            plugins_running: Default::default(),
            plugin_backoff: Default::default(),
            jobs_env: crate::jobs::JobsEnv::system(&home_path),
            publish: None,
            proposal_cache: Default::default(),
            last_flush: 0,
            daemon_note: None,
            watch: None,
            dirty: Arc::default(),
            edits: Default::default(),
            known_invitations: Default::default(),
        };
        c.import_v1_once(held);
        c.restore_session();
        Ok(c)
    }

    /// Parity 23: add the GitHub request counts to the db (the `requests` table).
    pub fn flush_requests(&self) {
        let rows: Vec<_> = self
            .client
            .take_counts()
            .into_iter()
            .map(|c| claude_sync_store::RequestRow {
                caller: c.caller,
                endpoint: c.endpoint,
                status: c.result,
                n: c.count,
            })
            .collect();
        if !rows.is_empty() {
            let _ = self.db.add_requests(&rows);
        }
    }

    pub(crate) fn now(&self) -> i64 {
        (self.env.now)()
    }

    pub(crate) fn choices(&self) -> Choices {
        Choices::load(&self.db)
    }

    pub(crate) fn me(&self) -> Result<String> {
        self.login
            .clone()
            .ok_or_else(|| Error::new("not signed in"))
    }

    pub fn handle(&mut self, cmd: Command) -> Result<Event> {
        let r = self.dispatch(cmd);
        self.note_expired();
        self.check_dirty();
        r
    }

    /// Parity 24: watch `~/.claude` and the marketplaces. A change wakes the actor with a
    /// `Refresh` (the daemon looks at the flag on its next tick). A folder a sync creates later
    /// is watched after the next change (`check_dirty` calls this again).
    pub fn start_watch(&mut self) {
        let (dirty, tx) = (self.dirty.clone(), self.tx.clone());
        let on_change = move || {
            dirty.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(tx) = tx.as_ref().and_then(|t| t.upgrade()) {
                let _ = tx.try_send(Msg {
                    cmd: Command::Refresh,
                    reply: None,
                });
            }
        };
        match claude_sync_fs::watch::watch(&self.home, WATCH_DEBOUNCE, on_change) {
            Ok(w) => self.watch = Some(w),
            Err(e) => tracing::warn!("file watcher not started: {e}"),
        }
    }

    fn dispatch(&mut self, cmd: Command) -> Result<Event> {
        use Command::*;
        match cmd {
            SignInStart { app } => self.sign_in_start(app),
            SignInCancel => {
                self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(Event::Done)
            }
            AppCoverage => self.app_coverage(),
            SignInDone(o) => self.sign_in_done(o),
            SignOut => self.sign_out(),
            PluginsDone(d) => self.plugins_done(d),
            ReorderGroups(l) => self.reorder(l),
            CreateGroup { name, repo } => self.create_group(&name, &repo),
            Discover => self.discover(),
            Join { repo } => self.join(&repo),
            AcceptInvitation { id } => self.accept_invitation(id),
            AddMember { repo, login } => self.add_member(&repo, &login),
            RemoveMember { repo, login } => self.remove_member(&repo, &login),
            Transfer { repo, new_owner } => self.transfer(&repo, &new_owner),
            Leave { repo } => self.leave(&repo),
            SetGroupOff { repo, off } => self.set_off(&repo, off),
            SyncNow => self.sync_now(),
            PluginsRetry { repo } => self.plugins_retry(&repo),
            ReviewList { repo } => self.review_list(&repo),
            ReviewDecide { repo, ids, accept } => self.review_decide(&repo, ids, accept),
            DropLine { repo, line } => self.drop_line(&repo, &line),
            EditLine { source, line, text } => self.edit_line(source.as_deref(), &line, &text),
            UseGroupVersion { repo } => self.use_group_version(&repo),
            Clashes => self.clashes(),
            JobAccept { repo, id } => self.job_accept(&repo, &id),
            JobEnable { repo, id, on } => self.job_enable(&repo, &id, on),
            JobsAutoAccept { repo, on } => self.jobs_auto_accept(&repo, on),
            JobRunNow { repo, id } => self.job_run_now(&repo, &id),
            JobFile { repo, id, report } => self.job_file(&repo, &id, report),
            EditsList { repo } => self.edits_list(&repo),
            KeepMine { repo, path } => self.keep_mine(&repo, &path),
            UndoEdit { repo, path } => self.undo_edit(&repo, &path),
            SharePreview { repo, path } => self.share_preview(&repo, &path),
            ShareEdit { repo, path } => self.share_edit(&repo, &path),
            ShareSkill { repo, name } => self.share_skill(&repo, &name),
            ShareClaudeMd { repo } => self.share_claude_md(&repo),
            Conflicts => self.conflicts(),
            ConflictOpen {
                repo,
                path,
                saved_at,
            } => self.conflict_open(&repo, &path, saved_at),
            ConflictDelete {
                repo,
                path,
                saved_at,
            } => self.conflict_delete(&repo, &path, saved_at),
            PersonalOn => self.personal_on(),
            PersonalOff => self.personal_off(),
            PersonalDismiss => self.personal_dismiss(),
            PersonalJoinAuto { on } => self.personal_join_auto(on),
            PersonalKind { kind, on } => self.personal_kind(&kind, on),
            DeviceRename { id, name } => {
                self.device_edit(crate::personal::Pending::Rename { id, name })
            }
            DeviceRemove { id } => self.device_edit(crate::personal::Pending::Remove { id }),
            SetProposalMode { repo, on } => self.set_proposal_mode(&repo, on),
            SetAutoAccept { repo, on } => self.set_auto_accept(&repo, on),
            SetReviewChoice { repo, choice } => self.set_review_choice(&repo, choice),
            Proposals { repo } => self.proposals(&repo).map(Event::Proposals),
            ProposalApprove { repo, number } => self.proposal_approve(&repo, number),
            ProposalDecline { repo, number } => self.proposal_decline(&repo, number),
            ProposalsAcceptAll { repo } => self.proposals_accept_all(&repo),
            ProposalsDeclineAll { repo } => self.proposals_decline_all(&repo),
            CopyPreview { from, to } => self.copy_preview(&from, &to),
            Copy { from, to, req } => self.copy(&from, &to, req),
            Packs { repo } => self.packs(repo.as_deref()),
            PackSetup { repo, id } => self.pack_setup(&repo, &id),
            PackLaunch { repo, id } => self.pack_launch(&repo, &id),
            PackCheck { repo, id } => self.pack_check(&repo, &id),
            PackOn { repo, id } => self.pack_on(&repo, &id),
            PackOff { repo, id } => self.pack_off(&repo, &id),
            SshSet { on } => self.ssh_set(on),
            SshAlias { alias } => self.ssh_alias(&alias),
            ActivitySet { on } => self.activity_set(on),
            ActivityTick { idle_secs } => self.activity_tick(idle_secs),
            Explain { task, text } => self.explain(task, &text),
            CheckUpdate => self.check_update(),
            SetUpdate { state } => self.set_update(&state),
            MarkRead { id } => self.notes(|d| d.mark_read(id)),
            MarkAllRead => self.notes(|d| d.mark_all_read()),
            ClearNotifications => self.notes(|d| d.clear_notifications()),
            SendFeedback(f) => self.send_feedback(f),
            RetryFeedback => self.retry_feedback(),
            RecentLogs => self.recent_logs(),
            RevertPreview { repo, sha } => self.revert_preview(&repo, &sha),
            Revert { repo, sha } => self.revert(&repo, &sha),
            History { repo, limit } => self.history(&repo, limit),
            Contents { repo } => self.contents(&repo),
            Pause => self.set_app(|a| a.paused = true),
            Resume => self.set_app(|a| a.paused = false),
            DesktopNotifications { on } => self.set_app(|a| a.desktop_notifications = Some(on)),
            Helper { on } => self.set_app(|a| a.helper = Some(on)),
            TestNotification => {
                self.db
                    .add_notification("test", msg::TEST_NOTIFICATION, None, self.now())?;
                Ok(Event::Done)
            }
            StartAtLogin { on } => self.set_app(|a| a.start_at_login = Some(on)),
            Refresh => {
                if self.daemon_note.is_some() {
                    self.import_v1_once(false);
                }
                Ok(Event::Done)
            }
            SetInterval { secs } => self.set_app(|a| a.interval_secs = Some(secs)),
            WindowOpened => {
                self.schedule.invites_now();
                self.tick()
            }
        }
    }

    fn set_app(&mut self, f: impl FnOnce(&mut crate::choices::AppChoice)) -> Result<Event> {
        let mut c = self.choices();
        f(&mut c.app);
        c.app.ts = self.now() as u64 * 1000;
        c.save(&self.db)?;
        Ok(Event::Done)
    }

    /// One pass of the background loop: what is due now (invitations, renames).
    pub fn tick(&mut self) -> Result<Event> {
        let c = self.choices();
        let repos: Vec<String> = self.db.groups()?.into_iter().map(|g| g.repo).collect();
        let facts = Facts {
            now: self.now(),
            signed_in: self.login.is_some(),
            expired: self.expired.is_some(),
            paused: c.app.paused,
            interval: c.interval() as i64,
            limited_until: self.client.limited_until(),
            repos: &repos,
        };
        self.check_dirty();
        let due = self.schedule.due(&facts);
        if due.invites {
            self.load_invitations();
            self.announce_invitations();
            self.load_pending_invites();
        }
        for r in due.renames {
            self.follow_rename(&r);
        }
        if due.sync {
            let _ = self.sync_now();
        }
        self.refresh_session();
        self.note_expired();
        if self.now() - self.last_flush >= 60 {
            self.last_flush = self.now();
            self.flush_requests();
        }
        Ok(Event::Done)
    }

    /// Ask GitHub for the invitations now (a one-shot command's `groups` and `join`).
    pub fn load_invitations(&mut self) {
        if let Ok(list) =
            claude_sync_github::with_caller("invitations", || self.client.invitations())
        {
            self.invitations = list
                .into_iter()
                .filter_map(|i| {
                    Some(InvitationView {
                        id: i.id,
                        repo: i.repository?.full_name,
                        inviter: i.inviter.map(|o| o.login).unwrap_or_default(),
                        created_at: i
                            .created_at
                            .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                            .map(|t| t.timestamp()),
                    })
                })
                .collect();
        }
    }

    /// v1 `check_invitations`: one notification for each invitation not seen since start.
    fn announce_invitations(&mut self) {
        let now = self.now();
        for i in &self.invitations {
            if self.known_invitations.insert(i.id) {
                let t = msg::invitation_note(&i.inviter, &i.repo);
                let _ = self.db.add_notification("group", &t, Some(&i.repo), now);
            }
        }
        let ids: Vec<u64> = self.invitations.iter().map(|i| i.id).collect();
        self.known_invitations.retain(|k| ids.contains(k));
    }

    /// Host groups: who GitHub still lists as invited.
    fn load_pending_invites(&mut self) {
        let hosted: Vec<String> = self
            .db
            .groups()
            .unwrap_or_default()
            .into_iter()
            .filter(|g| g.host)
            .map(|g| g.repo)
            .collect();
        for repo in hosted {
            if self.refresh_pending(&repo).is_err() {
                break;
            }
        }
    }

    /// Re-read one host group's pending invitations.
    pub(crate) fn refresh_pending(&mut self, repo: &str) -> Result<()> {
        let (o, n) = crate::groups::split(repo)?;
        let list =
            claude_sync_github::with_caller("invitations", || self.client.repo_invitations(o, n))?;
        let logins = list
            .into_iter()
            .filter_map(|i| i.invitee.map(|x| x.login))
            .collect();
        self.pending_invites.insert(repo.to_string(), logins);
        Ok(())
    }

    /// Parity 16: one repo GET a day; a new `full_name` moves the group's rows.
    fn follow_rename(&mut self, repo: &str) {
        let Ok((o, n)) = crate::groups::split(repo) else {
            return;
        };
        let Ok(r) = claude_sync_github::with_caller("rename", || self.client.repo(o, n)) else {
            return;
        };
        if !r.full_name.eq_ignore_ascii_case(repo)
            && self.db.rename_group(repo, &r.full_name).is_ok()
        {
            {
                let t = format!("{repo} is now {}", r.full_name);
                let _ = self
                    .db
                    .add_notification("rename", &t, Some(&r.full_name), self.now());
            }
        }
    }

    /// App mode: refresh tokens that are due.
    fn refresh_session(&mut self) {
        if let (Some(l), Ok(Some((_, t)))) = (
            self.login.clone(),
            claude_sync_github::secrets::load(&self.env.secrets),
        ) {
            if t.refresh_due(self.now()) {
                if let Some(rt) = &t.refresh_token {
                    if let Ok(n) = claude_sync_github::device::refresh(&self.client, rt) {
                        let _ = claude_sync_github::secrets::save(&self.env.secrets, &l, &n);
                        self.client.set_tokens(n);
                    }
                }
            }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let c = self.choices();
        let notes = self.db.notifications(50).unwrap_or_default();
        let groups = self
            .db
            .groups()
            .unwrap_or_default()
            .into_iter()
            .map(|g| {
                let gf = crate::groups::cached(&self.db, &g.repo);
                let host = self.login.clone().unwrap_or_default();
                let syncing = self.syncing
                    && self
                        .current
                        .lock()
                        .is_ok_and(|c| c.as_deref() == Some(g.repo.as_str()));
                GroupView {
                    name: gf
                        .as_ref()
                        .map(|f| f.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| g.repo.clone()),
                    host: g.repo.split('/').next().unwrap_or_default().to_string(),
                    is_host: g.host,
                    status: if g.off || c.groups.get(&g.repo).is_some_and(|x| x.off) {
                        GroupStatus::Off
                    } else if syncing {
                        GroupStatus::Syncing
                    } else if g.state.starts_with(msg::OFFLINE_PREFIX) {
                        GroupStatus::Offline
                    } else if g.state.starts_with(msg::RATE_LIMITED_PREFIX) {
                        GroupStatus::RateLimited
                    } else if !matches!(g.state.as_str(), "ok" | "active" | "") {
                        GroupStatus::Failed
                    } else {
                        GroupStatus::Synced
                    },
                    error: (!matches!(g.state.as_str(), "ok" | "active" | "failed" | ""))
                        .then(|| g.state.clone()),
                    last_sync: self.db.last_synced(&g.repo).ok().flatten(),
                    members: gf
                        .as_ref()
                        .map(|f| f.members.clone())
                        .unwrap_or_else(|| vec![host]),
                    proposal_mode: gf.as_ref().is_some_and(|f| f.proposal_mode),
                    auto_accept_claude_md: gf.as_ref().is_none_or(|f| f.auto_accept_claude_md),
                    jobs_auto_accept: c.groups.get(&g.repo).is_some_and(|x| x.auto_run_jobs),
                    host_lock: self.host_lock(&g.repo, g.host),
                    review_choice: c.review_choice(&g.repo),
                    plugin_problems: self.plugin_problems(&g.repo),
                    syncing,
                    failed_at: Store::read(&self.db, &format!("failed-at/{}", g.repo))
                        .ok()
                        .flatten()
                        .and_then(|b| String::from_utf8(b).ok()?.parse().ok()),
                    pending_review: crate::review::load(&self.db, &g.repo)
                        .map_or(0, |r| r.pending.len() as u32),
                    off: g.off,
                    edits: self.edits.get(&g.repo).cloned().unwrap_or_default(),
                    pending_invites: self
                        .pending_invites
                        .get(&g.repo)
                        .cloned()
                        .unwrap_or_default(),
                    repo: g.repo,
                }
            })
            .collect();
        let personal = self.personal_view();
        Snapshot {
            login: self.login.clone(),
            sign_in: self.signin.clone(),
            session_expired: self.expired.clone(),
            app_mode: self.app_mode,
            paused: c.app.paused,
            interval_secs: c.interval(),
            syncing: self.syncing,
            rate_limited_until: self.client.limited_until(),
            groups,
            invitations: self.invitations.clone(),
            discoverable: self.discoverable.clone(),
            personal_on: personal.on,
            personal,
            unread: self.db.unread_notifications().unwrap_or(0),
            notifications: notes
                .into_iter()
                .map(|n| NotificationView {
                    id: n.id,
                    kind: n.kind,
                    text: n.text,
                    read: n.read,
                    time: n.time,
                    repo: n.repo,
                })
                .collect(),
            jobs: self.job_views(),
            ssh_on: c
                .rest
                .get("sshPeers")
                .and_then(|p| p.get("on"))
                .and_then(|o| o.as_bool())
                .unwrap_or(false),
            activity_on: c
                .rest
                .get("activity")
                .and_then(|p| p.get("share"))
                .and_then(|o| o.as_bool())
                .unwrap_or(false),
            update: self.update_state(),
            update_checked: self.update_checked(),
            app_uncovered: self.app_uncovered.clone(),
            desktop_notifications: c.app.desktop_notifications.unwrap_or(true),
            helper: c.app.helper.unwrap_or(true),
            start_at_login: c.app.start_at_login,
            install: self.install_note(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoSecrets;
    impl SecretStore for NoSecrets {
        fn get(&self, _: &str) -> claude_sync_github::Result<Option<String>> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &str) -> claude_sync_github::Result<()> {
            Ok(())
        }
        fn delete(&self, _: &str) -> claude_sync_github::Result<()> {
            Ok(())
        }
    }

    fn env_with(dir: &std::path::Path, procs: Arc<dyn crate::handover::Procs>) -> Env {
        let mut e = Env {
            home: dir.into(),
            cfg: Config {
                api: "http://127.0.0.1:9".into(),
                login: "http://127.0.0.1:9".into(),
                relay: "http://127.0.0.1:9".into(),
                releases: String::new(),
                client_id: String::new(),
                retry_delay: std::time::Duration::from_millis(1),
            },
            app_client_id: String::new(),
            secrets: Secrets(Arc::new(NoSecrets)),
            now: Arc::new(|| 1),
            device: "laptop".into(),
            procs: Arc::new(crate::handover::NoProcs),
        };
        e.procs = procs;
        e
    }

    /// A v1 daemon holds `daemon.lock`; stopping it drops the lock.
    fn v1_holding_the_lock(
        state: &std::path::Path,
        unit: bool,
    ) -> Arc<crate::handover::fake::FakeProcs> {
        std::fs::create_dir_all(state).unwrap();
        std::fs::write(state.join("app.json"), "{}").unwrap();
        let lock = std::fs::File::create(state.join("daemon.lock")).unwrap();
        lock.lock().unwrap();
        let lock = std::sync::Mutex::new(Some(lock));
        let fake = crate::handover::fake::FakeProcs {
            procs: vec![crate::handover::fake::v1(4242, "/old/claude-sync-cli")],
            unit,
            ..Default::default()
        };
        *fake.on_kill.lock().unwrap() = Some(Box::new(move || drop(lock.lock().unwrap().take())));
        Arc::new(fake)
    }

    #[test]
    fn a_v1_daemon_is_stopped_then_its_state_is_imported_and_the_unit_restarted_if_there_is_one() {
        for unit in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let state = dir.path().join(".claude-sync");
            let fake = v1_holding_the_lock(&state, unit);
            let c = Core::open(env_with(dir.path(), fake.clone())).unwrap();
            assert_eq!(*fake.killed.lock().unwrap(), [4242]);
            assert_eq!(
                c.db.import_report().unwrap().unwrap().imported,
                ["app.json"]
            );
            assert!(c.snapshot().install.is_none());
            // the owner file lives only while the import runs
            assert!(!state.join("daemon.owner").exists());
            assert_eq!(*fake.restarts.lock().unwrap(), u32::from(unit));
        }
    }

    #[test]
    fn a_machine_with_no_v1_daemon_never_restarts_the_unit() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(crate::handover::fake::FakeProcs {
            unit: true,
            ..Default::default()
        });
        Core::open(env_with(dir.path(), fake.clone())).unwrap();
        assert_eq!(*fake.restarts.lock().unwrap(), 0);
    }

    #[test]
    fn a_failed_pass_is_offline_rate_limited_or_failed_by_its_text() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env {
            home: dir.path().into(),
            cfg: Config {
                api: "http://127.0.0.1:9".into(),
                login: "http://127.0.0.1:9".into(),
                relay: "http://127.0.0.1:9".into(),
                releases: String::new(),
                client_id: String::new(),
                retry_delay: std::time::Duration::from_millis(1),
            },
            app_client_id: String::new(),
            secrets: Secrets(Arc::new(NoSecrets)),
            now: Arc::new(|| 1),
            device: "laptop".into(),
            procs: Arc::new(crate::handover::NoProcs),
        };
        let c = Core::open(env).unwrap();
        c.db.add_group("zed/team", false).unwrap();
        let status = |state: &str| {
            c.db.set_group_state("zed/team", state).unwrap();
            let g = c.snapshot().groups.remove(0);
            (g.status, g.error)
        };
        let offline = "network error: connection refused";
        assert_eq!(
            status(offline),
            (GroupStatus::Offline, Some(offline.into()))
        );
        let limit = "GitHub limit reached, resumes 3:00 PM";
        assert_eq!(
            status(limit),
            (GroupStatus::RateLimited, Some(limit.into()))
        );
        assert_eq!(status("GitHub returned 500: x").0, GroupStatus::Failed);
        assert_eq!(status("ok"), (GroupStatus::Synced, None));
    }
}
