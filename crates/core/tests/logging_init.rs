//! `logging::init` sets the process-wide subscriber, so it gets its own test binary.
use claude_sync_core::{logging, Paths};

#[test]
fn init_writes_a_redacted_log_and_refuses_a_second_start() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths {
        claude_home: tmp.path().join(".claude"),
        state_dir: tmp.path().join(".claude-sync"),
    };
    let token = format!("sk-ant-{}", "x".repeat(24));
    let guard = logging::init(&paths).unwrap();
    tracing::info!("signed in with {token}");
    assert!(matches!(
        logging::init(&paths),
        Err(claude_sync_core::Error::Invalid(_))
    ));
    drop(guard);
    let text = logging::tail(&paths, 64 * 1024).unwrap();
    assert!(text.contains("signed in with [redacted:sk-ant]"), "{text}");
    assert!(!text.contains(&token));
}
