//! claude-sync core: GitHub client, group format, sync engine, apply layer.
//! Design: docs/architecture.md. Section numbers below (§N) refer to it.

pub mod apply;
pub mod engine;
pub mod error;
pub mod github;
pub mod group;
pub mod logging;
pub mod members;
pub mod merge;
pub mod paths;
pub mod remote;
pub mod secrets;
pub mod settings;
pub mod state;
pub mod watch;

pub use engine::Engine;
pub use error::{Error, Result};
pub use paths::Paths;

use std::collections::BTreeMap;

/// Files of one group, keyed by repo-relative path with forward slashes
/// (`skills/review/SKILL.md`). Only allowlisted paths (§4) are ever put in one.
pub type FileSet = BTreeMap<String, Vec<u8>>;

/// `owner/repo` of a group repo. Display and parse use `owner/repo`;
/// on disk it is `owner__repo` (§10).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct GroupId {
    pub owner: String,
    pub repo: String,
}

impl GroupId {
    pub fn new(owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        let (o, r) = s.split_once('/')?;
        (!o.is_empty() && !r.is_empty() && !r.contains('/')).then(|| Self::new(o, r))
    }
    pub fn dir_name(&self) -> String {
        format!("{}__{}", self.owner, self.repo)
    }
}

impl std::fmt::Display for GroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}
