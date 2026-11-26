//! Sign-in (Parity 1-3): device flow with a cancel flag, GitHub App mode with refresh, sign out.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{Event, SignInState};
use crate::msg;
use claude_sync_github::{device, secrets, Client, Config, Tokens};
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum Outcome {
    Tokens(Tokens),
    Expired,
    Denied,
    Cancelled,
    Offline,
    Failed(String),
}

impl Core {
    pub(crate) fn sign_in_start(&mut self, app: bool) -> Result<Event> {
        let id = if app {
            &self.env.app_client_id
        } else {
            &self.env.cfg.client_id
        };
        if id.is_empty() {
            let m = if app { msg::NO_APP } else { msg::NO_CLIENT_ID };
            self.signin = SignInState::Unavailable;
            return Err(Error::new(m));
        }
        let mut cfg: Config = self.env.cfg.clone();
        cfg.client_id = id.clone();
        let c = Client::new(cfg)?;
        self.signin = SignInState::Starting;
        let code = device::start(&c).inspect_err(|e| {
            self.signin = match e {
                claude_sync_github::Error::Network(_) => SignInState::Offline,
                e => SignInState::Failed {
                    message: e.to_string(),
                },
            }
        })?;
        self.cancel.store(false, Ordering::SeqCst);
        self.app_mode_pending = app;
        let cancel = self.cancel.clone();
        let tx = self.tx.clone();
        let c2 = c.clone();
        let code2 = code.clone();
        std::thread::spawn(move || {
            let out = match device::wait_with(
                &c2,
                &code2,
                |d| {
                    // wake every 200 ms so Cancel is quick
                    let end = std::time::Instant::now() + d;
                    while std::time::Instant::now() < end && !cancel.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                },
                || cancel.load(Ordering::SeqCst),
            ) {
                Ok(device::DevicePoll::Token(t)) => Outcome::Tokens(t),
                Ok(device::DevicePoll::Expired) => Outcome::Expired,
                Ok(device::DevicePoll::Denied) => Outcome::Denied,
                Ok(device::DevicePoll::Cancelled) => Outcome::Cancelled,
                Ok(_) => Outcome::Failed("unexpected answer from GitHub".into()),
                Err(claude_sync_github::Error::Network(_)) => Outcome::Offline,
                Err(e) => Outcome::Failed(e.to_string()),
            };
            if let Some(tx) = tx.and_then(|t| t.upgrade()) {
                let _ = tx.blocking_send(crate::actor::Msg {
                    cmd: crate::model::Command::SignInDone(out),
                    reply: None,
                });
            }
        });
        self.signin = SignInState::Code {
            user_code: code.user_code.clone(),
            verification_uri: code.verification_uri.clone(),
            expires_in: code.expires_in,
        };
        Ok(Event::DeviceCode {
            user_code: code.user_code,
            verification_uri: code.verification_uri,
            expires_in: code.expires_in,
        })
    }

    pub(crate) fn sign_in_done(&mut self, out: Outcome) -> Result<Event> {
        match out {
            Outcome::Tokens(t) => {
                self.client.set_tokens(t.clone());
                let user = self.client.user()?;
                secrets::save(&self.env.secrets, &user.login, &t)?;
                self.login = Some(user.login.clone());
                self.app_mode = t.refresh_token.is_some();
                self.expired = None;
                self.signin = SignInState::SignedOut;
                self.schedule.invites_now();
                Ok(Event::SignedIn { login: user.login })
            }
            Outcome::Expired => self.sign_in_end(SignInState::Expired),
            Outcome::Denied => self.sign_in_end(SignInState::Denied),
            Outcome::Offline => self.sign_in_end(SignInState::Offline),
            Outcome::Cancelled => self.sign_in_end(SignInState::SignedOut),
            Outcome::Failed(m) => self.sign_in_end(SignInState::Failed { message: m }),
        }
    }

    fn sign_in_end(&mut self, s: SignInState) -> Result<Event> {
        self.signin = s;
        Ok(Event::Done)
    }

    pub(crate) fn sign_out(&mut self) -> Result<Event> {
        self.client.sign_out();
        secrets::delete(&self.env.secrets)?;
        self.login = None;
        self.expired = None;
        self.app_mode = false;
        Ok(Event::SignedOut)
    }

    /// Restore the saved session at start (and refresh App-mode tokens that are due).
    pub(crate) fn restore_session(&mut self) {
        let Ok(Some((login, t))) = secrets::load(&self.env.secrets) else {
            return;
        };
        let mut t = t;
        if t.refresh_due((self.env.now)()) {
            if let Some(rt) = t.refresh_token.clone() {
                if let Ok(n) = device::refresh(&self.client, &rt) {
                    let _ = secrets::save(&self.env.secrets, &login, &n);
                    t = n;
                }
            }
        }
        self.app_mode = t.refresh_token.is_some();
        self.client.set_tokens(t);
        self.login = Some(login);
    }

    /// A 401 anywhere stops the loop with a visible notice (Parity 3).
    pub(crate) fn note_expired(&mut self) {
        if self.client.is_expired() && self.expired.is_none() {
            let who = self.login.clone().unwrap_or_default();
            let text = msg::session_expired_for(&who);
            if !self.db.has_unread("signin", &text).unwrap_or(true) {
                let _ = self
                    .db
                    .add_notification("signin", &text, None, (self.env.now)());
            }
            self.expired = Some(who);
        }
    }
}
