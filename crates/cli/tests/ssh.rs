//! `claude-sync-cli ssh` round trip. Only the choice is set: the sync pass (which touches
//! `~/.ssh`) never runs here, and the state folders are temporary.
use std::process::Command;

fn run(home: &std::path::Path, args: &[&str]) -> (i32, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_claude-sync-cli"))
        .args(args)
        .arg("--json")
        .env("CLAUDE_HOME", home.join("claude"))
        .env("CLAUDE_SYNC_HOME", home.join("state"))
        .output()
        .unwrap();
    let v = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    (out.status.code().unwrap_or(-1), v)
}

#[test]
fn enable_status_disable_round_trip() {
    let t = tempfile::tempdir().unwrap();
    let (code, v) = run(t.path(), &["ssh", "status"]);
    assert_eq!((code, v["on"].as_bool()), (0, Some(false)));
    let (code, v) = run(t.path(), &["ssh", "enable"]);
    assert_eq!((code, v["on"].as_bool()), (0, Some(true)));
    let (code, v) = run(t.path(), &["ssh", "status"]);
    assert_eq!((code, v["on"].as_bool()), (0, Some(true)));
    let (code, v) = run(t.path(), &["ssh", "alias", "tower"]);
    assert_eq!((code, v["alias"].as_str()), (0, Some("tower")));
    let (code, _) = run(t.path(), &["ssh", "alias", "bad name"]);
    assert_eq!(code, 1);
    let (code, v) = run(t.path(), &["ssh", "disable"]);
    assert_eq!((code, v["on"].as_bool()), (0, Some(false)));
    assert!(v["peers"].as_array().unwrap().is_empty());
}
