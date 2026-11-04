//! Sign-in for the command line: the device flow, the token file and the session that
//! refreshes and saves it.
use crate::config;
use crate::store::{self, Saved};
use claude_sync_core::github::{self, DeviceCode, DevicePoll, GitHub, Session, Tokens};
use claude_sync_core::state::AppState;
use claude_sync_core::{Engine, Paths};
use std::sync::Arc;
use std::time::Duration;

/// A usable sign-in.
#[derive(Clone)]
pub struct Signed {
    pub login: String,
    pub session: Arc<Session>,
}

/// Where this machine stands.
pub enum Standing {
    In(Signed),
    /// Signed in before, but GitHub no longer accepts it (or the token file is gone).
    Expired(String),
    Out,
}

pub fn standing(paths: &Paths) -> Result<Standing, String> {
    let state = AppState::load(paths).map_err(|e| e.to_string())?;
    match store::load(paths)? {
        Some(saved) => {
            let session = new_session(paths, &saved, config::client_id(state.github_app));
            Ok(Standing::In(Signed {
                login: saved.login,
                session,
            }))
        }
        None => Ok(match state.login {
            Some(l) => Standing::Expired(l),
            None => Standing::Out,
        }),
    }
}

/// A session for the saved tokens. A refreshed token goes to the file; an expired one
/// removes it (the login stays in `state.json`, which marks the expiry).
pub fn new_session(paths: &Paths, saved: &Saved, client_id: &str) -> Arc<Session> {
    let (p1, p2) = (paths.clone(), paths.clone());
    let login = saved.login.clone();
    Arc::new(
        Session::new(saved.tokens.clone())
            .refreshable(client_id, github::LOGIN)
            .on_refresh(move |_, tokens| {
                let saved = Saved {
                    login: login.clone(),
                    tokens: tokens.clone(),
                };
                if let Err(e) = store::save(&p1, &saved) {
                    tracing::warn!("refreshed token not saved: {e}");
                }
            })
            .on_expired(move |_| {
                tracing::warn!("GitHub sign-in expired; run `claude-sync-cli login` again");
                if let Err(e) = store::delete(&p2) {
                    tracing::warn!("{e}");
                }
            }),
    )
}

pub fn engine(paths: &Paths, signed: &Signed) -> Result<Engine, String> {
    let state = AppState::load(paths).map_err(|e| e.to_string())?;
    let device = state
        .device_name
        .clone()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(default_device_name);
    Ok(Engine::new(
        paths.clone(),
        GitHub::with_session(signed.session.clone(), github::API),
        signed.login.clone(),
        device,
    ))
}

pub fn default_device_name() -> String {
    gethostname::gethostname()
        .to_string_lossy()
        .trim()
        .to_lowercase()
}

/// The signed-in engine, or the sentence that says how to sign in.
pub fn require(paths: &Paths) -> Result<(Signed, Engine), String> {
    match standing(paths)? {
        Standing::In(s) => {
            let e = engine(paths, &s)?;
            Ok((s, e))
        }
        Standing::Expired(l) => Err(format!(
            "The sign-in of {l} expired. Run `claude-sync-cli login`."
        )),
        Standing::Out => Err("Not signed in. Run `claude-sync-cli login`.".into()),
    }
}

/// Where the sign-in talks to (tests point these at a mock server).
pub struct Hosts {
    pub login: String,
    pub api: String,
}

impl Hosts {
    pub fn github() -> Self {
        Self {
            login: github::LOGIN.into(),
            api: github::API.into(),
        }
    }
}

/// The device flow: prints the URL and code with `show`, waits for approval with `sleep`,
/// saves the token and records the login in `state.json`. Returns the login.
pub async fn login(
    paths: &Paths,
    hosts: &Hosts,
    client_id: &str,
    show: impl Fn(&DeviceCode),
    sleep: impl AsyncFnMut(Duration),
) -> Result<String, String> {
    if client_id.is_empty() {
        return Err("This build has no GitHub client ID (CLAUDE_SYNC_GITHUB_CLIENT_ID), so it cannot sign in.".into());
    }
    let http = github::http_client();
    let code = github::start_device_flow_at(&http, &hosts.login, client_id)
        .await
        .map_err(|e| e.to_string())?;
    show(&code);
    let polled = github::drive_device_flow(
        &code,
        async || {
            github::poll_device_flow_at(&http, &hosts.login, client_id, &code.device_code).await
        },
        sleep,
    )
    .await
    .map_err(|e| e.to_string())?;
    let tokens: Tokens = match polled {
        DevicePoll::Token(t) => t,
        DevicePoll::Denied => return Err("Sign-in was cancelled on GitHub.".into()),
        _ => return Err("The code expired. Run `claude-sync-cli login` again.".into()),
    };
    let user = GitHub::with_api(tokens.access_token.clone(), hosts.api.clone())
        .user()
        .await
        .map_err(|e| e.to_string())?;
    store::save(
        paths,
        &Saved {
            login: user.login.clone(),
            tokens,
        },
    )?;
    let mut state = AppState::load(paths).map_err(|e| e.to_string())?;
    state.login = Some(user.login.clone());
    state.save(paths).map_err(|e| e.to_string())?;
    Ok(user.login)
}

