//! `claude-sync.json` (§4, issue #14).
use serde::{Deserialize, Serialize};

pub const FILE: &str = "claude-sync.json";
pub const SCHEMA: u32 = 1;
pub const TOPIC: &str = "claude-sync-group";
pub const DEFAULT_REPO: &str = "claude-sync-data";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncFlags {
    pub settings: bool,
    pub claude_md: bool,
    pub skills: bool,
    pub agents: bool,
    pub commands: bool,
    pub output_styles: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupFile {
    pub schema: u32,
    pub name: String,
    pub members: Vec<String>,
    pub sync: SyncFlags,
    /// Unknown keys, kept on rewrite.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// True when `path` (repo-relative, forward slashes) is something a group syncs (§4).
pub fn is_allowlisted(_path: &str) -> bool {
    todo!()
}
