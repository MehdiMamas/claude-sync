//! A group repo that was renamed or transferred (§10): every place keyed by its `owner/repo`
//! moves to the new name. GitHub answers `301` for the old name, so the app only learns of it
//! from the `full_name` the repo reports.
//!
//! The steps run in a fixed order and each is safe to run again: folders first, the other
//! records next, `state.json` last. A run that stops half way (power loss) is redone, and
//! finished, by the next sync, which still sees the old name in `state.json`.
use crate::jobs::JobsState;
use crate::state::AppState;
use crate::{apply, notifications, plugin_install, GroupId, Paths};
use std::path::Path;

/// Move everything this device keeps for `old` to `new`. The joined-groups list keeps the
/// group at its place. Nothing is deleted: a folder already at the new place stays, and the
/// old one is left beside it.
pub fn migrate(paths: &Paths, old: &GroupId, new: &GroupId) -> crate::Result<()> {
    if old == new {
        return Ok(());
    }
    move_dir(&paths.group_dir(old), &paths.group_dir(new))?;
    move_dir(&paths.conflicts_dir(old), &paths.conflicts_dir(new))?;
    apply::rename_group(paths, old, new)?;
    let mut jobs = JobsState::load(paths)?;
    if jobs.rename_group(old, new) {
        jobs.save(paths)?;
    }
    crate::choices::rename_group(paths, old, new)?;
    plugin_install::rename_group(paths, old, new)?;
    notifications::rename_group(paths, old, new)?;
    let mut state = AppState::load(paths)?;
    let had_new = state.groups.contains(new);
    let mut seen = false;
    state.groups = state
        .groups
        .into_iter()
        .filter_map(|g| {
            if g != *old {
                return Some(g);
            }
            // Already joined under the new name: that entry stays, this one goes.
            (!had_new && !std::mem::replace(&mut seen, true)).then(|| new.clone())
        })
        .collect();
    state.save(paths)
}

/// Rename `from` to `to` unless `to` exists or `from` is gone (a finished earlier run).
fn move_dir(from: &Path, to: &Path) -> crate::Result<()> {
    if !from.exists() || (to.exists() && !only_case_differs(from, to)) {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, to)?;
    Ok(())
}

/// On a case-insensitive disk `to` "exists" when only the case of the name changed.
fn only_case_differs(from: &Path, to: &Path) -> bool {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    name(from) != name(to)
        && name(from).map(|n| n.to_lowercase()) == name(to).map(|n| n.to_lowercase())
}
