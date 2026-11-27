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
    pub(crate) plugin_backoff: std::collections::HashMap<String, crate::plugins::Backoff>,
    pub(crate) jobs_env: crate::jobs::JobsEnv,
    /// Lets a long command show its state before it ends (the actor sets it).
    pub(crate) publish: Option<tokio::sync::watch::Sender<Snapshot>>,
    /// (repo, number) to the (head sha, updated_at) a proposal view was built on, and the view.
    pub(crate) proposal_cache: std::collections::HashMap<(String, u64), ProposalCached>,
    /// Unix seconds of the last request-count flush (Parity 23: once a minute and on exit).
    last_flush: i64,
}

pub(crate) type ProposalCached = ((String, String), ProposalView);

impl Drop for Core {
    fn drop(&mut self) {
        self.flush_requests();
    }
}

impl Core {
    pub fn open(env: Env) -> Result<Core> {
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
            plugin_backoff: Default::default(),
            jobs_env: crate::jobs::JobsEnv::system(&home_path),
            publish: None,
            proposal_cache: Default::default(),
            last_flush: 0,
        };
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
        r
    }

    fn dispatch(&mut self, cmd: Command) -> Result<Event> {
        use Command::*;
        match cmd {
            SignInStart { app } => self.sign_in_start(app),
            SignInCancel => {
                self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(Event::Done)
            }
            SignInDone(o) => self.sign_in_done(o),
            SignOut => self.sign_out(),
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
            PersonalOn => self.personal_on(),
            PersonalOff => self.personal_off(),
            PersonalDismiss => self.personal_dismiss(),
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
            PackCheck { repo, id } => self.pack_check(&repo, &id),
            PackOn { repo, id } => self.pack_on(&repo, &id),
            PackOff { repo, id } => self.pack_off(&repo, &id),
            SshSet { on } => self.ssh_set(on),
            SshAlias { alias } => self.ssh_alias(&alias),
            ActivitySet { on } => self.activity_set(on),
            ActivityTick { idle_secs } => self.activity_tick(idle_secs),
            Explain { task, text } => self.explain(task, &text),
            CheckUpdate => self.check_update(),
            MarkRead { id } => self.notes(|d| d.mark_read(id)),
            MarkAllRead => self.notes(|d| d.mark_all_read()),
            ClearNotifications => self.notes(|d| d.clear_notifications()),
            SendFeedback(f) => self.send_feedback(f),
            RetryFeedback => self.retry_feedback(),
            RevertPreview { repo, sha } => self.revert_preview(&repo, &sha),
            Revert { repo, sha } => self.revert(&repo, &sha),
            History { repo, limit } => self.history(&repo, limit),
            Contents { repo } => self.contents(&repo),
            Pause => self.set_app(|a| a.paused = true),
            Resume => self.set_app(|a| a.paused = false),
            DesktopNotifications { on } => self.set_app(|a| a.desktop_notifications = Some(on)),
            StartAtLogin { on } => self.set_app(|a| a.start_at_login = Some(on)),
            Refresh => Ok(Event::Done),
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
        let due = self.schedule.due(&facts);
        if due.invites {
            self.load_invitations();
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
            app_uncovered: Vec::new(),
            desktop_notifications: c.app.desktop_notifications.unwrap_or(true),
            start_at_login: c.app.start_at_login,
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
