//! The one HTTP pipeline (v2-EngRules: every GitHub call goes through it). Rules are the v1
//! ones (v1-SpecGithub4): 3 attempts with 0.5 s doubling for connect failures, GET timeouts
//! and GET 502/503/504; a GET ETag cache (never for other methods or answers without an ETag);
//! rate limits kept per resource; a 401 renews once, then asks GET /user before signing out.
use crate::secrets::Tokens;
use crate::{Error, Result};
use reqwest::blocking::{Client as Http, Response as HttpResponse};
use reqwest::header::{HeaderMap, ACCEPT, IF_NONE_MATCH};
use reqwest::Method;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once};
use std::time::Duration;

const ATTEMPTS: u32 = 3;
/// Cached GET answers kept; a new URL on a full cache clears it (v1 CACHE_LIMIT).
const CACHE_LIMIT: usize = 256;

#[derive(Debug, Clone)]
pub struct Config {
    /// REST root.
    pub api: String,
    /// Host of the OAuth device flow.
    pub login: String,
    /// The relay (feedback, health).
    pub relay: String,
    /// Where `latest.json` lives.
    pub releases: String,
    pub client_id: String,
    /// First wait before a retry; doubles.
    pub retry_delay: Duration,
}

impl Config {
    pub fn github() -> Self {
        Config {
            api: "https://api.github.com".into(),
            login: "https://github.com".into(),
            relay: crate::RELAY_URL.into(),
            releases: "https://github.com/MehdiMamas/claude-sync/releases".into(),
            client_id: crate::CLIENT_ID.into(),
            retry_delay: Duration::from_millis(500),
        }
    }
}

/// One counted line: who asked, which endpoint, how it ended (`200`, `304` or `other`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Count {
    pub caller: String,
    pub endpoint: String,
    pub result: String,
    pub count: u64,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub etag: Option<String>,
    /// `rel="next"` of the Link header.
    pub next: Option<String>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|e| Error::Invalid(format!("bad reply: {e}")))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Resource {
    Core,
    Graphql,
}

type OnTokens = Box<dyn Fn(&Tokens) + Send + Sync>;

struct Inner {
    http: Http,
    cfg: Config,
    tokens: Mutex<Option<Tokens>>,
    on_tokens: Mutex<Option<OnTokens>>,
    expired: AtomicBool,
    cache: Mutex<HashMap<String, (String, Vec<u8>)>>,
    limits: Mutex<HashMap<Resource, i64>>,
    counts: Mutex<BTreeMap<(String, String, String), u64>>,
}

/// Cheap to clone; clones share the session, limits, cache and counts.
#[derive(Clone)]
pub struct Client(Arc<Inner>);

thread_local! {
    static CALLER: RefCell<String> = RefCell::new("other".into());
}

