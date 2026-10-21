//! Shared app state: paths, saved state, sign-in, the cached engine and the status line.
use crate::config;
use crate::guard::Error;
use claude_sync_core::github::{self, GitHub, Session};
use claude_sync_core::state::AppState;
use claude_sync_core::watch::LocalEdit;
use claude_sync_core::{Engine, Paths};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct Auth {
    pub login: String,
    /// The tokens; every GitHub client of this sign-in shares them (refresh, expiry).
    pub session: Arc<Session>,
}

/// What the tray's status line and the UI header show.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// `signedOut`, `expired`, `paused`, `syncing`, `ok`, `error` or `idle`.
    pub state: &'static str,
    pub text: String,
    pub last_sync_unix: Option<u64>,
    pub error: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: "idle",
            text: "Starting".into(),
            last_sync_unix: None,
            error: None,
        }
    }
}

/// App-only preferences that are not part of the core's `state.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// Autostart is turned on once, on the first run; after that it is the user's choice.
    pub autostart_initialized: bool,
    /// Show desktop notifications (Settings). The history is kept either way.
    pub desktop_notifications: bool,
}

impl Prefs {
    fn path(paths: &Paths) -> PathBuf {
        paths.state_dir.join("app.json")
    }

    /// The saved preferences, or the defaults. Also read by the headless `run-job`.
    pub fn load(paths: &Paths) -> Self {
        std::fs::read(Self::path(paths))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            autostart_initialized: false,
            desktop_notifications: true,
        }
    }
}

pub struct Ctx {
    pub paths: Paths,
    state: Mutex<AppState>,
    auth: Mutex<Option<Auth>>,
    /// The login whose sign-in GitHub stopped accepting. Set while signed out that way.
    expired: Mutex<Option<String>>,
    engine: Mutex<Option<Arc<Engine>>>,
    status: Mutex<Status>,
    /// Serialises sync passes (loop, tray, UI).
    pub sync_lock: tokio::sync::Mutex<()>,
    /// Wakes the sync loop early (resume, sign-in, settings change).
    pub wake: tokio::sync::Notify,
    /// Bumped on every sign-in start or sign-out; a device-flow poller stops when it is stale.
    pub sign_in_gen: AtomicU64,
    pub last_edits: Mutex<Vec<LocalEdit>>,
    pub known_invitations: Mutex<Vec<u64>>,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Ctx {
    /// Saved state and the keychain sign-in. Runs after logging starts so problems are logged.
    pub fn load(paths: Paths) -> Self {
        let state = AppState::load(&paths).unwrap_or_else(|e| {
            tracing::warn!("could not read the saved state, starting fresh: {e}");
            AppState::default()
        });
        #[cfg(debug_assertions)]
        let use_keychain = crate::auth::dev_token().is_none();
        #[cfg(not(debug_assertions))]
        let use_keychain = true;
        let keychain = if use_keychain {
            claude_sync_core::github::token_store::load()
        } else {
            Ok(None)
        };
        let (auth, expired) = match keychain {
            Ok(Some((login, tokens))) => (
                Some(Auth {
                    login,
                    session: crate::auth::new_session(
                        tokens,
                        true,
                        config::client_id(state.github_app),
                    ),
                }),
                None,
            ),
            // Signed in before (sign-out clears the login) but the keychain has no tokens:
            // the sign-in expired.
            Ok(None) if use_keychain => (None, state.login.clone()),
            Ok(None) => (None, None),
            Err(e) => {
                tracing::warn!("could not read the sign-in from the keychain: {e}");
                (None, None)
            }
        };
        Self {
            paths,
            state: Mutex::new(state),
            auth: Mutex::new(auth),
            expired: Mutex::new(expired),
            engine: Mutex::new(None),
            status: Mutex::new(Status::default()),
            sync_lock: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
            sign_in_gen: AtomicU64::new(0),
            last_edits: Mutex::new(Vec::new()),
            known_invitations: Mutex::new(Vec::new()),
        }
    }

    /// Change the state and save it. The change stays in memory even if saving fails.
    pub fn update_state(&self, f: impl FnOnce(&mut AppState)) -> Result<(), Error> {
        let snapshot = {
            let mut s = lock(&self.state);
            f(&mut s);
            s.clone()
        };
        snapshot.save(&self.paths).map_err(|e| {
            tracing::warn!("could not save the state: {e}");
            e.to_string()
        })
    }

    pub fn auth(&self) -> Option<Auth> {
        lock(&self.auth).clone()
    }

    pub fn login(&self) -> Option<String> {
        lock(&self.auth).as_ref().map(|a| a.login.clone())
    }

    /// Sign in or out. Either way the sign-in is no longer "expired".
    pub fn set_auth(&self, auth: Option<Auth>) {
        *lock(&self.auth) = auth;
        *lock(&self.expired) = None;
        *lock(&self.engine) = None;
    }

    /// The login of `session` when it is the current sign-in (not one replaced since).
    pub fn current_login(&self, session: &Session) -> Option<String> {
        lock(&self.auth)
            .as_ref()
            .filter(|a| std::ptr::eq(Arc::as_ptr(&a.session), session))
            .map(|a| a.login.clone())
    }

    /// `session` expired: sign out without forgetting the login or the groups. Returns the
    /// login, or `None` when `session` is no longer the current sign-in.
    pub fn expire(&self, session: &Session) -> Option<String> {
        let mut auth = lock(&self.auth);
        let login = auth
            .as_ref()
            .filter(|a| std::ptr::eq(Arc::as_ptr(&a.session), session))
            .map(|a| a.login.clone())?;
        *auth = None;
        *lock(&self.expired) = Some(login.clone());
        *lock(&self.engine) = None;
        Some(login)
    }

    /// The login whose sign-in expired, while signed out that way.
    pub fn expired_login(&self) -> Option<String> {
        lock(&self.expired).clone()
    }

    /// The groups joined on this device.
    pub fn joined_groups(&self) -> Vec<claude_sync_core::GroupId> {
        lock(&self.state).groups.clone()
    }

    /// Whether sign-in uses the GitHub App (setting `githubApp`).
    pub fn github_app(&self) -> bool {
        lock(&self.state).github_app
    }

    /// The client ID for this device's sign-in method.
    pub fn client_id(&self) -> &'static str {
        config::client_id(self.github_app())
    }

