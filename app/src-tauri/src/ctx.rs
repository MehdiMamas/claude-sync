//! Shared app state: paths, saved state, sign-in, the cached engine and the status line.
use crate::config;
use crate::guard::Error;
use claude_sync_core::github::GitHub;
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
    pub token: String,
}

/// What the tray's status line and the UI header show.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// `signedOut`, `paused`, `syncing`, `ok`, `error` or `idle`.
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// Autostart is turned on once, on the first run; after that it is the user's choice.
    pub autostart_initialized: bool,
}

pub struct Ctx {
    pub paths: Paths,
    state: Mutex<AppState>,
    auth: Mutex<Option<Auth>>,
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
        let auth = match keychain {
            Ok(a) => a.map(|(login, token)| Auth { login, token }),
            Err(e) => {
                tracing::warn!("could not read the sign-in from the keychain: {e}");
                None
            }
        };
        Self {
            paths,
            state: Mutex::new(state),
            auth: Mutex::new(auth),
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

    pub fn set_auth(&self, auth: Option<Auth>) {
        *lock(&self.auth) = auth;
        *lock(&self.engine) = None;
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
            GitHub::new(auth.token),
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
        self.paths.state_dir.join("app.json")
    }

    pub fn prefs(&self) -> Prefs {
        std::fs::read(self.prefs_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
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
