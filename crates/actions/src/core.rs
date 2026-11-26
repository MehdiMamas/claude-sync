//! The state the actor owns, and the dispatch from [`Command`] to the code that does it.
use crate::actor::Msg;
use crate::choices::Choices;
use crate::error::{Error, Result};
use crate::model::*;
use crate::schedule::{Facts, Schedule};
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
    pub(crate) discoverable: Vec<String>,
    pub(crate) syncing: bool,
    /// Lets a long command show its state before it ends (the actor sets it).
    pub(crate) publish: Option<tokio::sync::watch::Sender<Snapshot>>,
}

impl Core {
    pub fn open(env: Env) -> Result<Core> {
        let home = Home::under(&env.home);
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
            syncing: false,
            publish: None,
        };
        c.restore_session();
        Ok(c)
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
            ReviewList { repo } => self.review_list(&repo),
            ReviewDecide { repo, ids, accept } => self.review_decide(&repo, ids, accept),
            DropLine { repo, line } => self.drop_line(&repo, &line),
            UseGroupVersion { repo } => self.use_group_version(&repo),
            Clashes => self.clashes(),
            EditsList { repo } => self.edits_list(&repo),
            KeepMine { repo, path } => self.keep_mine(&repo, &path),
            UndoEdit { repo, path } => self.undo_edit(&repo, &path),
            ShareEdit { repo, path } => self.share_edit(&repo, &path),
            ShareSkill { repo, name } => self.share_skill(&repo, &name),
            ShareClaudeMd { repo } => self.share_claude_md(&repo),
            Conflicts => self.conflicts(),
            Pause => self.set_app(|a| a.paused = true),
            Resume => self.set_app(|a| a.paused = false),
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
                        })
                    })
                    .collect();
            }
        }
        for r in due.renames {
            self.follow_rename(&r);
        }
        if due.sync {
            let _ = self.sync_now();
        }
        self.refresh_session();
        self.note_expired();
        Ok(Event::Done)
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
                    } else if self.syncing {
                        GroupStatus::Syncing
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
                    host_lock: None,
                    review_choice: c.review_choice(&g.repo),
                    pending_review: crate::review::load(&self.db, &g.repo)
                        .map_or(0, |r| r.pending.len() as u32),
                    off: g.off,
                    repo: g.repo,
                }
            })
            .collect();
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
            personal_on: !c
                .rest
                .get("personal")
                .and_then(|p| p.get("off"))
                .and_then(|o| o.as_bool())
                .unwrap_or(false),
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
            jobs: vec![],
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
            update: String::new(),
        }
    }
}
