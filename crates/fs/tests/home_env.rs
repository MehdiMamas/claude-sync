//! `CLAUDE_HOME` and `CLAUDE_SYNC_HOME` move the folders, as in v1 (one test: it sets env vars).
use claude_sync_fs::Home;
use std::path::Path;

#[test]
fn overrides_win_and_empty_counts_as_unset() {
    let h = Path::new("/home/ann");
    let plain = Home::from_env(h);
    assert_eq!(plain.claude, h.join(".claude"));
    assert_eq!(plain.state, h.join(".claude-sync"));
    std::env::set_var("CLAUDE_HOME", "/x/claude");
    std::env::set_var("CLAUDE_SYNC_HOME", "/x/state");
    let moved = Home::from_env(h);
    assert_eq!(moved.claude, Path::new("/x/claude"));
    assert_eq!(moved.state, Path::new("/x/state"));
    assert_eq!(moved.ssh, h.join(".ssh"));
    std::env::set_var("CLAUDE_HOME", "");
    assert_eq!(Home::from_env(h).claude, h.join(".claude"));
    // `under` never reads the environment
    assert_eq!(Home::under(h).state, h.join(".claude-sync"));
}
