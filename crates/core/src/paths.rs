//! Where things live on disk (§10).
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/.claude` (env `CLAUDE_HOME` overrides; tests always set it).
    pub claude_home: PathBuf,
    /// `~/.claude-sync`, or `~/.claude-sync/app` when `~/.claude-sync/.git` exists (a v0 clone).
    /// Env `CLAUDE_SYNC_HOME` overrides.
    pub state_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> crate::Result<Self> {
        todo!("§10")
    }
    pub fn group_dir(&self, g: &crate::GroupId) -> PathBuf {
        self.state_dir.join("groups").join(g.dir_name())
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.state_dir.join("logs")
    }
}
