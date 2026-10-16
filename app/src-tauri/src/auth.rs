//! GitHub sign-in with the device flow (§3), token refresh and an expired sign-in.
use crate::config;
use crate::ctx::{Auth, Ctx};
use crate::guard::{self, Error};
use crate::{background, notify, window};
use claude_sync_core::github::{self, DevicePoll, GitHub, Session, Tokens};
use serde::Serialize;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeView {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedIn {
    pub login: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInFailed {
    pub message: String,
}

/// The app, for the session hooks. Set first thing in `setup`, before any GitHub call.
static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// A session for `tokens`: it refreshes with this build's client ID, saves refreshed
/// tokens to the keychain (when `keychain`), and signs the app out as "expired" when
/// GitHub stops accepting it.
pub fn new_session(tokens: Tokens, keychain: bool) -> Arc<Session> {
    let s = Session::new(tokens)
        .refreshable(config::GITHUB_CLIENT_ID, github::LOGIN)
        .on_expired(move |s| expired(s, keychain));
    Arc::new(if keychain {
        s.on_refresh(save_refreshed)
    } else {
        s
    })
}

fn save_refreshed(session: &Session, tokens: &Tokens) {
    let Some(app) = APP.get() else {
        tracing::warn!("token refreshed before the app started; not saved");
        return;
    };
    // A sign-in that replaced this one has its own tokens in the keychain.
    let Some(login) = app.state::<Arc<Ctx>>().current_login(session) else {
        return;
    };
    if let Err(e) = github::token_store::save(&login, tokens) {
        tracing::warn!("refreshed token not saved to the keychain: {e}");
    }
}

/// GitHub no longer accepts the sign-in and it cannot be refreshed. Stop the GitHub
/// calls and ask the user to sign in again; the groups and `state.json` stay as they are.
fn expired(session: &Session, keychain: bool) {
    let Some(app) = APP.get() else { return };
    let ctx = app.state::<Arc<Ctx>>();
    let Some(login) = ctx.expire(session) else {
        return;
    };
    // The dead tokens go; the login stays in state.json, which marks the expiry at the
    // next start too.
    if keychain {
        if let Err(e) = github::token_store::delete() {
            tracing::warn!("expired token not removed from the keychain: {e}");
        }
    }
    tracing::warn!("GitHub sign-in of {login} expired; background GitHub calls stopped");
    let _ = app.emit("sign-in-expired", SignedIn { login });
    notify::show(
        app,
        notify::Kind::Other,
        None,
        "GitHub sign-in expired",
        "Open claude-sync and sign in again to keep syncing. Your groups on this computer are kept.",
    );
    background::refresh_status(app);
}

pub async fn start(app: &AppHandle) -> Result<DeviceCodeView, Error> {
    if config::GITHUB_CLIENT_ID.is_empty() {
        return Err("This copy of claude-sync was built without a GitHub client ID (CLAUDE_SYNC_GITHUB_CLIENT_ID), so it cannot sign in.".into());
    }
    let ctx = app.state::<Arc<Ctx>>().inner().clone();
    let gen = ctx.next_sign_in_gen();
    let code = github::start_device_flow(config::GITHUB_CLIENT_ID)
        .await
        .map_err(guard::logged("start sign-in"))?;
    if let Err(e) = app.opener().open_url(&code.verification_uri, None::<&str>) {
        tracing::warn!("could not open the browser: {e}");
    }
    let view = DeviceCodeView {
        user_code: code.user_code.clone(),
        verification_uri: code.verification_uri.clone(),
        expires_in: code.expires_in,
    };
    let a = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = poll(&ctx, gen, &code.device_code, code.interval, code.expires_in).await;
        if !ctx.sign_in_current(gen) {
            return;
        }
        match result {
            Ok(login) => {
                let _ = a.emit(
                    "signed-in",
                    SignedIn {
                        login: login.clone(),
                    },
                );
                notify::show(
                    &a,
                    notify::Kind::Other,
                    None,
                    "Signed in",
                    &format!("claude-sync is signed in as {login}."),
                );
                background::refresh_status(&a);
                background::sync_soon(&a, true);
                let b = a.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = background::check_invitations(&b).await;
                });
                window::open(&a, Some("groups"));
            }
            Err(message) => {
                let _ = a.emit("sign-in-failed", SignInFailed { message });
            }
        }
    });
    Ok(view)
}

