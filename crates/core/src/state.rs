//! `state.json` (§10).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub login: Option<String>,
    pub device_name: Option<String>,
    /// Joined groups in priority order (first wins collisions).
    pub groups: Vec<crate::GroupId>,
    pub poll_seconds: Option<u64>,
    pub paused: bool,
}

impl AppState {
    pub fn load(_paths: &crate::Paths) -> crate::Result<Self> {
        todo!()
    }
    pub fn save(&self, _paths: &crate::Paths) -> crate::Result<()> {
        todo!("write to temp, then rename")
    }
}
