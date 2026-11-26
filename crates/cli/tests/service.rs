//! The service text is v1's, byte for byte (fixtures written from the v1 source).
use claude_sync_cli::service::{cron_lines, merge_cron, strip_cron, unit_text};
use std::path::Path;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

#[test]
fn unit_and_cron_bytes_equal_v1() {
    let exe = Path::new("/home/me/.local/bin/claude-sync-cli");
    assert_eq!(unit_text(exe), fixture("v1-claude-sync.service"));
    let crontab = fixture("v1-crontab.txt");
    assert_eq!(cron_lines(exe).join("\n") + "\n", crontab);
    assert_eq!(merge_cron("", exe), crontab);
    // another line of the person's is kept, ours replaced, never doubled
    let mine = "0 3 * * * backup.sh\n";
    let merged = merge_cron(mine, exe);
    assert_eq!(merged, format!("{mine}{crontab}"));
    assert_eq!(merge_cron(&merged, exe), merged);
    assert_eq!(strip_cron(&merged), mine);
}

#[test]
fn awkward_paths_are_quoted_as_v1_did() {
    let exe = Path::new("/home/a b/50%/it's/cli");
    let want = fixture("v1-unit-awkward.txt");
    let mut lines = want.lines();
    assert!(unit_text(exe).contains(lines.next().unwrap()));
    assert_eq!(cron_lines(exe)[0], lines.next().unwrap());
}
