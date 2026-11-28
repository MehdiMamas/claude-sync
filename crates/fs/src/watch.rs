//! Watches the managed paths and calls back once per burst of changes (debounced), so a local
//! edit is pushed or applied without waiting for the next tick. Our own atomic temp files are
//! ignored. v2-Parity rows covered: 24 (local change pass).
use crate::disk::Home;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::path::PathBuf;
use std::time::Duration;

/// Keeping this alive keeps the watch alive.
pub struct Watch(#[allow(dead_code)] Debouncer<RecommendedWatcher>);

/// The folders and files that matter: `~/.claude` (CLAUDE.md, settings.json, the managed
/// folders, projects memory, personal state) and the marketplaces.
pub fn managed_paths(h: &Home) -> Vec<(PathBuf, RecursiveMode)> {
    let mut v = vec![(h.claude.clone(), RecursiveMode::NonRecursive)];
    for d in [
        "skills",
        "agents",
        "commands",
        "output-styles",
        "personal-state",
        "projects",
    ] {
        v.push((h.claude.join(d), RecursiveMode::Recursive));
    }
    v.push((h.state.join("marketplaces"), RecursiveMode::Recursive));
    v
}

/// Start watching. A folder that does not exist yet is skipped (the next tick picks up its
/// files); `on_change` runs on the watcher's thread, so it should only signal.
pub fn watch(
    h: &Home,
    debounce: Duration,
    on_change: impl Fn() + Send + 'static,
) -> Result<Watch, String> {
    let mut d = new_debouncer(debounce, move |r: DebounceEventResult| {
        if let Ok(events) = r {
            if events
                .iter()
                // a folder's own event (macOS replays a just-made one) is no edit; its files report
                .any(|e| {
                    !e.path.to_string_lossy().contains(".claude-sync-tmp-") && !e.path.is_dir()
                })
            {
                on_change();
            }
        }
    })
    .map_err(|e| e.to_string())?;
    for (p, mode) in managed_paths(h) {
        if p.exists() {
            d.watcher().watch(&p, mode).map_err(|e| e.to_string())?;
        }
    }
    Ok(Watch(d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn a_change_in_a_managed_folder_calls_back_once_and_temp_files_do_not() {
        let t = tempfile::tempdir().unwrap();
        let h = Home::under(t.path());
        std::fs::create_dir_all(h.claude.join("skills/a")).unwrap();
        let (tx, rx) = channel();
        let _w = watch(&h, Duration::from_millis(100), move || {
            let _ = tx.send(());
        })
        .unwrap();
        std::fs::write(h.claude.join("skills/a/.SKILL.md.claude-sync-tmp-1"), "x").unwrap();
        assert!(
            rx.recv_timeout(Duration::from_millis(600)).is_err(),
            "temp file must not trigger"
        );
        std::fs::write(h.claude.join("skills/a/SKILL.md"), "x").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }
}
