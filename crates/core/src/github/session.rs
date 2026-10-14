//! The signed-in session's tokens, and their refresh (§3).
//!
//! GitHub can hand out user tokens that expire (8 hours) together with a refresh token
//! (6 months). A [`Session`] holds the pair, refreshes the access token shortly before it
//! expires or after a `401`, and marks itself expired when GitHub refuses the refresh
//! token or a token without one stops working. Every [`super::GitHub`] clone of a session
//! shares it, so the sync engine, the background checks and the UI all see one token.
use super::{http_client, oauth_error, oauth_post, LOGIN};
use crate::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Refresh this long before the access token expires.
pub const REFRESH_MARGIN_SECS: u64 = 5 * 60;

/// An access token and, when GitHub sent them, its refresh token and expiry times
/// (unix seconds).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_expires_at: Option<u64>,
}

/// Never prints a token.
impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("refresh_token", &self.refresh_token.is_some())
            .field("expires_at", &self.expires_at)
            .field("refresh_token_expires_at", &self.refresh_token_expires_at)
            .finish_non_exhaustive()
    }
}

impl Tokens {
    /// A token that does not expire (an OAuth App token, a personal token, an old entry).
    pub fn plain(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            refresh_token: None,
            expires_at: None,
            refresh_token_expires_at: None,
        }
    }

    /// The tokens in a reply of `/login/oauth/access_token` (device flow or refresh).
    /// `expires_in` and `refresh_token_expires_in` become times from `now`.
    pub fn from_reply(reply: &Value, now: u64) -> Option<Self> {
        let access_token = reply.get("access_token")?.as_str()?.to_string();
        let text = |k: &str| {
            reply
                .get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let at = |k: &str| reply.get(k).and_then(Value::as_u64).map(|s| now + s);
        Some(Self {
            access_token,
            refresh_token: text("refresh_token"),
            expires_at: at("expires_in"),
            refresh_token_expires_at: at("refresh_token_expires_in"),
        })
    }

    /// What the keychain holds: the bare token when there is nothing else (the layout of
    /// 0.1.1 and earlier), else JSON.
    pub fn to_secret(&self) -> String {
        if self.refresh_token.is_none() && self.expires_at.is_none() {
            return self.access_token.clone();
        }
        serde_json::to_string(self).expect("tokens serialise")
    }

    /// Reads both keychain layouts. A GitHub token never starts with `{`.
    pub fn from_secret(secret: &str) -> Self {
        let secret = secret.trim();
        if secret.starts_with('{') {
            if let Ok(t) = serde_json::from_str::<Self>(secret) {
                return t;
            }
        }
        Self::plain(secret)
    }

    /// The access token expires within the margin and there is a refresh token to renew it.
    fn refresh_due(&self, now: u64) -> bool {
        self.refresh_token.is_some()
            && self
                .expires_at
                .is_some_and(|at| now + REFRESH_MARGIN_SECS >= at)
    }
}

