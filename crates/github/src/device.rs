//! OAuth device flow and token refresh (v2-SequenceSignIn, v2-ScenarioSignInEdge). Public
//! client id, no secret. Calls POST /login/device/code and POST /login/oauth/access_token.
use crate::client::{error_message, now, PlainBody};
use crate::{Client, Error, Result, Tokens};
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

pub const SCOPES: &str = "repo read:user";
const GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    /// GitHub asked to slow down; the interval it named (0: none).
    SlowDown(u64),
    Token(Tokens),
    Expired,
    Denied,
}

/// POST a form to the OAuth host; the answer must be a JSON object.
fn oauth(c: &Client, path: &str, form: Vec<(&'static str, String)>) -> Result<Value> {
    let url = format!("{}{path}", c.config().login.trim_end_matches('/'));
    let (status, body) = c.plain(Method::POST, &url, None, PlainBody::Form(form))?;
    match serde_json::from_slice::<Value>(&body) {
        Ok(v) if v.is_object() => Ok(v),
        _ => Err(Error::Github {
            status,
            message: String::from_utf8_lossy(&body).chars().take(200).collect(),
        }),
    }
}

fn oauth_error(code: &str, reply: &Value) -> Error {
    let detail = reply
        .get("error_description")
        .and_then(Value::as_str)
        .unwrap_or(code);
    Error::Github {
        status: 400,
        message: format!("{code}: {detail}"),
    }
}

pub fn start(c: &Client) -> Result<DeviceCode> {
    let cid = c.config().client_id.clone();
    let reply = oauth(
        c,
        "/login/device/code",
        vec![("client_id", cid), ("scope", SCOPES.into())],
    )?;
    if let Some(e) = reply.get("error").and_then(Value::as_str) {
        return Err(oauth_error(e, &reply));
    }
    serde_json::from_value(reply).map_err(|e| Error::Invalid(format!("bad reply: {e}")))
}

pub fn poll(c: &Client, device_code: &str) -> Result<DevicePoll> {
    let cid = c.config().client_id.clone();
    let reply = oauth(
        c,
        "/login/oauth/access_token",
        vec![
            ("client_id", cid),
            ("device_code", device_code.into()),
            ("grant_type", GRANT.into()),
        ],
    )?;
    if let Some(t) = tokens_from(&reply) {
        return Ok(DevicePoll::Token(t));
    }
    match reply.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => Ok(DevicePoll::Pending),
        Some("slow_down") => Ok(DevicePoll::SlowDown(
            reply.get("interval").and_then(Value::as_u64).unwrap_or(0),
        )),
        Some("expired_token") => Ok(DevicePoll::Expired),
        Some("access_denied") => Ok(DevicePoll::Denied),
        Some(other) => Err(oauth_error(other, &reply)),
        None => Err(Error::Invalid("GitHub sent no token and no error".into())),
    }
}

/// Poll until approved, denied or expired. Sleeps `interval` before each poll; `slow_down`
/// takes the server's interval or adds 5 s; a network error waits and tries again; any other
/// error ends the wait. `sleep` is injected so tests do not wait.
pub fn wait(c: &Client, code: &DeviceCode, mut sleep: impl FnMut(Duration)) -> Result<DevicePoll> {
    let (mut interval, mut elapsed) = (code.interval.max(1), 0u64);
    loop {
        sleep(Duration::from_secs(interval));
        elapsed += interval;
        if elapsed > code.expires_in {
            return Ok(DevicePoll::Expired);
        }
        match poll(c, &code.device_code) {
            Ok(DevicePoll::Pending) | Err(Error::Network(_)) => {}
            Ok(DevicePoll::SlowDown(n)) => interval = if n > 0 { n } else { interval + 5 },
            Ok(done) => return Ok(done),
            Err(e) => return Err(e),
        }
    }
}

/// GitHub App mode: the new token set for a refresh token.
pub fn refresh(c: &Client, refresh_token: &str) -> Result<Tokens> {
    let cid = c.config().client_id.clone();
    let reply = oauth(
        c,
        "/login/oauth/access_token",
        vec![
            ("client_id", cid),
            ("grant_type", "refresh_token".into()),
            ("refresh_token", refresh_token.into()),
        ],
    )?;
    tokens_from(&reply).ok_or_else(|| match reply.get("error").and_then(Value::as_str) {
        Some(e) => oauth_error(e, &reply),
        None => Error::Github {
            status: 400,
            message: error_message(reply.to_string().as_bytes()),
        },
    })
}

fn tokens_from(reply: &Value) -> Option<Tokens> {
    let access = reply.get("access_token")?.as_str()?;
    let secs = |k: &str| reply.get(k).and_then(Value::as_i64).map(|s| now() + s);
    Some(Tokens {
        access_token: access.into(),
        refresh_token: reply
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from),
        expires_at: secs("expires_in"),
        refresh_token_expires_at: secs("refresh_token_expires_in"),
    })
}
