//! Where things live on disk (§10).
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/.claude` (env `CLAUDE_HOME` overrides; tests always set it).
    pub claude_home: PathBuf,
    /// `~/.claude-sync`, or `~/.claude-sync/app` when `~/.claude-sync/.git` exists (a v0 clone
    /// that `migrate` could not move; the app normally migrates it at start).
    /// Env `CLAUDE_SYNC_HOME` overrides.
    pub state_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> crate::Result<Self> {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = std::env::home_dir();
        Self::resolve(var("CLAUDE_HOME"), var("CLAUDE_SYNC_HOME"), home.as_deref())
    }

    /// `from_env` without the environment: overrides first, then `home`.
    pub fn resolve(
        claude_home: Option<PathBuf>,
        sync_home: Option<PathBuf>,
        home: Option<&Path>,
    ) -> crate::Result<Self> {
        let need_home =
            || home.ok_or_else(|| crate::Error::Invalid("cannot find the home directory".into()));
        let claude_home = match claude_home {
            Some(p) => p,
            None => need_home()?.join(".claude"),
        };
        let state_dir = match sync_home {
            Some(p) => p,
            None => {
                let dir = need_home()?.join(".claude-sync");
                if dir.join(".git").exists() {
                    dir.join("app")
                } else {
                    dir
                }
            }
        };
        Ok(Self {
            claude_home,
            state_dir,
        })
    }

    pub fn group_dir(&self, g: &crate::GroupId) -> PathBuf {
        self.state_dir.join("groups").join(g.dir_name())
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.state_dir.join("logs")
    }
    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }
    /// Notification history (`notifications.rs`).
    pub fn notifications_file(&self) -> PathBuf {
        self.state_dir.join("notifications.json")
    }
    /// Counts of GitHub requests (`github/requests.rs`).
    pub fn requests_file(&self) -> PathBuf {
        self.state_dir.join("requests.json")
    }
    /// Base snapshot of a group (§6).
    pub fn base_dir(&self, g: &crate::GroupId) -> PathBuf {
        self.group_dir(g).join("base")
    }
    pub fn conflicts_dir(&self, g: &crate::GroupId) -> PathBuf {
        self.state_dir.join("conflicts").join(g.dir_name())
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.state_dir.join("backups")
    }
    /// Plugin marketplaces bundled in group repos (`plugins/<name>/` -> `marketplaces/<name>/`).
    pub fn marketplaces_dir(&self) -> PathBuf {
        self.state_dir.join(MARKETPLACES)
    }
}

/// The folder under `state_dir` that holds bundled plugin marketplaces.
pub const MARKETPLACES: &str = "marketplaces";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_under_home() {
        let home = tempfile::tempdir().unwrap();
        let p = Paths::resolve(None, None, Some(home.path())).unwrap();
        assert_eq!(p.claude_home, home.path().join(".claude"));
        assert_eq!(p.state_dir, home.path().join(".claude-sync"));
    }

    #[test]
    fn v0_clone_moves_state_to_app() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".claude-sync/.git")).unwrap();
        let p = Paths::resolve(None, None, Some(home.path())).unwrap();
        assert_eq!(p.state_dir, home.path().join(".claude-sync").join("app"));
    }

    #[test]
    fn overrides_win() {
        let p = Paths::resolve(Some("/c".into()), Some("/s".into()), None).unwrap();
        assert_eq!(p.claude_home, PathBuf::from("/c"));
        assert_eq!(p.state_dir, PathBuf::from("/s"));
        assert!(Paths::resolve(None, Some("/s".into()), None).is_err());
    }
}
