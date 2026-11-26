//! The headless sign-in: `{login, tokens}` in `<state>/token`, mode 0600, the same JSON as v1.
//! It plugs into the actions as a `SecretStore`, so the engine never knows it is not a keychain.
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

#[cfg(test)]
mod tests {
    use super::*;
    use claude_sync_github::secrets::{delete, load, save};

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
}