async fn poll(
    ctx: &Ctx,
    gen: u64,
    device_code: &str,
    interval: u64,
    expires_in: u64,
) -> Result<String, Error> {
    let deadline = Instant::now() + Duration::from_secs(expires_in.max(60));
    let mut every = Duration::from_secs(interval.max(5));
    loop {
        tokio::time::sleep(every).await;
        if !ctx.sign_in_current(gen) {
            return Err("cancelled".into());
        }
        if Instant::now() > deadline {
            return Err("The code expired. Start again.".into());
        }
        match github::poll_device_flow(config::GITHUB_CLIENT_ID, device_code)
            .await
            .map_err(guard::logged("sign-in"))?
        {
            DevicePoll::Pending => {}
            DevicePoll::SlowDown(s) => every = Duration::from_secs(s.max(every.as_secs() + 5)),
            DevicePoll::Expired => return Err("The code expired. Start again.".into()),
            DevicePoll::Denied => return Err("Sign-in was cancelled on GitHub.".into()),
            DevicePoll::Token(tokens) => {
                let user = GitHub::new(tokens.access_token.clone())
                    .user()
                    .await
                    .map_err(guard::logged("GitHub user"))?;
                github::token_store::save(&user.login, &tokens)
                    .map_err(guard::logged("save token"))?;
                ctx.set_auth(Some(Auth {
                    login: user.login.clone(),
                    session: new_session(tokens, true),
                }));
                let login = user.login.clone();
                if let Err(e) = ctx.update_state(|s| s.login = Some(login)) {
                    tracing::warn!("signed in, but state not saved: {e}");
                }
                tracing::info!("signed in as {}", user.login);
                return Ok(user.login);
            }
        }
    }
}

/// Debug builds only: `CLAUDE_SYNC_DEV_TOKEN` signs in with that token so the app can be
/// tried without a GitHub OAuth App. The token stays in memory: it is never written to
/// the keychain, and signing out leaves the keychain alone.
#[cfg(debug_assertions)]
pub fn dev_token() -> Option<String> {
    std::env::var("CLAUDE_SYNC_DEV_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

#[cfg(debug_assertions)]
pub fn sign_in_with_dev_token(app: &AppHandle, token: String) {
    let a = app.clone();
    tauri::async_runtime::spawn(async move {
        let ctx = a.state::<Arc<Ctx>>().inner().clone();
        let user = match GitHub::new(token.clone()).user().await {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!("CLAUDE_SYNC_DEV_TOKEN was not accepted: {e}");
                return;
            }
        };
        tracing::info!("signed in as {} with CLAUDE_SYNC_DEV_TOKEN", user.login);
        ctx.set_auth(Some(Auth {
            login: user.login.clone(),
            session: new_session(Tokens::plain(token), false),
        }));
        let login = user.login.clone();
        let _ = ctx.update_state(|s| s.login = Some(login));
        let _ = a.emit("signed-in", SignedIn { login: user.login });
        background::refresh_status(&a);
        background::sync_soon(&a, true);
    });
}

pub fn cancel(ctx: &Ctx) {
    ctx.next_sign_in_gen();
}

pub fn sign_out(app: &AppHandle) -> Result<(), Error> {
    let ctx = app.state::<Arc<Ctx>>();
    cancel(&ctx);
    #[cfg(debug_assertions)]
    let deleted = if dev_token().is_some() {
        Ok(())
    } else {
        github::token_store::delete().map_err(guard::logged("delete token"))
    };
    #[cfg(not(debug_assertions))]
    let deleted = github::token_store::delete().map_err(guard::logged("delete token"));
    ctx.set_auth(None);
    let _ = ctx.update_state(|s| s.login = None);
    background::refresh_status(app);
    deleted
}
