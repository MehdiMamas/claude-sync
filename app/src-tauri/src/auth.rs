//! GitHub sign-in with the device flow (§3).
use crate::config;
use crate::ctx::{Auth, Ctx};
use crate::guard::{self, Error};
use crate::{background, notify, window};
use claude_sync_core::github::{self, DevicePoll, GitHub};
use serde::Serialize;
use std::sync::Arc;
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

pub async fn start(app: &AppHandle) -> Result<DeviceCodeView, Error> {
    if config::GITHUB_CLIENT_ID.is_empty() {
        return Err("This copy of claude-sync was built without a GitHub client ID (CLAUDE_SYNC_GITHUB_CLIENT_ID), so it cannot sign in.".into());
    }
    let ctx = app.state::<Arc<Ctx>>().inner().clone();
    let gen = ctx.next_sign_in_gen();
    let code = guard::call_async(
        "start sign-in",
        github::start_device_flow(config::GITHUB_CLIENT_ID),
    )
    .await?;
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
        match guard::call_async(
            "sign-in",
            github::poll_device_flow(config::GITHUB_CLIENT_ID, device_code),
        )
        .await?
        {
            DevicePoll::Pending => {}
            DevicePoll::SlowDown(s) => every = Duration::from_secs(s.max(every.as_secs() + 5)),
            DevicePoll::Expired => return Err("The code expired. Start again.".into()),
            DevicePoll::Denied => return Err("Sign-in was cancelled on GitHub.".into()),
            DevicePoll::Token(token) => {
                let user =
                    guard::call_async("GitHub user", GitHub::new(token.clone()).user()).await?;
                guard::call("save token", || {
                    github::token_store::save(&user.login, &token)
                })?;
                ctx.set_auth(Some(Auth {
                    login: user.login.clone(),
                    token,
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

pub fn cancel(ctx: &Ctx) {
    ctx.next_sign_in_gen();
}

pub fn sign_out(app: &AppHandle) -> Result<(), Error> {
    let ctx = app.state::<Arc<Ctx>>();
    cancel(&ctx);
    let deleted = guard::call("delete token", github::token_store::delete);
    ctx.set_auth(None);
    let _ = ctx.update_state(|s| s.login = None);
    background::refresh_status(app);
    deleted
}
