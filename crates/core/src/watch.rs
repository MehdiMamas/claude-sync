//! Local edits to managed files (§7 "Local edits", issue #18).
use crate::GroupId;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEdit {
    pub group: GroupId,
    pub path: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditAction {
    Share,
    KeepLocal,
    Undo,
}

/// Managed files whose content differs from the manifest.
pub fn local_edits(_paths: &crate::Paths) -> crate::Result<Vec<LocalEdit>> {
    todo!()
}
