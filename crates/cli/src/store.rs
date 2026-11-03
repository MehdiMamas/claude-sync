//! The sign-in on a machine without a keychain: `~/.claude-sync/token`, readable by the
//! owner only (mode 0600; Windows keeps it in the profile folder, which only the user can
//! read). Written whole through a temp file, so a crash never leaves half a token.
use claude_sync_core::github::Tokens;
use claude_sync_core::Paths;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    pub login: String,
    pub tokens: Tokens,
}

pub fn path(paths: &Paths) -> PathBuf {
    paths.state_dir.join("token")
}

/// `None`: no file. A file that cannot be read is an error, never a silent sign-out.
pub fn load(paths: &Paths) -> Result<Option<Saved>, String> {
    match std::fs::read(path(paths)) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| format!("{} is not valid: {e}", path(paths).display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path(paths).display())),
    }
}

pub fn save(paths: &Paths, saved: &Saved) -> Result<(), String> {
    let dest = path(paths);
    let err = |e: std::io::Error| format!("cannot write {}: {e}", dest.display());
    std::fs::create_dir_all(&paths.state_dir).map_err(err)?;
    let tmp = paths
        .state_dir
        .join(format!(".token.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(saved).map_err(|e| e.to_string())?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).map_err(err)?;
    let written = f.write_all(&bytes).and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = written.and_then(|_| std::fs::rename(&tmp, &dest)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err(e));
    }
    Ok(())
}

pub fn delete(paths: &Paths) -> Result<(), String> {
    match std::fs::remove_file(path(paths)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove {}: {e}", path(paths).display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(d: &std::path::Path) -> Paths {
        Paths {
            claude_home: d.join("claude"),
            state_dir: d.join("state"),
        }
    }

    #[test]
    fn saves_privately_and_reads_back() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        assert!(load(&p).unwrap().is_none());
        let saved = Saved {
            login: "me".into(),
            tokens: Tokens::plain("ghu_secret"),
        };
        save(&p, &saved).unwrap();
        let back = load(&p).unwrap().unwrap();
        assert_eq!(back.login, "me");
        assert_eq!(back.tokens.access_token, "ghu_secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path(&p)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Saving again replaces it, and no temp file is left.
        save(&p, &saved).unwrap();
        assert_eq!(std::fs::read_dir(&p.state_dir).unwrap().count(), 1);
        delete(&p).unwrap();
        delete(&p).unwrap();
        assert!(load(&p).unwrap().is_none());
    }

    #[test]
    fn a_broken_file_is_an_error_not_a_sign_out() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        std::fs::create_dir_all(&p.state_dir).unwrap();
        std::fs::write(path(&p), "{nope").unwrap();
        assert!(load(&p).unwrap_err().contains("not valid"));
    }
}
