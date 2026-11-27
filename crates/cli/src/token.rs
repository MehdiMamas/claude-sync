//! The headless sign-in: `{login, tokens}` in `<state>/token`, mode 0600, the same JSON as v1.
//! It plugs into the actions as a `SecretStore`, so the engine never knows it is not a keychain.
use claude_sync_actions::Core;
use claude_sync_github::secrets::{file_delete, file_load, file_save, Saved, SecretStore, Tokens};
use claude_sync_github::Result;
use std::path::PathBuf;

const CURRENT: &str = "@current";

pub struct FileSecrets(pub PathBuf);

impl SecretStore for FileSecrets {
    fn get(&self, account: &str) -> Result<Option<String>> {
        Ok(file_load(&self.0)?.and_then(|s| {
            if account == CURRENT {
                Some(s.login)
            } else if s.login == account {
                Some(s.tokens.to_secret())
            } else {
                None
            }
        }))
    }

    fn set(&self, account: &str, secret: &str) -> Result<()> {
        // The login and its tokens are one file: `@current` is written with the tokens.
        if account == CURRENT {
            return Ok(());
        }
        file_save(
            &self.0,
            &Saved {
                login: account.into(),
                tokens: Tokens::from_secret(secret),
            },
        )
    }

    fn delete(&self, account: &str) -> Result<()> {
        match file_load(&self.0)? {
            Some(s) if account == CURRENT || s.login == account => file_delete(&self.0),
            _ => Ok(()),
        }
    }
}

/// A sign-in held in memory only: nothing on disk, nothing in the keychain.
#[derive(Default)]
pub struct MemSecrets(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl SecretStore for MemSecrets {
    fn get(&self, a: &str) -> Result<Option<String>> {
        Ok(self.0.lock().ok().and_then(|m| m.get(a).cloned()))
    }
    fn set(&self, a: &str, s: &str) -> Result<()> {
        if let Ok(mut m) = self.0.lock() {
            m.insert(a.into(), s.into());
        }
        Ok(())
    }
    fn delete(&self, a: &str) -> Result<()> {
        if let Ok(mut m) = self.0.lock() {
            m.remove(a);
        }
        Ok(())
    }
}

/// Debug builds only: `CLAUDE_SYNC_DEV_TOKEN` signs in with that token, held in memory
/// (never the keychain, never the token file; sign-out leaves them alone), as v1's app.
pub fn dev_token() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("CLAUDE_SYNC_DEV_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Sign in with the dev token when nobody is signed in; a token GitHub refuses is logged, not used.
pub fn dev_sign_in(core: &mut Core, token: Option<String>) -> bool {
    let Some(t) = token.filter(|_| core.snapshot().login.is_none()) else {
        return false;
    };
    let ok = core.recover_sign_in(&t);
    if !ok {
        tracing::warn!("CLAUDE_SYNC_DEV_TOKEN was not accepted");
    }
    ok
}

/// `gh auth token` (argument list, no shell, no console window): the token of the `gh`
/// sign-in, or `None` when gh is missing or signed out. As v1.
pub fn gh_token() -> Option<String> {
    let mut cmd = std::process::Command::new("gh");
    cmd.args(["auth", "token"]);
    let out = claude_sync_fs::quiet(&mut cmd).output().ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !t.is_empty()).then_some(t)
}

/// A sign-in GitHub refused (never: no sign-in at all): take the `gh` sign-in if GitHub accepts it.
pub fn recover(core: &mut Core, gh: impl FnOnce() -> Option<String>) -> bool {
    core.token_refused() && gh().is_some_and(|t| core.recover_sign_in(&t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_sync_github::secrets::{delete, load, save};
    use std::cell::Cell;

    #[test]
    fn after_logout_gh_is_never_asked() {
        let h = tempfile::tempdir().unwrap();
        let ctx = crate::Ctx {
            home: Some(h.path().to_path_buf()),
            ..crate::Ctx::cli()
        };
        let mut core = Core::open(crate::run::env(&ctx).unwrap()).unwrap();
        core.handle(claude_sync_actions::Command::SignOut).unwrap();
        let asked = Cell::new(false);
        let fake_gh = || {
            asked.set(true);
            Some("ghtok".to_string())
        };
        assert!(!recover(&mut core, fake_gh));
        assert!(!asked.get(), "no token at all: gh stays unasked");
        assert!(core.snapshot().login.is_none());
    }

    #[test]
    fn round_trips_and_is_private() {
        let d = tempfile::tempdir().unwrap();
        let s = FileSecrets(d.path().join("state").join("token"));
        assert!(load(&s).unwrap().is_none());
        save(&s, "ann", &Tokens::plain("t0")).unwrap();
        let (login, t) = load(&s).unwrap().unwrap();
        assert_eq!((login.as_str(), t.access_token.as_str()), ("ann", "t0"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&s.0).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o600);
        }
        save(&s, "bob", &Tokens::plain("t1")).unwrap();
        assert_eq!(load(&s).unwrap().unwrap().0, "bob");
        delete(&s).unwrap();
        assert!(!s.0.exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_dev_token_signs_in_from_memory_and_writes_no_file() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .and(header("authorization", "Bearer gho_dev"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"login": "ann", "id": 7})),
            )
            .mount(&server)
            .await;
        tokio::task::spawn_blocking(move || {
            let h = tempfile::tempdir().unwrap();
            let mut cfg = claude_sync_github::Config::github();
            cfg.api = server.uri();
            let ctx = crate::Ctx {
                home: Some(h.path().to_path_buf()),
                cfg: Some(cfg),
                // a keychain that must never be touched
                secrets: Some(claude_sync_actions::Secrets(std::sync::Arc::new(Untouched))),
                ..crate::Ctx::cli()
            };
            let env = crate::run::env_in_dev(&ctx, true).unwrap();
            let mut core = Core::open(env).unwrap();
            assert!(core.snapshot().login.is_none());
            assert!(!dev_sign_in(&mut core, None));
            assert!(dev_sign_in(&mut core, Some("gho_dev".into())));
            assert_eq!(core.snapshot().login.as_deref(), Some("ann"));
            assert!(!h.path().join(".claude-sync/token").exists());
            // a second call with someone signed in does nothing
            assert!(!dev_sign_in(&mut core, Some("gho_dev".into())));
        })
        .await
        .unwrap();
    }

    struct Untouched;
    impl SecretStore for Untouched {
        fn get(&self, _: &str) -> Result<Option<String>> {
            Err(claude_sync_github::Error::Invalid(
                "keychain touched".into(),
            ))
        }
        fn set(&self, _: &str, _: &str) -> Result<()> {
            Err(claude_sync_github::Error::Invalid(
                "keychain touched".into(),
            ))
        }
        fn delete(&self, _: &str) -> Result<()> {
            Err(claude_sync_github::Error::Invalid(
                "keychain touched".into(),
            ))
        }
    }
}
