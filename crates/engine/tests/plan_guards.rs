//! Plan guards: every push is secret-scanned; line-end differences alone are no conflict.
use claude_sync_engine::plan::{plan, FileSet, Opts, Scope};
use std::collections::BTreeSet;

const P: &str = "memory/home/MEMORY.md";

fn opts() -> Opts {
    Opts {
        scope: Scope::Personal,
        push: true,
        links: BTreeSet::new(),
    }
}

fn set(t: &str) -> FileSet {
    FileSet::from([(P.to_string(), t.as_bytes().to_vec())])
}

#[test]
fn merged_memory_index_with_a_token_is_not_pushed() {
    let token = format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
    let base = set("- a\n");
    let local = set(&format!("- a\n- key {token}\n"));
    let remote = set("- a\n- b\n");
    let pl = plan(&remote, &base, &local, &opts());
    assert!(pl.pushes.is_empty(), "pushed: {:?}", pl.pushes);
    assert_eq!(pl.secrets.len(), 1);
    assert_eq!(pl.new_base.get(P), base.get(P));
}

#[test]
fn unchanged_crlf_local_with_a_remote_change_is_no_conflict() {
    let f = |t: &str| FileSet::from([("CLAUDE.md".to_string(), t.as_bytes().to_vec())]);
    let (base, local, remote) = (f("- a\n"), f("- a\r\n"), f("- a\n- b\n"));
    let pl = plan(&remote, &base, &local, &opts());
    assert!(pl.conflicts.is_empty());
    assert_eq!(pl.writes.get("CLAUDE.md"), remote.get("CLAUDE.md"));
}

#[test]
fn a_secret_line_already_on_the_remote_does_not_hold_the_merged_push() {
    let token = format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
    let base = set("- a\n");
    let local = set("- a\n- c\n");
    let remote = set(&format!("- a\n- key {token}\n"));
    let pl = plan(&remote, &base, &local, &opts());
    assert!(pl.pushes.contains_key(P), "held: {:?}", pl.secrets);
    assert!(pl.secrets.is_empty());
}
