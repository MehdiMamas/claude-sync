//! GitHub REST client, OAuth device flow, keychain token (§3, issue #13).
use crate::{Error, GroupId};
use reqwest::{header, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const API: &str = "https://api.github.com";
/// Where the OAuth device-flow endpoints live.
pub const LOGIN: &str = "https://github.com";
pub const SCOPES: &str = "repo read:user";
pub const USER_AGENT: &str = "claude-sync";
const API_VERSION: &str = "2022-11-28";

/// A client builder with TLS ready. reqwest is built without a default crypto provider
/// (as tauri-plugin-updater uses it), so install ring once before building any client.
pub fn client_builder() -> reqwest::ClientBuilder {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Err means another thread installed one first, which is just as good.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    reqwest::Client::builder()
}

/// The HTTP client every request goes through.
pub fn http_client() -> reqwest::Client {
    client_builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .build()
        .expect("TLS backend initialises")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Result of one poll of the device flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    /// GitHub asked to slow down; new interval in seconds.
    SlowDown(u64),
    Token(String),
    Expired,
    Denied,
}

pub async fn start_device_flow(client_id: &str) -> crate::Result<DeviceCode> {
    start_device_flow_at(&http_client(), LOGIN, client_id).await
}

pub async fn poll_device_flow(client_id: &str, device_code: &str) -> crate::Result<DevicePoll> {
    poll_device_flow_at(&http_client(), LOGIN, client_id, device_code).await
}

/// Poll until the user approves, denies or the code expires. Honours `interval`, `slow_down`
/// and `expires_in`; network errors wait one interval and try again. Returns `Token`,
/// `Expired` or `Denied`.
pub async fn wait_for_token(client_id: &str, code: &DeviceCode) -> crate::Result<DevicePoll> {
    let http = http_client();
    drive_device_flow(
        code,
        async || poll_device_flow_at(&http, LOGIN, client_id, &code.device_code).await,
        async |d| tokio::time::sleep(d).await,
    )
    .await
}

/// `start_device_flow` against another host (tests, GitHub Enterprise).
pub async fn start_device_flow_at(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
) -> crate::Result<DeviceCode> {
    let reply = oauth_post(
        http,
        &format!("{base}/login/device/code"),
        &[("client_id", client_id), ("scope", SCOPES)],
    )
    .await?;
    if let Some(err) = reply.get("error").and_then(Value::as_str) {
        return Err(oauth_error(err, &reply));
    }
    Ok(serde_json::from_value(reply)?)
}

pub async fn poll_device_flow_at(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
    device_code: &str,
) -> crate::Result<DevicePoll> {
    let reply = oauth_post(
        http,
        &format!("{base}/login/oauth/access_token"),
        &[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ],
    )
    .await?;
    if let Some(token) = reply.get("access_token").and_then(Value::as_str) {
        return Ok(DevicePoll::Token(token.into()));
    }
    match reply.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => Ok(DevicePoll::Pending),
        Some("slow_down") => {
            let interval = reply.get("interval").and_then(Value::as_u64);
            Ok(DevicePoll::SlowDown(interval.unwrap_or(0)))
        }
        Some("expired_token") => Ok(DevicePoll::Expired),
        Some("access_denied") => Ok(DevicePoll::Denied),
        Some(other) => Err(oauth_error(other, &reply)),
        None => Err(Error::Invalid("GitHub sent no token and no error".into())),
    }
}

/// The polling state machine of `wait_for_token`, with the poll and the sleep injected.
pub async fn drive_device_flow(
    code: &DeviceCode,
    mut poll: impl AsyncFnMut() -> crate::Result<DevicePoll>,
    mut sleep: impl AsyncFnMut(Duration),
) -> crate::Result<DevicePoll> {
    let mut interval = code.interval.max(1);
    let mut waited = 0;
    loop {
        if waited >= code.expires_in {
            return Ok(DevicePoll::Expired);
        }
        sleep(Duration::from_secs(interval)).await;
        waited += interval;
        match poll().await {
            Ok(DevicePoll::Pending) | Err(Error::Network(_)) => {}
            // RFC 8628: on slow_down add 5 s unless the server says how long.
            Ok(DevicePoll::SlowDown(next)) => interval = next.max(interval + 5),
            Ok(done) => return Ok(done),
            Err(e) => return Err(e),
        }
    }
}