/// After the saved sign-in expired: save the `gh` sign-in of the same account as this
/// sign-in, if GitHub accepts it. `gh_token` gives that token (`gh auth token`; `None` when
/// gh is missing or signed out). Returns whether a new sign-in was saved. The token is never
/// logged.
pub async fn recover_with(
    paths: &Paths,
    api: &str,
    login: &str,
    gh_token: impl FnOnce() -> Option<String>,
) -> Result<bool, String> {
    let Some(token) = gh_token()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    else {
        return Ok(false);
    };
    let Ok(user) = GitHub::with_api(token.clone(), api.to_string())
        .user()
        .await
    else {
        return Ok(false);
    };
    if !user.login.eq_ignore_ascii_case(login) {
        return Ok(false);
    }
    store::save(
        paths,
        &Saved {
            login: user.login.clone(),
            tokens: Tokens::plain(token),
        },
    )?;
    tracing::info!("sign-in of {login} recovered from the gh sign-in");
    Ok(true)
}

/// `recover_with` for the real `gh` command (`gh auth token`, argument list, no shell).
pub async fn recover_from_gh(paths: &Paths, api: &str, login: &str) -> Result<bool, String> {
    recover_with(paths, api, login, || {
        let out = std::process::Command::new("gh")
            .args(["auth", "token"])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    })
    .await
}

/// Remove the token and the login.
pub fn logout(paths: &Paths) -> Result<(), String> {
    store::delete(paths)?;
    let mut state = AppState::load(paths).map_err(|e| e.to_string())?;
    state.login = None;
    state.save(paths).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn paths(d: &std::path::Path) -> Paths {
        Paths {
            claude_home: d.join("claude"),
            state_dir: d.join("state"),
        }
    }

    async fn github_mock() -> MockServer {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "dev123",
                "user_code": "ABCD-1234",
                "verification_uri": "https://github.com/login/device",
                "expires_in": 900,
                "interval": 5
            })))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"access_token": "gho_abc", "token_type": "bearer"}),
            ))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"login": "octo"})),
            )
            .mount(&s)
            .await;
        s
    }

    #[tokio::test]
    async fn login_shows_the_code_and_saves_the_token() {
        let server = github_mock().await;
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let hosts = Hosts {
            login: server.uri(),
            api: server.uri(),
        };
        let shown = std::sync::Mutex::new(None);
        let login = login(
            &p,
            &hosts,
            "client",
            |c| *shown.lock().unwrap() = Some((c.user_code.clone(), c.verification_uri.clone())),
            async |_| {},
        )
        .await
        .unwrap();
        assert_eq!(login, "octo");
        assert_eq!(
            shown.lock().unwrap().clone().unwrap(),
            ("ABCD-1234".into(), "https://github.com/login/device".into())
        );
        let saved = store::load(&p).unwrap().unwrap();
        assert_eq!(saved.tokens.access_token, "gho_abc");
        assert_eq!(AppState::load(&p).unwrap().login.as_deref(), Some("octo"));
        assert!(matches!(standing(&p).unwrap(), Standing::In(s) if s.login == "octo"));
        logout(&p).unwrap();
        assert!(matches!(standing(&p).unwrap(), Standing::Out));
    }

    #[tokio::test]
    async fn login_reports_a_denied_code() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "d", "user_code": "U", "verification_uri": "u",
                "expires_in": 900, "interval": 5
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"error": "access_denied"})),
            )
            .mount(&server)
            .await;
        let d = tempfile::tempdir().unwrap();
        let hosts = Hosts {
            login: server.uri(),
            api: server.uri(),
        };
        let err = login(&paths(d.path()), &hosts, "client", |_| {}, async |_| {})
            .await
            .unwrap_err();
        assert!(err.contains("cancelled"), "{err}");
        assert!(store::load(&paths(d.path())).unwrap().is_none());
    }

    #[tokio::test]
    async fn an_expired_sign_in_is_recovered_from_the_gh_token_of_the_same_account() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"login": "octo"})),
            )
            .mount(&server)
            .await;
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let api = server.uri();
        assert!(recover_with(&p, &api, "octo", || Some("gho_gh\n".into()))
            .await
            .unwrap());
        assert_eq!(
            store::load(&p).unwrap().unwrap().tokens.access_token,
            "gho_gh"
        );
    }

    #[tokio::test]
    async fn a_gh_token_of_another_account_or_none_is_not_used() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"login": "someone"})),
            )
            .mount(&server)
            .await;
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let api = server.uri();
        assert!(!recover_with(&p, &api, "octo", || Some("gho_x".into()))
            .await
            .unwrap());
        assert!(!recover_with(&p, &api, "octo", || None).await.unwrap());
        assert!(store::load(&p).unwrap().is_none());
    }

    #[test]
    fn an_expired_login_is_told_apart_from_never_signed_in() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        assert!(matches!(standing(&p).unwrap(), Standing::Out));
        AppState {
            login: Some("octo".into()),
            ..AppState::default()
        }
        .save(&p)
        .unwrap();
        assert!(matches!(standing(&p).unwrap(), Standing::Expired(l) if l == "octo"));
        assert!(require(&p).err().unwrap().contains("expired"));
    }
}
