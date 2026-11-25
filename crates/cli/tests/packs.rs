//! `claude-sync-cli packs`: exit codes and output of the commands that need no sign-in. The
//! state folders are temporary; the group's packs are placed with the core, as a sync does.
use claude_sync_core::state::AppState;
use claude_sync_core::{FileSet, GroupId, Paths};
use std::process::Command;

fn exit(code: i32) -> serde_json::Value {
    if cfg!(windows) {
        serde_json::json!(["cmd", "/c", format!("exit {code}")])
    } else {
        serde_json::json!(["sh", "-c", format!("exit {code}")])
    }
}

fn pack(id: &str, code: i32) -> serde_json::Value {
    serde_json::json!({
        "schema": 1, "id": id, "title": format!("Pack {id}"), "summary": "Does it.",
        "setup": "setup.md", "check": {"run": exit(code), "timeoutSeconds": 20}
    })
}

/// A machine that joined `acme/team`, whose repo has the packs `good` (check passes) and
/// `bad` (check fails).
fn machine() -> (tempfile::TempDir, Paths) {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let paths = Paths {
        claude_home: home.join(".claude"),
        state_dir: home.join(".claude-sync"),
    };
    let g = GroupId::new("acme", "team");
    let st = AppState {
        groups: vec![g.clone()],
        ..Default::default()
    };
    st.save(&paths).unwrap();
    let mut files = FileSet::new();
    for (id, code) in [("good", 0), ("bad", 3)] {
        files.insert(
            format!("packs/{id}/pack.json"),
            pack(id, code).to_string().into_bytes(),
        );
        files.insert(format!("packs/{id}/setup.md"), b"Set up.\n".to_vec());
    }
    let p = claude_sync_core::packs::Prepare {
        home: &home,
        host: Some(false),
        now_ms: 1,
    };
    claude_sync_core::packs::prepare(&paths, &g, &files, &p).unwrap();
    (t, paths)
}

fn run(paths: &Paths, args: &[&str]) -> (i32, String, String) {
    let home = paths.state_dir.parent().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_claude-sync-cli"))
        .arg("packs")
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("CLAUDE_HOME", &paths.claude_home)
        .env("CLAUDE_SYNC_HOME", &paths.state_dir)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn list_prints_the_packs_and_json() {
    let (_t, paths) = machine();
    let (code, out, _) = run(&paths, &["list"]);
    assert_eq!(code, 0);
    assert!(out.contains("acme/team"), "{out}");
    assert!(
        out.contains("good") && out.contains("bad") && out.contains("not set up"),
        "{out}"
    );
    let (code, out, _) = run(&paths, &["--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["packs"].as_array().unwrap().len(), 2);
}

#[test]
fn exit_codes_are_0_for_done_1_for_failed_2_for_wrong_usage() {
    let (_t, paths) = machine();
    for bad in [
        &["frobnicate"][..],
        &["on"],
        &["check", "Not-An-Id"],
        &["list", "--nope"],
    ] {
        assert_eq!(run(&paths, bad).0, 2, "{bad:?}");
    }
    let (code, _, err) = run(&paths, &["check", "ghost"]);
    assert_eq!(code, 1);
    assert!(err.contains("no pack"), "{err}");
    let (code, out, _) = run(&paths, &["check", "good"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("check passed"));
    assert!(out.contains("claude-sync packs on good"));
    let (code, out, _) = run(&paths, &["check", "bad"]);
    assert_eq!(code, 1);
    assert!(out.contains("check failed (exit 3)"), "{out}");
    let (code, _, err) = run(&paths, &["off", "good"]);
    assert_eq!(code, 1, "a pack that is not on cannot be turned off");
    assert!(err.contains("is not on"), "{err}");
    let (code, _, err) = run(&paths, &["list", "--group", "other/repo"]);
    assert_eq!(code, 1);
    assert!(err.contains("not in other/repo"), "{err}");
    // The checks were recorded.
    let (_, out, _) = run(&paths, &["list"]);
    assert!(out.contains("ready to turn on"), "{out}");
    assert!(out.contains("last check failed"), "{out}");
}
