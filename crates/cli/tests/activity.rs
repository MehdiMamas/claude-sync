//! `claude-sync-cli activity` round trip. Only the choice is set; the state folders are
//! temporary.
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
    let (code, v) = run(t.path(), &["activity", "status"]);
    assert_eq!((code, v["share"].as_bool()), (0, Some(false)));
    let (code, v) = run(t.path(), &["activity", "enable"]);
    assert_eq!((code, v["share"].as_bool()), (0, Some(true)));
    let (code, v) = run(t.path(), &["activity", "status"]);
    assert_eq!((code, v["share"].as_bool()), (0, Some(true)));
    // The value on this machine goes at once when sharing is turned off.
    let file = t.path().join("state").join("activity.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}").unwrap();
    let (code, v) = run(t.path(), &["activity", "disable"]);
    assert_eq!((code, v["share"].as_bool()), (0, Some(false)));
    assert!(!file.exists());
    let (code, _) = run(t.path(), &["activity", "watch"]);
    assert_eq!(code, 2);
}
