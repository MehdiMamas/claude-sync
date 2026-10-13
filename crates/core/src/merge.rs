//! Three-way merge (§6, issue #15). Pure functions, no I/O.
use crate::FileSet;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub path: String,
    /// The local version that lost (remote wins). `None` = local had deleted it.
    pub local: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeResult {
    /// What the local side should become.
    pub merged: FileSet,
    /// What to commit to the remote (`None` = delete). Empty = nothing to push.
    pub to_push: BTreeMap<String, Option<Vec<u8>>>,
    pub conflicts: Vec<Conflict>,
}

pub fn three_way(_base: &FileSet, _local: &FileSet, _remote: &FileSet) -> MergeResult {
    todo!()
}