/// Swap a refresh token for new tokens (`grant_type=refresh_token`). Device-flow apps
/// refresh without a client secret. A refusal (`bad_refresh_token`, ...) is
/// `Error::GitHub { status: 400 }`; network failures and server errors are other errors.
pub async fn refresh_tokens_at(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
    refresh_token: &str,
) -> crate::Result<Tokens> {
    let reply = oauth_post(
        http,
        &format!("{base}/login/oauth/access_token"),
        &[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await?;
    if let Some(t) = Tokens::from_reply(&reply, crate::state::now_unix()) {
        return Ok(t);
    }
    match reply.get("error").and_then(Value::as_str) {
        Some(code) => Err(oauth_error(code, &reply)),
        None => Err(Error::Invalid("GitHub sent no token and no error".into())),
    }
}

type RefreshHook = Box<dyn Fn(&Session, &Tokens) + Send + Sync>;
type ExpiredHook = Box<dyn Fn(&Session) + Send + Sync>;

/// One sign-in. Shared (`Arc`) by every client that uses it.
pub struct Session {
    tokens: Mutex<Tokens>,
    /// Held while a refresh runs, so only one runs at a time (single flight).
    refreshing: tokio::sync::Mutex<()>,
    expired: AtomicBool,
    client_id: String,
    login_base: String,
    http: reqwest::Client,
    on_refresh: Option<RefreshHook>,
    on_expired: Option<ExpiredHook>,
}

impl Session {
    /// A session that cannot refresh until [`Session::refreshable`] says how.
    pub fn new(tokens: Tokens) -> Self {
        Self {
            tokens: Mutex::new(tokens),
            refreshing: tokio::sync::Mutex::new(()),
            expired: AtomicBool::new(false),
            client_id: String::new(),
            login_base: LOGIN.into(),
            http: http_client(),
            on_refresh: None,
            on_expired: None,
        }
    }

    /// Refresh with this OAuth client ID at `login_base` (`LOGIN`, or a test server).
    pub fn refreshable(mut self, client_id: &str, login_base: &str) -> Self {
        self.client_id = client_id.into();
        self.login_base = login_base.trim_end_matches('/').into();
        self
    }

    /// Called with the new tokens after each refresh (to save them).
    pub fn on_refresh(mut self, f: impl Fn(&Session, &Tokens) + Send + Sync + 'static) -> Self {
        self.on_refresh = Some(Box::new(f));
        self
    }

    /// Called once, when the session expires.
    pub fn on_expired(mut self, f: impl Fn(&Session) + Send + Sync + 'static) -> Self {
        self.on_expired = Some(Box::new(f));
        self
    }

    /// The current tokens, as they are.
    pub fn tokens(&self) -> Tokens {
        self.tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn is_expired(&self) -> bool {
        self.expired.load(Ordering::SeqCst)
    }

    fn can_refresh(&self, t: &Tokens) -> bool {
        !self.client_id.is_empty() && t.refresh_token.is_some()
    }

    /// The access token to send now. Refreshes it first when it expires within
    /// [`REFRESH_MARGIN_SECS`]. A refresh that fails for a passing reason (network) keeps the
    /// current token: a `401` brings the refresh back. `Error::SignInExpired` once expired.
    pub async fn token(&self) -> crate::Result<String> {
        if self.is_expired() {
            return Err(Error::SignInExpired);
        }
        let t = self.tokens();
        if !(self.can_refresh(&t) && t.refresh_due(crate::state::now_unix())) {
            return Ok(t.access_token);
        }
        match self.renew(&t.access_token).await {
            Ok(Some(token)) => Ok(token),
            Ok(None) => Ok(t.access_token),
            Err(Error::SignInExpired) => Err(Error::SignInExpired),
            Err(e) => {
                tracing::warn!("token refresh failed, trying the current token: {e}");
                Ok(t.access_token)
            }
        }
    }

    /// A newer token than `used`, which GitHub refused or which is about to expire:
    /// another caller's refresh if one happened meanwhile, else a refresh of our own.
    /// `None` when there is no way to get one (no refresh token). A refused refresh
    /// expires the session.
    pub async fn renew(&self, used: &str) -> crate::Result<Option<String>> {
        let _one = self.refreshing.lock().await;
        if self.is_expired() {
            return Err(Error::SignInExpired);
        }
        let t = self.tokens();
        if t.access_token != used {
            return Ok(Some(t.access_token));
        }
        let Some(refresh_token) = t.refresh_token.as_deref().filter(|_| self.can_refresh(&t))
        else {
            return Ok(None);
        };
        match refresh_tokens_at(&self.http, &self.login_base, &self.client_id, refresh_token).await
        {
            Ok(mut new) => {
                // GitHub rotates the refresh token; keep the old one if it sent none.
                if new.refresh_token.is_none() {
                    new.refresh_token = t.refresh_token.clone();
                    new.refresh_token_expires_at = t.refresh_token_expires_at;
                }
                *self.tokens.lock().unwrap_or_else(|e| e.into_inner()) = new.clone();
                tracing::info!("GitHub token refreshed");
                if let Some(f) = &self.on_refresh {
                    f(self, &new);
                }
                Ok(Some(new.access_token))
            }
            Err(e @ Error::GitHub { status: 400, .. }) => {
                tracing::warn!("GitHub refused the refresh token: {e}");
                self.expire();
                Err(Error::SignInExpired)
            }
            Err(e) => Err(e),
        }
    }

    /// The tokens no longer work: every later call fails with `Error::SignInExpired`
    /// without reaching GitHub. The hook runs on the first call only.
    pub fn expire(&self) {
        if !self.expired.swap(true, Ordering::SeqCst) {
            tracing::warn!("the GitHub sign-in expired");
            if let Some(f) = &self.on_expired {
                f(self);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::GitHub;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use std::time::Duration;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn now() -> u64 {
        crate::state::now_unix()
    }

    fn tokens(access: &str, refresh: Option<&str>, expires_at: Option<u64>) -> Tokens {
        Tokens {
            access_token: access.into(),
            refresh_token: refresh.map(str::to_string),
            expires_at,
            refresh_token_expires_at: refresh.map(|_| now() + 15_897_600),
        }
    }

    /// A session against `server` that counts its hook calls.
    fn session(
        server: &MockServer,
        t: Tokens,
    ) -> (Arc<Session>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let refreshed = Arc::new(AtomicUsize::new(0));
        let expired = Arc::new(AtomicUsize::new(0));
        let (r, e) = (refreshed.clone(), expired.clone());
        let s = Session::new(t)
            .refreshable("cid", &server.uri())
            .on_refresh(move |_, _| {
                r.fetch_add(1, Ordering::SeqCst);
            })
            .on_expired(move |_| {
                e.fetch_add(1, Ordering::SeqCst);
            });
        (Arc::new(s), refreshed, expired)
    }

    fn gh(server: &MockServer, s: &Arc<Session>) -> GitHub {
        GitHub::with_session(s.clone(), server.uri()).with_retry_delay(Duration::from_millis(1))
    }

    async fn refresh_replies(server: &MockServer, reply: Value, times: u64) {
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("client_id=cid"))
            .and(body_string_contains("refresh_token=ghr_old"))
            .respond_with(ResponseTemplate::new(200).set_body_json(reply))
            .expect(times)
            .mount(server)
            .await;
    }

    async fn user_accepts(server: &MockServer, token: &str) {
        Mock::given(method("GET"))
            .and(path("/user"))
            .and(header("authorization", format!("Bearer {token}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"login": "ana"})))
            .mount(server)
            .await;
    }

    async fn rejects_others(server: &MockServer) {
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"message": "Bad credentials"})),
            )
            .with_priority(10)
            .mount(server)
            .await;
    }

    fn new_pair() -> Value {
        json!({
            "access_token": "ghu_new", "token_type": "bearer", "scope": "",
            "expires_in": 28800, "refresh_token": "ghr_new", "refresh_token_expires_in": 15897600
        })
    }

    #[test]
    fn reply_and_keychain_layouts() {
        let t = Tokens::from_reply(&new_pair(), 1000).unwrap();
        assert_eq!(t.access_token, "ghu_new");
        assert_eq!(t.refresh_token.as_deref(), Some("ghr_new"));
        assert_eq!(t.expires_at, Some(29_800));
        assert_eq!(t.refresh_token_expires_at, Some(15_898_600));
        assert_eq!(Tokens::from_secret(&t.to_secret()), t);
        // An OAuth App token that never expires is stored bare, as before.
        let plain = Tokens::from_reply(&json!({"access_token": "gho_x"}), 1000).unwrap();
        assert_eq!(plain, Tokens::plain("gho_x"));
        assert_eq!(plain.to_secret(), "gho_x");
        // A 0.1.1 keychain entry: the bare token.
        assert_eq!(
            Tokens::from_secret("gho_legacy\n"),
            Tokens::plain("gho_legacy")
        );
        assert!(Tokens::from_reply(&json!({"error": "x"}), 0).is_none());
        let shown = format!("{t:?}");
        assert!(
            !shown.contains("ghu_new") && !shown.contains("ghr_new"),
            "{shown}"
        );
    }

    #[tokio::test]
    async fn refreshes_before_expiry() {
        let server = MockServer::start().await;
        refresh_replies(&server, new_pair(), 1).await;
        user_accepts(&server, "ghu_new").await;
        rejects_others(&server).await;
        // Expires in one minute: inside the five-minute margin.
        let (s, refreshed, expired) = session(
            &server,
            tokens("ghu_old", Some("ghr_old"), Some(now() + 60)),
        );
        assert_eq!(gh(&server, &s).user().await.unwrap().login, "ana");
        assert_eq!(gh(&server, &s).user().await.unwrap().login, "ana");
        let t = s.tokens();
        assert_eq!(
            (t.access_token.as_str(), t.refresh_token.as_deref()),
            ("ghu_new", Some("ghr_new"))
        );
        assert!(t.expires_at.unwrap() > now() + 28_000);
        assert_eq!(refreshed.load(Ordering::SeqCst), 1, "saved once");
        assert_eq!(expired.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn token_far_from_expiry_is_not_refreshed() {
        let server = MockServer::start().await;
        refresh_replies(&server, new_pair(), 0).await;
        user_accepts(&server, "ghu_old").await;
        let (s, refreshed, _) = session(
            &server,
            tokens("ghu_old", Some("ghr_old"), Some(now() + 3600)),
        );
        assert_eq!(gh(&server, &s).user().await.unwrap().login, "ana");
        assert_eq!(refreshed.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn refreshes_after_a_401_and_retries() {
        let server = MockServer::start().await;
        refresh_replies(&server, new_pair(), 1).await;
        user_accepts(&server, "ghu_new").await;
        rejects_others(&server).await;
        // No expiry known (or a clock that is off): GitHub's 401 is the signal.
        let (s, refreshed, expired) = session(&server, tokens("ghu_old", Some("ghr_old"), None));
        assert_eq!(gh(&server, &s).user().await.unwrap().login, "ana");
        assert_eq!(s.tokens().access_token, "ghu_new");
        assert_eq!(refreshed.load(Ordering::SeqCst), 1);
        assert_eq!(expired.load(Ordering::SeqCst), 0);
    }

    /// Eight callers at once with `t`; GitHub must see exactly one refresh.
    async fn eight_at_once(t: Tokens) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(new_pair())
                    .set_delay(Duration::from_millis(200)),
            )
            .expect(1)
            .mount(&server)
            .await;
        user_accepts(&server, "ghu_new").await;
        rejects_others(&server).await;
        let (s, refreshed, _) = session(&server, t);
        let calls: Vec<_> = (0..8)
            .map(|_| {
                let g = gh(&server, &s);
                tokio::spawn(async move { g.user().await })
            })
            .collect();
        for c in calls {
            assert_eq!(c.await.unwrap().unwrap().login, "ana");
        }
        assert_eq!(refreshed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_callers_share_one_refresh() {
        // All see a token about to expire.
        eight_at_once(tokens("ghu_old", Some("ghr_old"), Some(now() + 10))).await;
        // All get a 401 first.
        eight_at_once(tokens("ghu_old", Some("ghr_old"), None)).await;
    }

    #[tokio::test]
    async fn refused_refresh_expires_the_session() {
        let server = MockServer::start().await;
        refresh_replies(
            &server,
            json!({"error": "bad_refresh_token", "error_description": "The refresh token passed is incorrect or expired."}),
            1,
        )
        .await;
        rejects_others(&server).await;
        let (s, refreshed, expired) = session(&server, tokens("ghu_old", Some("ghr_old"), None));
        let g = gh(&server, &s);
        assert!(matches!(g.user().await, Err(Error::SignInExpired)));
        assert!(s.is_expired());
        assert_eq!(expired.load(Ordering::SeqCst), 1);
        assert_eq!(refreshed.load(Ordering::SeqCst), 0);
        // Later calls stop before the network (the refresh mock expects one call).
        let before = server.received_requests().await.unwrap().len();
        assert!(matches!(g.user().await, Err(Error::SignInExpired)));
        assert!(matches!(g.invitations().await, Err(Error::SignInExpired)));
        assert_eq!(server.received_requests().await.unwrap().len(), before);
        assert_eq!(expired.load(Ordering::SeqCst), 1, "the hook runs once");
    }

    #[tokio::test]
    async fn refused_refresh_before_expiry_expires_the_session() {
        let server = MockServer::start().await;
        refresh_replies(&server, json!({"error": "bad_refresh_token"}), 1).await;
        let (s, _, expired) = session(
            &server,
            tokens("ghu_old", Some("ghr_old"), Some(now() + 60)),
        );
        assert!(matches!(
            gh(&server, &s).user().await,
            Err(Error::SignInExpired)
        ));
        assert_eq!(expired.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn network_failure_of_a_refresh_does_not_expire() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"))
            .mount(&server)
            .await;
        rejects_others(&server).await;
        let (s, _, expired) = session(&server, tokens("ghu_old", Some("ghr_old"), None));
        let err = gh(&server, &s).user().await.unwrap_err();
        assert!(matches!(err, Error::GitHub { status: 502, .. }), "{err}");
        assert!(!s.is_expired());
        assert_eq!(expired.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn legacy_token_without_refresh_expires_on_a_confirmed_401() {
        let server = MockServer::start().await;
        refresh_replies(&server, new_pair(), 0).await;
        Mock::given(method("GET"))
            .and(path("/user/repository_invitations"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"message": "Bad credentials"})),
            )
            .mount(&server)
            .await;
        rejects_others(&server).await;
        // What 0.1.1 saved: the bare access token.
        let (s, _, expired) = session(&server, Tokens::from_secret("gho_legacy"));
        let g = gh(&server, &s);
        assert!(matches!(g.invitations().await, Err(Error::SignInExpired)));
        assert_eq!(expired.load(Ordering::SeqCst), 1);
        let paths: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| r.url.path().to_string())
            .collect();
        assert_eq!(
            paths,
            ["/user/repository_invitations", "/user"],
            "confirmed with /user"
        );
    }

    #[tokio::test]
    async fn a_401_that_does_not_persist_is_only_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({"message": "odd"})))
            .mount(&server)
            .await;
        user_accepts(&server, "gho_fine").await;
        let (s, _, expired) = session(&server, Tokens::plain("gho_fine"));
        let err = gh(&server, &s)
            .get::<Value>("/repos/o/r")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::GitHub { status: 401, .. }));
        assert!(!s.is_expired());
        assert_eq!(expired.load(Ordering::SeqCst), 0);
    }
}
