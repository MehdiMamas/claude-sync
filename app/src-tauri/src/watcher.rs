//! Local file watcher.
//!
//! TODO(core watch): replace this stub with `claude_sync_core::watch::spawn_watcher(paths, on_change)`
//! once the apply layer adds it (debounced 2 s, §7 "Local edits"). Until then nothing is
//! watched and local edits are picked up by the sync loop on every tick (`check_local_edits`).
use std::path::PathBuf;

/// Keeps the watcher alive while it is managed by the app.
pub struct WatcherHandle;

pub fn spawn_watcher(
    paths: Vec<PathBuf>,
    _on_change: impl Fn() + Send + Sync + 'static,
) -> Option<WatcherHandle> {
    tracing::info!(
        "file watcher not available yet; {} paths checked on each sync instead",
        paths.len()
    );
    None
}
