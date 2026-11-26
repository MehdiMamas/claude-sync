//! `run-job` takes v1's `<owner/repo> <id>` (tasks v1 registered keep working) and `<id>`.
use clap::Parser;
use claude_sync_cli::{run_status, Cli, Cmd, Ctx};
use claude_sync_engine::traits::Store;
use claude_sync_store::Db;
use std::path::Path;

fn go(home: &Path, args: &[&str]) -> u8 {
    let ctx = Ctx {
        home: Some(home.to_path_buf()),
        ..Ctx::cli()
    };
    let mut v = vec!["claude-sync-cli".to_string()];
    v.extend(args.iter().map(|s| s.to_string()));
    run_status(v, &ctx)
}

/// Groups `repos`, each with the job `id` in its copy of schedules.json.
fn seed(home: &Path, repos: &[&str], id: &str) {
    let state = home.join(".claude-sync");
    std::fs::create_dir_all(&state).unwrap();
    let db = Db::open(&state.join("claude-sync.db")).unwrap();
    let doc = serde_json::json!({"jobs": [{"id": id, "cron": "17 10 * * 1", "prompt": "hi"}]});
    for r in repos {
        db.add_group(r, false).unwrap();
        db.write(&format!("schedules/{r}"), doc.to_string().as_bytes())
            .unwrap();
    }
}

#[test]
fn v1_argv_parses() {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1-run-job-argv.txt"),
    )
    .unwrap();
    for line in text.lines() {
        let mut v = vec!["claude-sync"];
        v.extend(line.split(' '));
        let cli = Cli::try_parse_from(v).unwrap();
        let Cmd::RunJob { args, now } = cli.cmd else {
            panic!("not run-job: {line}")
        };
        assert_eq!(args, ["acme/a", "setup-scout"]);
        assert_eq!(now, line.ends_with("--now"));
    }
}

#[test]
fn both_forms_find_the_job_and_ambiguity_is_usage() {
    let h = tempfile::tempdir().unwrap();
    let p = h.path();
    seed(p, &["acme/a"], "nightly");
    // found, but not accepted: a run failure (1), not a usage error (2)
    assert_eq!(go(p, &["run-job", "acme/a", "nightly"]), 1);
    assert_eq!(go(p, &["run-job", "nightly"]), 1);
    assert_eq!(go(p, &["run-job", "acme/a", "nightly", "--now"]), 1);
    // unknown id: v1 exit codes
    assert_eq!(go(p, &["run-job", "acme/a", "nope"]), 1);
    assert_eq!(go(p, &["run-job", "nope"]), 2);
    assert_eq!(go(p, &["run-job", "notagroup", "nightly"]), 2);
    assert_eq!(go(p, &["run-job"]), 2);
    // the same id in two groups: the single-id form is ambiguous
    seed(p, &["acme/b"], "nightly");
    assert_eq!(go(p, &["run-job", "nightly"]), 2);
    assert_eq!(go(p, &["run-job", "acme/b", "nightly"]), 1);
}