async fn oauth_post(
    http: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> crate::Result<Value> {
    let resp = http
        .post(url)
        .header(header::ACCEPT, "application/json")
        .header(header::USER_AGENT, USER_AGENT)
        .form(form)
        .send()
        .await
        .map_err(|e| Error::Network(e.without_url().to_string()))?;
    let status = resp.status().as_u16();
    let body = resp
        .bytes()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    match serde_json::from_slice::<Value>(&body) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(Error::GitHub {
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
    Error::GitHub {
        status: 400,
        message: format!("{code}: {detail}"),
    }
}

/// OS keychain storage (service `claude-sync`). Never files, never logs.
///
/// Two entries: account `@current` holds the signed-in login (`@` cannot appear in a GitHub
/// login), and account `<login>` holds that login's token.
pub mod token_store {
    pub const SERVICE: &str = "claude-sync";
    const CURRENT: &str = "@current";

    /// Where secrets go. `Keychain` in the app, an in-memory map in tests.
    pub trait Store {
        fn get(&self, account: &str) -> crate::Result<Option<String>>;
        fn set(&self, account: &str, secret: &str) -> crate::Result<()>;
        fn delete(&self, account: &str) -> crate::Result<()>;
    }

    pub struct Keychain;

    fn keychain_error(e: keyring::Error) -> crate::Error {
        crate::Error::Invalid(format!("keychain: {e}"))
    }

    impl Store for Keychain {
        fn get(&self, account: &str) -> crate::Result<Option<String>> {
            let entry = keyring::Entry::new(SERVICE, account).map_err(keychain_error)?;
            match entry.get_password() {
                Ok(s) => Ok(Some(s)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(keychain_error(e)),
            }
        }
        fn set(&self, account: &str, secret: &str) -> crate::Result<()> {
            let entry = keyring::Entry::new(SERVICE, account).map_err(keychain_error)?;
            entry.set_password(secret).map_err(keychain_error)
        }
        fn delete(&self, account: &str) -> crate::Result<()> {
            let entry = keyring::Entry::new(SERVICE, account).map_err(keychain_error)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(keychain_error(e)),
            }
        }
    }

    /// `(login, token)` of the signed-in account.
    pub fn load() -> crate::Result<Option<(String, String)>> {
        load_from(&Keychain)
    }
    pub fn save(login: &str, token: &str) -> crate::Result<()> {
        save_to(&Keychain, login, token)
    }
    pub fn delete() -> crate::Result<()> {
        delete_from(&Keychain)
    }

    pub fn load_from(store: &impl Store) -> crate::Result<Option<(String, String)>> {
        let Some(login) = store.get(CURRENT)? else {
            return Ok(None);
        };
        Ok(store.get(&login)?.map(|token| (login, token)))
    }
    pub fn save_to(store: &impl Store, login: &str, token: &str) -> crate::Result<()> {
        if let Some(old) = store.get(CURRENT)? {
            if old != login {
                store.delete(&old)?;
            }
        }
        store.set(login, token)?;
        store.set(CURRENT, login)
    }
    pub fn delete_from(store: &impl Store) -> crate::Result<()> {
        if let Some(login) = store.get(CURRENT)? {
            store.delete(&login)?;
        }
        store.delete(CURRENT)
    }
}

/// Authenticated client. Cheap to clone; clones share the rate-limit state.
#[derive(Clone)]
pub struct GitHub {
    pub token: String,
    pub api: String,
    pub http: reqwest::Client,
    /// Unix time until which GitHub said we are rate limited.
    limited_until: Arc<Mutex<Option<u64>>>,
    retry_delay: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub id: u64,
    pub repo: crate::GroupId,
    pub inviter: String,
}

/// A successful (2xx or 304) response.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub etag: Option<String>,
    /// `rel="next"` of the `Link` header.
    pub next: Option<String>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json<T: DeserializeOwned>(&self) -> crate::Result<T> {
        Ok(serde_json::from_slice(&self.body)?)
    }
}

/// Result of a conditional GET.
#[derive(Debug, Clone, PartialEq)]
pub enum Conditional<T> {
    NotModified,
    Modified { value: T, etag: Option<String> },
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct RawRepo {
    name: String,
    owner: Login,
    #[serde(default)]
    private: bool,
    topics: Option<Vec<String>>,
}

impl RawRepo {
    fn id(&self) -> GroupId {
        GroupId::new(&self.owner.login, &self.name)
    }
}

#[derive(Deserialize)]
struct RawInvitation {
    id: u64,
    repository: RawRepo,
    inviter: Option<Login>,
}

const ATTEMPTS: u32 = 3;

impl GitHub {
    pub fn new(token: String) -> Self {
        Self::with_api(token, API)
    }

    /// A client for another API root (tests, GitHub Enterprise).
    pub fn with_api(token: String, api: impl Into<String>) -> Self {
        Self {
            token,
            api: api.into().trim_end_matches('/').to_string(),
            http: http_client(),
            limited_until: Arc::default(),
            retry_delay: Duration::from_millis(500),
        }
    }

    /// First wait between retries of a failed request (doubles each time).
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    fn url(&self, path: &str) -> crate::Result<String> {
        if path.starts_with("http://") || path.starts_with("https://") {
            // Only follow absolute links (pagination) back to our own API: they carry the token.
            return if path.starts_with(&format!("{}/", self.api)) {
                Ok(path.into())
            } else {
                Err(Error::Invalid(format!(
                    "refusing to send the token to {path}"
                )))
            };
        }
        Ok(format!("{}{}", self.api, path))
    }

    /// One API call. `Ok` for 2xx and 304; GitHub errors, rate limits and network failures
    /// map to `Error`. Network failures and 502-504 are retried with backoff for GETs
    /// (and for connection failures of any method, where nothing was sent).
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        etag: Option<&str>,
    ) -> crate::Result<Response> {
        self.check_limit()?;
        let url = self.url(path)?;
        let mut attempt = 0;
        loop {
            let mut req = self
                .http
                .request(method.clone(), &url)
                .header(header::ACCEPT, "application/vnd.github+json")
                .header(header::USER_AGENT, USER_AGENT)
                .header("X-GitHub-Api-Version", API_VERSION);
            if !self.token.is_empty() {
                req = req.bearer_auth(&self.token);
            }
            if let Some(tag) = etag {
                req = req.header(header::IF_NONE_MATCH, tag);
            }
            if let Some(b) = body {
                req = req.json(b);
            }
            attempt += 1;
            let retry_left = attempt < ATTEMPTS;
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e)
                    if retry_left
                        && (e.is_connect()
                            || (method == Method::GET && (e.is_timeout() || e.is_request()))) =>
                {
                    self.backoff(attempt).await;
                    continue;
                }
                Err(e) => return Err(Error::Network(e.without_url().to_string())),
            };
            let status = resp.status();
            if method == Method::GET && retry_left && matches!(status.as_u16(), 502..=504) {
                self.backoff(attempt).await;
                continue;
            }
            return self.finish(resp).await;
        }
    }

    async fn backoff(&self, attempt: u32) {
        tokio::time::sleep(self.retry_delay * 2u32.pow(attempt - 1)).await;
    }

    fn check_limit(&self) -> crate::Result<()> {
        let mut limit = self.limited_until.lock().unwrap();
        match *limit {
            Some(reset) if crate::state::now_unix() < reset => {
                Err(Error::RateLimited { reset_unix: reset })
            }
            Some(_) => {
                *limit = None;
                Ok(())
            }
            None => Ok(()),
        }
    }

    async fn finish(&self, resp: reqwest::Response) -> crate::Result<Response> {
        let status = resp.status();
        let headers = resp.headers().clone();
        let text = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let body = resp
            .bytes()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?
            .to_vec();
        if status.is_success() || status == StatusCode::NOT_MODIFIED {
            return Ok(Response {
                status: status.as_u16(),
                etag: text("etag"),
                next: text("link").as_deref().and_then(next_link),
                body,
            });
        }
        if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
            let now = crate::state::now_unix();
            let exhausted = text("x-ratelimit-remaining").as_deref() == Some("0");
            let reset = if let Some(secs) = text("retry-after").and_then(|s| s.parse::<u64>().ok())
            {
                Some(now + secs)
            } else if exhausted {
                Some(
                    text("x-ratelimit-reset")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(now + 60),
                )
            } else if status == StatusCode::TOO_MANY_REQUESTS {
                Some(now + 60)
            } else {
                None
            };
            if let Some(reset_unix) = reset {
                *self.limited_until.lock().unwrap() = Some(reset_unix);
                return Err(Error::RateLimited { reset_unix });
            }
        }
        let message = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| String::from_utf8_lossy(&body).chars().take(200).collect());
        Err(Error::GitHub {
            status: status.as_u16(),
            message,
        })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> crate::Result<T> {
        self.request(Method::GET, path, None, None).await?.json()
    }

    /// GET with `If-None-Match`. A `304` costs nothing against the rate limit.
    pub async fn get_if_changed<T: DeserializeOwned>(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> crate::Result<Conditional<T>> {
        let resp = self.request(Method::GET, path, None, etag).await?;
        if resp.status == 304 {
            return Ok(Conditional::NotModified);
        }
        Ok(Conditional::Modified {
            value: resp.json()?,
            etag: resp.etag,
        })
    }

    /// Every page of a list endpoint (follows `Link: rel="next"`).
    pub async fn get_pages<T: DeserializeOwned>(&self, path: &str) -> crate::Result<Vec<T>> {
        let mut out = Vec::new();
        let mut next = Some(path.to_string());
        while let Some(p) = next {
            let resp = self.request(Method::GET, &p, None, None).await?;
            out.extend(resp.json::<Vec<T>>()?);
            next = resp.next;
        }
        Ok(out)
    }

    /// A call with a JSON body and a JSON reply.
    pub async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &Value,
    ) -> crate::Result<T> {
        self.request(method, path, Some(body), None).await?.json()
    }

    /// A call whose reply body does not matter (`204`s).
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> crate::Result<()> {
        self.request(method, path, body, None).await.map(drop)
    }

    pub async fn user(&self) -> crate::Result<User> {
        self.get("/user").await
    }

    /// Pending invitations to group repos (topic `claude-sync-group`, §5).
    pub async fn invitations(&self) -> crate::Result<Vec<Invitation>> {
        let raw: Vec<RawInvitation> = self
            .get_pages("/user/repository_invitations?per_page=100")
            .await?;
        let mut out = Vec::new();
        for inv in raw {
            if self.is_group_repo(&inv.repository).await? {
                out.push(Invitation {
                    id: inv.id,
                    repo: inv.repository.id(),
                    inviter: inv.inviter.map(|l| l.login).unwrap_or_default(),
                });
            }
        }
        Ok(out)
    }

    /// Topics are not always in the invitation payload, and an invitee usually cannot read a
    /// private repo's topics before accepting. Then the default repo name decides.
    async fn is_group_repo(&self, repo: &RawRepo) -> crate::Result<bool> {
        if let Some(topics) = &repo.topics {
            return Ok(topics.iter().any(|t| t == crate::group::TOPIC));
        }
        let id = repo.id();
        match self.repo_topics(&id).await {
            Ok(topics) => Ok(topics.iter().any(|t| t == crate::group::TOPIC)),
            Err(Error::GitHub {
                status: 403 | 404, ..
            }) => Ok(repo.name == crate::group::DEFAULT_REPO),
            Err(e) => Err(e),
        }
    }

    pub async fn repo_topics(&self, id: &GroupId) -> crate::Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Topics {
            names: Vec<String>,
        }
        let t: Topics = self
            .get(&format!("/repos/{}/{}/topics", id.owner, id.repo))
            .await?;
        Ok(t.names)
    }

    /// Accept an invitation; returns the repo it was for.
    pub async fn accept_invitation(&self, id: u64) -> crate::Result<GroupId> {
        let raw: Vec<RawInvitation> = self
            .get_pages("/user/repository_invitations?per_page=100")
            .await?;
        let inv = raw
            .into_iter()
            .find(|i| i.id == id)
            .ok_or_else(|| Error::Invalid(format!("no pending invitation {id}")))?;
        self.call(
            Method::PATCH,
            &format!("/user/repository_invitations/{id}"),
            None,
        )
        .await?;
        Ok(inv.repository.id())
    }

    /// Private group repos this user owns or collaborates on.
    pub async fn group_repos(&self) -> crate::Result<Vec<GroupId>> {
        let repos: Vec<RawRepo> = self
            .get_pages("/user/repos?affiliation=owner,collaborator&per_page=100")
            .await?;
        Ok(repos
            .into_iter()
            .filter(|r| {
                r.private
                    && r.topics
                        .as_ref()
                        .is_some_and(|t| t.iter().any(|t| t == crate::group::TOPIC))
            })
            .map(|r| r.id())
            .collect())
    }

    /// Create an empty private repo on the signed-in account and tag it as a group.
    pub async fn create_group_repo(&self, name: &str) -> crate::Result<GroupId> {
        let id = self
            .create_repo(name, "claude-sync group (shared Claude Code setup)")
            .await?;
        let topics = json!({ "names": [crate::group::TOPIC] });
        self.call(
            Method::PUT,
            &format!("/repos/{}/{}/topics", id.owner, id.repo),
            Some(&topics),
        )
        .await?;
        Ok(id)
    }

    /// Create an empty private repo on the signed-in account with no topic: the personal
    /// memory repo, which must never look like a group.
    pub async fn create_private_repo(&self, name: &str) -> crate::Result<GroupId> {
        self.create_repo(name, "claude-sync personal memory (private to you)")
            .await
    }

    async fn create_repo(&self, name: &str, description: &str) -> crate::Result<GroupId> {
        let body = json!({
            "name": name,
            "private": true,
            "auto_init": false,
            "description": description,
        });
        let repo: RawRepo = self.send(Method::POST, "/user/repos", &body).await?;
        let id = repo.id();
        if !repo.private {
            return Err(Error::PublicRepo(id.to_string()));
        }
        Ok(id)
    }
}

