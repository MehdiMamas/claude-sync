//! Exit codes (0 done, 1 failed, 2 wrong usage) and the `status --json` keys, on a temp home.
use claude_sync_cli::{command_names, run_status, Ctx};

fn go(home: &std::path::Path, args: &[&str]) -> u8 {
    let ctx = Ctx {
        home: Some(home.to_path_buf()),
        ..Ctx::cli()
    };
    let mut v = vec!["claude-sync-cli".to_string()];
    v.extend(args.iter().map(|s| s.to_string()));
    run_status(v, &ctx)
}

#[test]
fn exit_codes() {
    let h = tempfile::tempdir().unwrap();
    let p = h.path();
    for (args, code) in [
        (vec![], 2),
        (vec!["bogus"], 2),
        (vec!["join"], 2),
        (vec!["join", "nogroup"], 2),
        (vec!["ssh", "alias"], 2),
        (vec!["daemon", "--bogus"], 2),
        (vec!["remove-jobs"], 2),
        (vec!["version"], 0),
        (vec!["--version"], 0),
        (vec!["status"], 0),
        (vec!["status", "--json"], 0),
        (vec!["ssh", "enable"], 0),
        (vec!["ssh", "status", "--json"], 0),
        (vec!["activity", "enable"], 0),
        (vec!["personal", "status"], 1),
        (vec!["groups"], 1),
        (vec!["sync"], 1),
        (vec!["join", "a/b"], 1),
        (vec!["review", "list"], 1),
        (vec!["personal", "enable"], 1),
        (vec!["run-job", "x"], 2),
        (vec!["run-job", "a/b", "x"], 1),
    ] {
        assert_eq!(go(p, &args), code, "{args:?}");
    }
    // `sync` is one of the commands that write the log file (v1 names, `claude-sync.<day>.log`).
    let logs: Vec<_> = std::fs::read_dir(p.join(".claude-sync/logs"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        logs.iter()
            .any(|n| n.starts_with("claude-sync.") && n.ends_with(".log")),
        "{logs:?}"
    );
}

#[test]
fn every_agents_md_command_is_in_the_tree() {
    let names = command_names();
    for c in [
        "login", "logout", "groups", "join", "leave", "sync", "status", "personal", "ssh",
        "activity", "packs", "review", "daemon", "service", "update", "version",
    ] {
        assert!(names.iter().any(|n| n == c), "{c}");
    }
    assert!(!names.iter().any(|n| n == "run-job" || n == "remove-jobs"));
}

#[test]
fn a_v1_daemon_that_holds_the_lock_makes_every_command_exit_1_and_nothing_is_imported() {
    let h = tempfile::tempdir().unwrap();
    let state = h.path().join(".claude-sync");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("app.json"), "{}").unwrap();
    let lock = std::fs::File::create(state.join("daemon.lock")).unwrap();
    lock.lock().unwrap();
    for args in [vec!["status"], vec!["status", "--json"], vec!["groups"]] {
        assert_eq!(go(h.path(), &args), 1, "{args:?}");
    }
    assert!(!state.join("daemon.owner").exists());
    drop(lock);
    // once it lets go, the next command imports
    assert_eq!(go(h.path(), &["status"]), 0);
}

#[test]
fn the_stuck_daemon_message_names_the_pid_and_the_stop_command() {
    let m = claude_sync_cli::v1_daemon_message(4242);
    assert!(
        m.contains("4242") && m.contains("did not stop in 30 s"),
        "{m}"
    );
    assert!(m.contains(if cfg!(windows) {
        "taskkill /PID 4242 /F"
    } else {
        "kill 4242"
    }));
}