/// Name the caller of every request made inside `f` on this thread (request counts).
pub fn with_caller<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let old = CALLER.with(|c| c.replace(name.into()));
    let out = f();
    CALLER.with(|c| *c.borrow_mut() = old);
    out
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Raw {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Raw {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

impl Client {
    pub fn new(cfg: Config) -> Result<Client> {
        static PROVIDER: Once = Once::new();
        // A race lost to another installer is fine.
        PROVIDER.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
        let http = Http::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .user_agent("claude-sync")
            .build()
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        Ok(Client(Arc::new(Inner {
            http,
            cfg,
            tokens: Mutex::new(None),
            on_tokens: Mutex::new(None),
            expired: AtomicBool::new(false),
            cache: Mutex::default(),
            limits: Mutex::default(),
            counts: Mutex::default(),
        })))
    }

    pub fn config(&self) -> &Config {
        &self.0.cfg
    }

    /// Sign in with these tokens (clears an earlier expiry).
    pub fn set_tokens(&self, t: Tokens) {
        *lock(&self.0.tokens) = Some(t);
        self.0.expired.store(false, Ordering::SeqCst);
    }

    pub fn sign_out(&self) {
        *lock(&self.0.tokens) = None;
    }

    /// Called with a refreshed token set so the caller can store it.
    pub fn on_tokens(&self, f: impl Fn(&Tokens) + Send + Sync + 'static) {
        *lock(&self.0.on_tokens) = Some(Box::new(f));
    }

    /// True once GitHub confirmed the token is dead; every later call fails until `set_tokens`.
    pub fn is_expired(&self) -> bool {
        self.0.expired.load(Ordering::SeqCst)
    }

    /// Reset time of the REST limit while it is in the future.
    pub fn limited_until(&self) -> Option<i64> {
        self.limit_of(Resource::Core)
    }

    pub fn graphql_limited_until(&self) -> Option<i64> {
        self.limit_of(Resource::Graphql)
    }

    /// Request counts, most frequent first.
    pub fn counts(&self) -> Vec<Count> {
        let mut v: Vec<Count> = lock(&self.0.counts)
            .iter()
            .map(|((caller, endpoint, result), count)| Count {
                caller: caller.clone(),
                endpoint: endpoint.clone(),
                result: result.clone(),
                count: *count,
            })
            .collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.count));
        v
    }

    // ----- requests ------------------------------------------------------------------

    pub(crate) fn get(&self, path: &str) -> Result<Response> {
        self.request(Method::GET, path, None, None)
    }

    /// A GET with the caller's own ETag: `304` comes back as status 304, not from the cache.
    pub(crate) fn get_etag(&self, path: &str, etag: Option<&str>) -> Result<Response> {
        self.request(Method::GET, path, None, etag)
    }

    pub(crate) fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Response> {
        self.request(method, path, body, None)
    }

    /// Every page of a list: follows `rel="next"` until none.
    pub(crate) fn get_pages<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        let (mut out, mut next) = (Vec::new(), Some(path.to_string()));
        while let Some(p) = next {
            let r = self.get(&p)?;
            out.extend(r.json::<Vec<T>>()?);
            next = r.next;
        }
        Ok(out)
    }

    fn url(&self, path: &str) -> Result<String> {
        let api = self.0.cfg.api.trim_end_matches('/');
        if path.starts_with("http://") || path.starts_with("https://") {
            if path.starts_with(&format!("{api}/")) {
                return Ok(path.into());
            }
            return Err(Error::Invalid(format!(
                "refusing to send the token to {path}"
            )));
        }
        Ok(format!("{api}{path}"))
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        etag: Option<&str>,
    ) -> Result<Response> {
        let url = self.url(path)?;
        let res = if url.ends_with("/graphql") {
            Resource::Graphql
        } else {
            Resource::Core
        };
        self.check_limit(res)?;
        let mut token = self.token()?;
        let mut renewed = false;
        loop {
            let raw = self.send_raw(&method, &url, Some(&token), body, etag)?;
            if raw.status == 401 && !token.is_empty() {
                if !renewed {
                    renewed = true;
                    if let Some(t) = self.renew()? {
                        token = t;
                        continue;
                    }
                }
                return Err(self.confirm_401(&method, path, &token, &raw));
            }
            return self.finish(raw, res, &method, &url, etag.is_some());
        }
    }

    /// Fresh enough access token: refresh ahead of expiry when due.
    pub(crate) fn token(&self) -> Result<String> {
        if self.is_expired() {
            return Err(Error::SignInExpired);
        }
        let due = match &*lock(&self.0.tokens) {
            None => return Err(Error::NotSignedIn),
            Some(t) => t.refresh_due(now()),
        };
        if due {
            if let Some(t) = self.renew()? {
                return Ok(t);
            }
        }
        lock(&self.0.tokens)
            .as_ref()
            .map(|t| t.access_token.clone())
            .ok_or(Error::NotSignedIn)
    }

    /// Exchange the refresh token (GitHub App mode); `None` when there is none.
    fn renew(&self) -> Result<Option<String>> {
        let Some(refresh) = lock(&self.0.tokens)
            .as_ref()
            .and_then(|t| t.refresh_token.clone())
        else {
            return Ok(None);
        };
        match crate::device::refresh(self, &refresh) {
            Ok(t) => {
                let access = t.access_token.clone();
                if let Some(f) = &*lock(&self.0.on_tokens) {
                    f(&t);
                }
                *lock(&self.0.tokens) = Some(t);
                Ok(Some(access))
            }
            Err(Error::Github { .. }) => {
                self.0.expired.store(true, Ordering::SeqCst);
                Err(Error::SignInExpired)
            }
            Err(e) => Err(e),
        }
    }

    /// A 401 signs the person out only when GET /user with the same token says 401 too, so one
    /// odd 401 on one repo never does.
    fn confirm_401(&self, method: &Method, path: &str, token: &str, first: &Raw) -> Error {
        let dead = if *method == Method::GET && path.split('?').next() == Some("/user") {
            true
        } else {
            self.url("/user")
                .and_then(|u| self.send_raw(&Method::GET, &u, Some(token), None, None))
                .map(|r| r.status == 401)
                .unwrap_or(false)
        };
        if dead {
            self.0.expired.store(true, Ordering::SeqCst);
            return Error::SignInExpired;
        }
        Error::Github {
            status: 401,
            message: error_message(&first.body),
        }
    }

    /// One request with retries; every answer that arrives is counted.
    fn send_raw(
        &self,
        method: &Method,
        url: &str,
        token: Option<&str>,
        body: Option<&Value>,
        etag: Option<&str>,
    ) -> Result<Raw> {
        let get = *method == Method::GET;
        let cached = if get && etag.is_none() {
            lock(&self.0.cache).get(url).map(|(e, _)| e.clone())
        } else {
            None
        };
        let inm = etag.map(str::to_string).or(cached);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut rb = self
                .0
                .http
                .request(method.clone(), url)
                .header(ACCEPT, "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28");
            if let Some(t) = token.filter(|t| !t.is_empty()) {
                rb = rb.bearer_auth(t);
            }
            if let Some(b) = body {
                rb = rb.json(b);
            }
            if let Some(e) = &inm {
                rb = rb.header(IF_NONE_MATCH, e);
            }
            let again = attempt < ATTEMPTS;
            match rb.send().and_then(read) {
                Ok(raw) => {
                    self.count(method, url, raw.status);
                    if get && again && matches!(raw.status, 502..=504) {
                        self.backoff(attempt);
                        continue;
                    }
                    return Ok(raw);
                }
                Err(e) => {
                    if again && (e.is_connect() || (get && (e.is_timeout() || e.is_request()))) {
                        self.backoff(attempt);
                        continue;
                    }
                    return Err(Error::Network(e.without_url().to_string()));
                }
            }
        }
    }

    fn backoff(&self, attempt: u32) {
        std::thread::sleep(self.0.cfg.retry_delay * 2u32.pow(attempt - 1));
    }

    fn count(&self, method: &Method, url: &str, status: u16) {
        let api = self.0.cfg.api.trim_end_matches('/');
        let rel = url.strip_prefix(api).unwrap_or(url);
        let result = match status {
            304 => "304",
            200..=299 => "200",
            _ => "other",
        };
        let key = (
            CALLER.with(|c| c.borrow().clone()),
            format!("{method} {}", normalize(rel)),
            result.to_string(),
        );
        *lock(&self.0.counts).entry(key).or_default() += 1;
    }

    // ----- limits --------------------------------------------------------------------

    fn limit_of(&self, r: Resource) -> Option<i64> {
        lock(&self.0.limits).get(&r).copied().filter(|t| *t > now())
    }

    fn check_limit(&self, r: Resource) -> Result<()> {
        match self.limit_of(r) {
            Some(reset_unix) => Err(Error::RateLimited { reset_unix }),
            None => {
                lock(&self.0.limits).remove(&r);
                Ok(())
            }
        }
    }

    fn finish(
        &self,
        raw: Raw,
        own: Resource,
        method: &Method,
        url: &str,
        explicit_etag: bool,
    ) -> Result<Response> {
        // Any resource other than "graphql" (core, search, ...) counts as core.
        let res = match raw.header("x-ratelimit-resource") {
            Some("graphql") => Resource::Graphql,
            Some(_) => Resource::Core,
            None => own,
        };
        let remaining_zero = raw.header("x-ratelimit-remaining") == Some("0");
        let reset = raw
            .header("x-ratelimit-reset")
            .and_then(|v| v.parse::<i64>().ok());
        let ok = raw.status / 100 == 2 || raw.status == 304;
        if ok {
            if let (true, Some(r)) = (remaining_zero, reset) {
                if r > now() {
                    lock(&self.0.limits).insert(res, r);
                }
            }
            return Ok(self.answer(raw, method, url, explicit_etag));
        }
        if raw.status == 403 || raw.status == 429 {
            let text = String::from_utf8_lossy(&raw.body).to_lowercase();
            let retry_after = raw
                .header("retry-after")
                .and_then(|v| v.trim().parse::<i64>().ok());
            let reset_unix = if let Some(s) = retry_after {
                Some(now() + s)
            } else if remaining_zero {
                Some(reset.unwrap_or(now() + 60))
            } else if raw.status == 429 || text.contains("secondary rate limit") {
                Some(now() + 60)
            } else {
                None
            };
            if let Some(reset_unix) = reset_unix {
                lock(&self.0.limits).insert(res, reset_unix);
                return Err(Error::RateLimited { reset_unix });
            }
        }
        Err(Error::Github {
            status: raw.status,
            message: error_message(&raw.body),
        })
    }

    /// Success: serve a 304 from the cache, store a GET 200 that has an ETag.
    fn answer(&self, raw: Raw, method: &Method, url: &str, explicit_etag: bool) -> Response {
        let etag = raw.header("etag").map(str::to_string);
        let next = raw.header("link").and_then(next_link);
        let get = *method == Method::GET;
        if get && !explicit_etag {
            let mut cache = lock(&self.0.cache);
            if raw.status == 304 {
                if let Some((e, body)) = cache.get(url) {
                    return Response {
                        status: 200,
                        etag: Some(e.clone()),
                        next,
                        body: body.clone(),
                    };
                }
            } else if let Some(e) = &etag {
                if cache.len() >= CACHE_LIMIT && !cache.contains_key(url) {
                    cache.clear();
                }
                cache.insert(url.to_string(), (e.clone(), raw.body.clone()));
            }
        }
        Response {
            status: raw.status,
            etag,
            next,
            body: raw.body,
        }
    }

    // ----- calls that bypass the GitHub pipeline (no token, other hosts) -------------

    /// A plain call to another host, counted like any other. Used for OAuth, releases, relay.
    pub(crate) fn plain(
        &self,
        method: Method,
        url: &str,
        token: Option<&str>,
        body: PlainBody,
    ) -> Result<(u16, Vec<u8>)> {
        let mut rb = self.0.http.request(method.clone(), url);
        rb = match &body {
            PlainBody::None => rb,
            PlainBody::Form(f) => rb.header(ACCEPT, "application/json").form(f),
            PlainBody::Json(v) => rb.json(v),
        };
        if let Some(t) = token {
            rb = rb.bearer_auth(t);
        }
        let raw = rb
            .send()
            .and_then(read)
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        let path = url
            .split_once("://")
            .and_then(|(_, r)| r.split_once('/'))
            .map(|(_, p)| format!("/{p}"))
            .unwrap_or_default();
        self.count(&method, &path, raw.status);
        Ok((raw.status, raw.body))
    }
}