    pub fn poll_seconds(&self) -> u64 {
        lock(&self.state)
            .poll_seconds
            .unwrap_or(config::DEFAULT_POLL_SECONDS)
            .max(config::MIN_POLL_SECONDS)
    }

    pub fn paused(&self) -> bool {
        lock(&self.state).paused
    }

    pub fn device_name(&self) -> String {
        lock(&self.state)
            .device_name
            .clone()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(default_device_name)
    }

    /// Drop the cached engine (sign-in, sign-out, device rename).
    pub fn reset_engine(&self) {
        *lock(&self.engine) = None;
    }

    pub fn engine(&self) -> Result<Arc<Engine>, Error> {
        if let Some(e) = lock(&self.engine).as_ref() {
            return Ok(e.clone());
        }
        let auth = self.auth().ok_or_else(|| "Not signed in".to_string())?;
        let engine = Arc::new(Engine::new(
            self.paths.clone(),
            GitHub::with_session(auth.session, github::API),
            auth.login,
            self.device_name(),
        ));
        *lock(&self.engine) = Some(engine.clone());
        Ok(engine)
    }

    pub fn next_sign_in_gen(&self) -> u64 {
        self.sign_in_gen.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn sign_in_current(&self, gen: u64) -> bool {
        self.sign_in_gen.load(Ordering::SeqCst) == gen
    }

    pub fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    /// Returns true when the status changed.
    pub fn set_status(&self, s: Status) -> bool {
        let mut cur = lock(&self.status);
        if *cur == s {
            return false;
        }
        *cur = s;
        true
    }

    fn prefs_path(&self) -> PathBuf {
        Prefs::path(&self.paths)
    }

    pub fn prefs(&self) -> Prefs {
        Prefs::load(&self.paths)
    }

    pub fn save_prefs(&self, p: &Prefs) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.paths.state_dir)?;
        let tmp = self.prefs_path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(p)?)?;
        std::fs::rename(tmp, self.prefs_path())
    }

    pub fn outbox_dir(&self) -> PathBuf {
        self.paths.state_dir.join("outbox")
    }
}

pub fn default_device_name() -> String {
    gethostname::gethostname()
        .to_string_lossy()
        .trim()
        .to_lowercase()
}
