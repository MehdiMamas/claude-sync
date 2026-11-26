//! CLAUDE.md review (Parity 37, 38, 82): the engine's [`ReviewState`] per group, kept in the
//! store as JSON in the v1 shape (format 2). The sync pass asks `gate` for the text it may
//! import; the commands here preview, accept, decline, drop, edit and reset.
use claude_sync_engine::review::ReviewState;
use claude_sync_engine::traits::Store;
use claude_sync_store::Db;

fn key(repo: &str) -> String {
    format!("review/{repo}")
}

pub(crate) fn load(db: &Db, repo: &str) -> Option<ReviewState> {
    Store::read(db, &key(repo))
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_slice(&b).ok())
}