/// `<https://…?page=2>; rel="next", <…>; rel="last"` -> the `next` URL.
fn next_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        params
            .split(';')
            .any(|p| p.trim() == "rel=\"next\"")
            .then(|| {
                url.trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string()
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use wiremock::matchers::{body_string_contains, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn gh(server: &MockServer) -> GitHub {
        GitHub::with_api("tok".into(), server.uri()).with_retry_delay(Duration::from_millis(1))
    }

    fn code(interval: u64, expires_in: u64) -> DeviceCode {
        DeviceCode {
            device_code: "dc".into(),
            user_code: "ABCD-1234".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in,
            interval,
        }
    }

    #[tokio::test]
    async fn device_flow_start_and_poll_states() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/device/code"))
            .and(header("accept", "application/json"))
            .and(body_string_contains("client_id=cid"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code": "dc", "user_code": "ABCD-1234",
                "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 5
            })))
            .mount(&server)
            .await;
        let http = http_client();
        let c = start_device_flow_at(&http, &server.uri(), "cid")
            .await
            .unwrap();
        assert_eq!((c.user_code.as_str(), c.interval), ("ABCD-1234", 5));

        let replies = [
            (
                json!({"error": "authorization_pending"}),
                DevicePoll::Pending,
            ),
            (
                json!({"error": "slow_down", "interval": 10}),
                DevicePoll::SlowDown(10),
            ),
            (json!({"error": "expired_token"}), DevicePoll::Expired),
            (json!({"error": "access_denied"}), DevicePoll::Denied),
            (
                json!({"access_token": "gho_x", "token_type": "bearer"}),
                DevicePoll::Token("gho_x".into()),
            ),
        ];
        for (reply, want) in replies {
            server.reset().await;
            Mock::given(method("POST"))
                .and(path("/login/oauth/access_token"))
                .and(body_string_contains("device_code=dc"))
                .respond_with(ResponseTemplate::new(200).set_body_json(reply))
                .mount(&server)
                .await;
            assert_eq!(
                poll_device_flow_at(&http, &server.uri(), "cid", "dc")
                    .await
                    .unwrap(),
                want
            );
        }

        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"error": "incorrect_client_credentials"})),
            )
            .mount(&server)
            .await;
        let err = poll_device_flow_at(&http, &server.uri(), "cid", "dc")
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::GitHub { status: 400, ref message } if message.contains("incorrect_client"))
        );
    }

    async fn drive(
        c: &DeviceCode,
        polls: Vec<crate::Result<DevicePoll>>,
    ) -> (crate::Result<DevicePoll>, Vec<u64>) {
        let polls = RefCell::new(VecDeque::from(polls));
        let slept = RefCell::new(Vec::new());
        let out = drive_device_flow(
            c,
            async || {
                polls
                    .borrow_mut()
                    .pop_front()
                    .unwrap_or(Ok(DevicePoll::Pending))
            },
            async |d: Duration| slept.borrow_mut().push(d.as_secs()),
        )
        .await;
        (out, slept.into_inner())
    }

    #[tokio::test]
    async fn device_flow_state_machine() {
        let (out, slept) = drive(
            &code(5, 900),
            vec![
                Ok(DevicePoll::Pending),
                Err(Error::Network("reset".into())),
                Ok(DevicePoll::SlowDown(0)),
                Ok(DevicePoll::SlowDown(20)),
                Ok(DevicePoll::Token("t".into())),
            ],
        )
        .await;
        assert_eq!(out.unwrap(), DevicePoll::Token("t".into()));
        assert_eq!(
            slept,
            [5, 5, 5, 10, 20],
            "slow_down adds 5 s or takes the server's interval"
        );

        let (out, slept) = drive(&code(5, 12), vec![]).await;
        assert_eq!(out.unwrap(), DevicePoll::Expired);
        assert_eq!(slept, [5, 5, 5]);

        let (out, _) = drive(&code(5, 900), vec![Ok(DevicePoll::Denied)]).await;
        assert_eq!(out.unwrap(), DevicePoll::Denied);
        let (out, _) = drive(&code(5, 900), vec![Err(Error::Invalid("x".into()))]).await;
        assert!(out.is_err());
    }

    #[derive(Default)]
    struct MemStore(RefCell<HashMap<String, String>>);

    impl token_store::Store for MemStore {
        fn get(&self, account: &str) -> crate::Result<Option<String>> {
            Ok(self.0.borrow().get(account).cloned())
        }
        fn set(&self, account: &str, secret: &str) -> crate::Result<()> {
            self.0.borrow_mut().insert(account.into(), secret.into());
            Ok(())
        }
        fn delete(&self, account: &str) -> crate::Result<()> {
            self.0.borrow_mut().remove(account);
            Ok(())
        }
    }

    #[test]
    fn token_store_keeps_one_login() {
        use token_store::*;
        let store = MemStore::default();
        assert_eq!(load_from(&store).unwrap(), None);
        save_to(&store, "alice", "t1").unwrap();
        assert_eq!(
            load_from(&store).unwrap(),
            Some(("alice".into(), "t1".into()))
        );
        save_to(&store, "bob", "t2").unwrap();
        assert_eq!(
            load_from(&store).unwrap(),
            Some(("bob".into(), "t2".into()))
        );
        assert_eq!(store.0.borrow().len(), 2, "the old login's token is gone");
        delete_from(&store).unwrap();
        assert_eq!(load_from(&store).unwrap(), None);
        assert!(store.0.borrow().is_empty());
    }

    #[tokio::test]
    async fn sends_github_headers_and_handles_etags() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .and(header("authorization", "Bearer tok"))
            .and(header("accept", "application/vnd.github+json"))
            .and(header("user-agent", "claude-sync"))
            .and(header("x-github-api-version", "2022-11-28"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"login": "me"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/thing"))
            .and(header("if-none-match", "\"v1\""))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/thing"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "\"v1\"")
                    .set_body_json(json!(1)),
            )
            .mount(&server)
            .await;
        let gh = gh(&server);
        assert_eq!(gh.user().await.unwrap().login, "me");
        assert_eq!(
            gh.get_if_changed::<u32>("/thing", None).await.unwrap(),
            Conditional::Modified {
                value: 1,
                etag: Some("\"v1\"".into())
            }
        );
        assert_eq!(
            gh.get_if_changed::<u32>("/thing", Some("\"v1\""))
                .await
                .unwrap(),
            Conditional::NotModified
        );
    }

    #[tokio::test]
    async fn rate_limit_is_reported_and_short_circuits() {
        let server = MockServer::start().await;
        let reset = crate::state::now_unix() + 3600;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-remaining", "0")
                    .insert_header("x-ratelimit-reset", reset.to_string().as_str())
                    .set_body_json(json!({"message": "API rate limit exceeded"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let gh = gh(&server);
        assert!(
            matches!(gh.user().await, Err(Error::RateLimited { reset_unix }) if reset_unix == reset)
        );
        let clone = gh.clone();
        assert!(
            matches!(clone.user().await, Err(Error::RateLimited { .. })),
            "no second request"
        );
    }

    #[tokio::test]
    async fn plain_403_is_an_error_not_a_rate_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"message": "Resource not accessible"})),
            )
            .mount(&server)
            .await;
        let err = gh(&server).user().await.unwrap_err();
        assert!(
            matches!(err, Error::GitHub { status: 403, ref message } if message == "Resource not accessible")
        );
    }

    #[tokio::test]
    async fn retries_gets_on_gateway_errors_only() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"login": "me"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        let gh = gh(&server);
        assert_eq!(gh.user().await.unwrap().login, "me");
        assert!(matches!(
            gh.call(Method::POST, "/x", None).await,
            Err(Error::GitHub { status: 503, .. })
        ));
    }

    #[tokio::test]
    async fn pagination_follows_own_links_only() {
        let server = MockServer::start().await;
        let next = format!(
            "<{0}/items?page=2>; rel=\"next\", <{0}/items?page=2>; rel=\"last\"",
            server.uri()
        );
        Mock::given(method("GET"))
            .and(path("/items"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([3])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/items"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("link", next.as_str())
                    .set_body_json(json!([1, 2])),
            )
            .mount(&server)
            .await;
        let gh = gh(&server);
        assert_eq!(gh.get_pages::<u32>("/items").await.unwrap(), [1, 2, 3]);
        assert!(matches!(
            gh.get::<u32>("https://evil.example/items").await,
            Err(Error::Invalid(_))
        ));
        assert_eq!(next_link("<a>; rel=\"prev\""), None);
    }

    fn raw_invitation(id: u64, owner: &str, name: &str, topics: Option<&[&str]>) -> Value {
        let mut repo = json!({"name": name, "owner": {"login": owner}, "private": true});
        if let Some(t) = topics {
            repo["topics"] = json!(t);
        }
        json!({"id": id, "repository": repo, "inviter": {"login": owner}})
    }

    #[tokio::test]
    async fn invitations_are_filtered_by_topic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/repository_invitations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                raw_invitation(1, "a", "stuff", Some(&["claude-sync-group"])),
                raw_invitation(2, "b", "stuff", Some(&["other"])),
                raw_invitation(3, "c", "shared", None),
                raw_invitation(4, "d", "claude-sync-data", None),
                raw_invitation(5, "e", "secret", None),
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/c/shared/topics"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"names": ["claude-sync-group"]})),
            )
            .mount(&server)
            .await;
        for repo in ["/repos/d/claude-sync-data/topics", "/repos/e/secret/topics"] {
            Mock::given(method("GET"))
                .and(path(repo))
                .respond_with(
                    ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"})),
                )
                .mount(&server)
                .await;
        }
        Mock::given(method("PATCH"))
            .and(path("/user/repository_invitations/4"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let gh = gh(&server);
        let found: Vec<_> = gh
            .invitations()
            .await
            .unwrap()
            .into_iter()
            .map(|i| (i.id, i.repo.owner, i.inviter))
            .collect();
        let want = [(1, "a"), (3, "c"), (4, "d")].map(|(id, l)| (id, l.to_string(), l.to_string()));
        assert_eq!(found, want);
        assert_eq!(
            gh.accept_invitation(4).await.unwrap(),
            GroupId::new("d", "claude-sync-data")
        );
        assert!(gh.accept_invitation(99).await.is_err());
    }

    #[tokio::test]
    async fn group_repos_are_private_and_tagged() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .and(query_param("affiliation", "owner,collaborator"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"name": "g", "owner": {"login": "a"}, "private": true, "topics": ["claude-sync-group"]},
                {"name": "pub", "owner": {"login": "a"}, "private": false, "topics": ["claude-sync-group"]},
                {"name": "x", "owner": {"login": "b"}, "private": true, "topics": []},
            ])))
            .mount(&server)
            .await;
        assert_eq!(
            gh(&server).group_repos().await.unwrap(),
            [GroupId::new("a", "g")]
        );
    }

    #[tokio::test]
    async fn create_group_repo_makes_a_private_tagged_repo() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/user/repos"))
            .and(body_string_contains("\"private\":true"))
            .respond_with(ResponseTemplate::new(201).set_body_json(
                json!({"name": "claude-sync-data", "owner": {"login": "me"}, "private": true}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/repos/me/claude-sync-data/topics"))
            .and(body_string_contains("claude-sync-group"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"names": ["claude-sync-group"]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let id = gh(&server)
            .create_group_repo("claude-sync-data")
            .await
            .unwrap();
        assert_eq!(id, GroupId::new("me", "claude-sync-data"));
    }

    #[tokio::test]
    async fn create_group_repo_refuses_a_public_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(
                    json!({"name": "g", "owner": {"login": "me"}, "private": false}),
                ),
            )
            .mount(&server)
            .await;
        assert!(matches!(
            gh(&server).create_group_repo("g").await,
            Err(Error::PublicRepo(_))
        ));
    }
}
