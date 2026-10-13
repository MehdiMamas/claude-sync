//! Apply layer: group files -> ~/.claude, with a manifest (§7, issue #16).
//! Settings go through `crate::settings` (§8).
use crate::{FileSet, GroupId, Paths};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Collision {
    /// A file the user owns is in the way.
    UserFile { path: String },
    /// Another group, earlier in the order, already ships this path.
    OtherGroup { path: String, owner: GroupId },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub written: Vec<String>,
    pub removed: Vec<String>,
    pub collisions: Vec<Collision>,
    /// Settings keys not applied and why (§8).
    pub settings_skipped: Vec<String>,
}

/// Make `~/.claude` reflect `files` for `group`. `order` is the joined-group order (§7 collisions).
/// Idempotent: a second call with the same input writes nothing.
pub fn apply_group(
    _paths: &Paths,
    _group: &GroupId,
    _order: &[GroupId],
    _files: &FileSet,
) -> crate::Result<ApplyReport> {
    todo!()
}

/// The group's files as they are on this machine right now, read back from `~/.claude`
/// (the "local" side of the three-way merge). Paths set to "keep local" are left out.
pub fn local_files(_paths: &Paths, _group: &GroupId) -> crate::Result<FileSet> {
    todo!()
}

/// Remove everything this group put on the machine (files, import line, owned settings), with a backup.
pub fn remove_group(_paths: &Paths, _group: &GroupId) -> crate::Result<ApplyReport> {
    todo!()
}

/// Stop managing `path` on this device (the "Keep local" action, §7).
pub fn keep_local(_paths: &Paths, _group: &GroupId, _path: &str) -> crate::Result<()> {
    todo!()
}
