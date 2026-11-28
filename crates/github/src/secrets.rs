//! Where the sign-in lives. The keychain (service `claude-sync`): account `@current` holds the
//! login (`@` cannot occur in a GitHub login), account `<login>` holds that login's tokens as
//! the bare access token (the 0.1.1 layout) or as JSON; both load (v2-Cutover). The headless
//! CLI keeps `{login, tokens}` in `~/.claude-sync/token`, mode 0600. Nothing here logs a token.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SERVICE: &str = "claude-sync";
const CURRENT: &str = "@current";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_expires_at: Option<i64>,
}

// Never print a secret, even in a debug dump.
impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tokens(..)")
    }
}

impl Tokens {
    pub fn plain(access_token: &str) -> Self {
        Tokens {
            access_token: access_token.into(),
            refresh_token: None,
            expires_at: None,
            refresh_token_expires_at: None,
        }
    }

    /// Bare token when it neither refreshes nor expires, else JSON.
    pub fn to_secret(&self) -> String {
        if self.refresh_token.is_none() && self.expires_at.is_none() {
            return self.access_token.clone();
        }
        serde_json::to_string(self).unwrap_or_else(|_| self.access_token.clone())
    }

    /// JSON when it starts with `{` and parses, else the whole text is the token.
    pub fn from_secret(secret: &str) -> Self {
        let s = secret.trim();
        if s.starts_with('{') {
            if let Ok(t) = serde_json::from_str(s) {
                return t;
            }
        }
        Tokens::plain(s)
    }

    /// A refresh token and a known expiry within five minutes.
    pub fn refresh_due(&self, now: i64) -> bool {
        matches!((&self.refresh_token, self.expires_at), (Some(_), Some(e)) if now + 300 >= e)
    }
}

/// Where secrets go: the OS keychain in the app, a map in tests.
pub trait SecretStore {
    fn get(&self, account: &str) -> Result<Option<String>>;
    fn set(&self, account: &str, secret: &str) -> Result<()>;
    fn delete(&self, account: &str) -> Result<()>;
}

/// `(login, tokens)` of the signed-in account; a keychain error is an error, not signed out.
pub fn load(store: &impl SecretStore) -> Result<Option<(String, Tokens)>> {
    let Some(login) = store.get(CURRENT)? else {
        return Ok(None);
    };
    Ok(store.get(&login)?.map(|s| (login, Tokens::from_secret(&s))))
}

/// One write holds the whole token set, so a refresh never leaves a mismatched pair.
pub fn save(store: &impl SecretStore, login: &str, tokens: &Tokens) -> Result<()> {
    if let Some(old) = store.get(CURRENT)? {
        if old != login {
            store.delete(&old)?;
        }
    }
    store.set(login, &tokens.to_secret())?;
    store.set(CURRENT, login)
}

pub fn delete(store: &impl SecretStore) -> Result<()> {
    if let Some(login) = store.get(CURRENT)? {
        store.delete(&login)?;
    }
    store.delete(CURRENT)
}

#[cfg(feature = "keychain")]
pub struct Keychain;

#[cfg(feature = "keychain")]
impl Keychain {
    fn entry(account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, account).map_err(|e| Error::Invalid(format!("keychain: {e}")))
    }
}

#[cfg(feature = "keychain")]
impl SecretStore for Keychain {
    fn get(&self, account: &str) -> Result<Option<String>> {
        match Self::entry(account)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Invalid(format!("keychain: {e}"))),
        }
    }
    fn set(&self, account: &str, secret: &str) -> Result<()> {
        Self::entry(account)?
            .set_password(secret)
            .map_err(|e| Error::Invalid(format!("keychain: {e}")))
    }
    fn delete(&self, account: &str) -> Result<()> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Invalid(format!("keychain: {e}"))),
        }
    }
}

/// The CLI's file (`~/.claude-sync/token`), the same JSON as v1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    pub login: String,
    pub tokens: Tokens,
}

/// `None`: no file. An unreadable file is an error, never a silent sign-out.
pub fn file_load(path: &Path) -> Result<Option<Saved>> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| Error::Invalid(format!("{} is not valid: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Invalid(format!(
            "cannot read {}: {e}",
            path.display()
        ))),
    }
}

/// Whole file through a temp file, mode 0600, so a crash never leaves half a token.
pub fn file_save(path: &Path, saved: &Saved) -> Result<()> {
    use std::io::Write;
    let err = |e: std::io::Error| Error::Invalid(format!("cannot write {}: {e}", path.display()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(saved).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).map_err(err)?;
    let done = f.write_all(&bytes).and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = done.and_then(|_| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err(e));
    }
    Ok(())
}

pub fn file_delete(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Invalid(format!(
            "cannot remove {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Mem(RefCell<HashMap<String, String>>);
    impl SecretStore for Mem {
        fn get(&self, a: &str) -> Result<Option<String>> {
            Ok(self.0.borrow().get(a).cloned())
        }
        fn set(&self, a: &str, s: &str) -> Result<()> {
            self.0.borrow_mut().insert(a.into(), s.into());
            Ok(())
        }
        fn delete(&self, a: &str) -> Result<()> {
            self.0.borrow_mut().remove(a);
            Ok(())
        }
    }

    #[test]
    fn reads_both_v1_layouts() {
        let m = Mem::default();
        m.set(CURRENT, "octo").unwrap();
        m.set("octo", "gho_bare\n").unwrap();
        let (login, t) = load(&m).unwrap().unwrap();
        assert_eq!(
            (login.as_str(), t.access_token.as_str()),
            ("octo", "gho_bare")
        );
        assert_eq!(t.refresh_token, None);
        m.set(
            "octo",
            r#"{"access_token":"a","refresh_token":"r","expires_at":5}"#,
        )
        .unwrap();
        let t = load(&m).unwrap().unwrap().1;
        assert_eq!(t.refresh_token.as_deref(), Some("r"));
        assert_eq!(t.refresh_token_expires_at, None);
    }

    #[test]
    fn writes_the_v1_layouts_and_replaces_the_old_login() {
        let m = Mem::default();
        save(&m, "a", &Tokens::plain("t1")).unwrap();
        assert_eq!(m.get("a").unwrap().as_deref(), Some("t1"));
        let mut t = Tokens::plain("t2");
        t.refresh_token = Some("r".into());
        save(&m, "b", &t).unwrap();
        assert_eq!(m.get("a").unwrap(), None);
        assert!(m.get("b").unwrap().unwrap().starts_with('{'));
        delete(&m).unwrap();
        assert!(load(&m).unwrap().is_none());
    }

    #[test]
    fn refresh_is_due_five_minutes_ahead_only_with_a_refresh_token() {
        let mut t = Tokens::plain("a");
        t.expires_at = Some(1000);
        assert!(!t.refresh_due(1000));
        t.refresh_token = Some("r".into());
        assert!(!t.refresh_due(699));
        assert!(t.refresh_due(700));
    }

    #[test]
    fn token_file_is_private_and_round_trips() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state").join("token");
        assert!(file_load(&p).unwrap().is_none());
        let s = Saved {
            login: "octo".into(),
            tokens: Tokens::plain("gho_x"),
        };
        file_save(&p, &s).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(file_load(&p).unwrap().unwrap().tokens.access_token, "gho_x");
        std::fs::write(&p, "nope").unwrap();
        assert!(file_load(&p).is_err());
        file_delete(&p).unwrap();
        file_delete(&p).unwrap();
    }
}