pub(crate) enum PlainBody {
    None,
    Form(Vec<(&'static str, String)>),
    Json(Value),
}

fn read(r: HttpResponse) -> reqwest::Result<Raw> {
    let status = r.status().as_u16();
    let headers = r.headers().clone();
    let body = r.bytes()?.to_vec();
    Ok(Raw {
        status,
        headers,
        body,
    })
}

/// `<url>; rel="next", ...` to the next URL.
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

/// GitHub's message plus the details of `errors`; else the first 200 characters of the body.
pub(crate) fn error_message(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let raw = || text.chars().take(200).collect::<String>();
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return raw();
    };
    let Some(msg) = v.get("message").and_then(Value::as_str) else {
        return raw();
    };
    let detail = |e: &Value| -> Option<String> {
        if let Some(s) = e.as_str() {
            return Some(s.to_string());
        }
        if let Some(m) = e.get("message").and_then(Value::as_str) {
            return Some(m.to_string());
        }
        let parts: Vec<&str> = ["resource", "field", "code"]
            .iter()
            .filter_map(|k| e.get(k).and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .collect();
        (!parts.is_empty()).then(|| {
            let (head, tail) = parts.split_at(parts.len().min(2));
            format!("{} {}", head.join("."), tail.join(" "))
                .trim()
                .to_string()
        })
    };
    let details: Vec<String> = match v.get("errors") {
        Some(Value::Array(a)) => a.iter().filter_map(detail).collect(),
        Some(e @ (Value::String(_) | Value::Object(_))) => detail(e).into_iter().collect(),
        _ => vec![],
    };
    if details.is_empty() {
        msg.to_string()
    } else {
        format!("{msg}: {}", details.join("; "))
    }
}

/// Endpoint label for the counts: ids and names become placeholders (v1-GithubClient row 8).
pub(crate) fn normalize(path: &str) -> String {
    let path = path.split('?').next().unwrap_or("");
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        let s = segs[i];
        let more = i + 1 < segs.len();
        match s {
            "repos" if i + 2 < segs.len() => {
                out.extend(["repos".into(), ":repo".into()]);
                i += 3;
                continue;
            }
            "users" | "orgs" if more => {
                out.extend([s.into(), ":user".into()]);
                i += 2;
                continue;
            }
            "contents" => {
                out.push(s.into());
                if more {
                    out.push(":path".into());
                }
                break;
            }
            "ref" | "refs" | "heads" | "tags" => {
                out.push(s.into());
                if more {
                    out.push(":ref".into());
                }
                break;
            }
            _ if s.contains("...") => out.push(":range".into()),
            _ if is_id(s) => out.push(":id".into()),
            _ => out.push(s.into()),
        }
        i += 1;
    }
    out.join("/")
}

fn is_id(s: &str) -> bool {
    let digits = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let hex = s.len() >= 7
        && s.bytes().all(|b| b.is_ascii_hexdigit())
        && s.bytes().any(|b| b.is_ascii_digit());
    digits || hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_endpoints() {
        assert_eq!(
            normalize("/repos/o/r/commits/main?x=1"),
            "repos/:repo/commits/main"
        );
        assert_eq!(
            normalize("/repos/o/r/pulls/12/reviews"),
            "repos/:repo/pulls/:id/reviews"
        );
        assert_eq!(
            normalize("/repos/o/r/git/refs/heads/main"),
            "repos/:repo/git/refs/:ref"
        );
        assert_eq!(normalize("/repos/o/r/git/refs"), "repos/:repo/git/refs");
        assert_eq!(
            normalize("/repos/o/r/compare/a1b2c3d...e4f5a6b"),
            "repos/:repo/compare/:range"
        );
        assert_eq!(
            normalize("/repos/o/r/git/trees/0123abc9"),
            "repos/:repo/git/trees/:id"
        );
        assert_eq!(normalize("/user"), "user");
    }

    #[test]
    fn reads_next_link_and_error_text() {
        let l = r#"<https://x/a?page=1>; rel="prev", <https://x/a?page=3>; rel="next""#;
        assert_eq!(next_link(l).as_deref(), Some("https://x/a?page=3"));
        assert_eq!(next_link(r#"<https://x>; rel=next"#), None);
        let b = br#"{"message":"Validation Failed","errors":[{"resource":"Repo","field":"name","code":"taken"},"plain"]}"#;
        assert_eq!(
            error_message(b),
            "Validation Failed: Repo.name taken; plain"
        );
        assert_eq!(error_message(b"not json"), "not json");
    }
}
