//! Fixtures shaped like real v1 group and personal repos (sample data only) read and written
//! back byte for byte, unknown keys kept, one edit changes one value.
use claude_sync_engine::formats::group::{self, GroupFile};
use claude_sync_engine::formats::pack::PackManifest;
use claude_sync_engine::formats::personal::{Choices, DevicesFile, PersonalFile};
use claude_sync_engine::formats::schedules::{self, validate};
use claude_sync_engine::formats::text::*;
use claude_sync_engine::formats::{Doc, FormatError};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn fixtures() -> Vec<PathBuf> {
    let mut v = Vec::new();
    walk(&root(), &mut v);
    v
}

fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap()
}

fn fix(rel: &str) -> Vec<u8> {
    read(&root().join(rel))
}

/// Read a fixture the way its format does and write it back. `None`: not a format file.
fn round_trip(path: &Path, raw: &[u8]) -> Option<Vec<u8>> {
    let name = path.file_name()?.to_str()?;
    let in_dir = |d: &str| path.components().any(|c| c.as_os_str() == d);
    Some(match name {
        "claude-sync.json" => GroupFile::open(raw).unwrap().to_bytes().unwrap(),
        "settings.shared.json" => group::open_settings(raw).unwrap().to_bytes().unwrap(),
        "schedules.json" | "before-now.json" => {
            Doc::<Value>::parse(raw).unwrap().to_bytes().unwrap()
        }
        "pack.json" => PackManifest::open(raw).unwrap().to_bytes().unwrap(),
        "claude-sync-personal.json" => PersonalFile::open(raw).unwrap().to_bytes().unwrap(),
        "devices.json" => Doc::<DevicesFile>::parse(raw).unwrap().to_bytes().unwrap(),
        "choices.json" => Doc::<Choices>::parse(raw).unwrap().to_bytes().unwrap(),
        "CLAUDE.md" if in_dir("machine") => render_claude_md(raw, &block_lines(raw)),
        "authorized_keys" => {
            let s = std::str::from_utf8(raw).unwrap();
            replace_peers_block(s, &peers_lines(s)).into_bytes()
        }
        "COMMIT" => CommitMarker::parse(raw).unwrap().to_bytes(),
        _ => return None,
    })
}

#[test]
fn every_fixture_reads_and_writes_back_identical_bytes() {
    let mut checked = 0;
    for p in fixtures() {
        let raw = read(&p);
        if let Some(back) = round_trip(&p, &raw) {
            assert_eq!(back, raw, "{}", p.display());
            checked += 1;
        }
    }
    assert!(checked >= 20, "only {checked} fixtures were checked");
}

#[test]
fn v1_style_files_are_what_a_fresh_write_produces() {
    // What v1 writes (two spaces, LF, final newline) is what Doc::new writes.
    let g = GroupFile::open(&fix("group/claude-sync.json"))
        .unwrap()
        .value;
    assert_eq!(
        Doc::new(g).to_bytes().unwrap(),
        fix("group/claude-sync.json")
    );
    for rel in [
        "personal/devices.json",
        "personal/state/claude-sync/choices.json",
    ] {
        let raw = fix(rel);
        let out = if rel.ends_with("devices.json") {
            Doc::new(Doc::<DevicesFile>::parse(&raw).unwrap().value).to_bytes()
        } else {
            Doc::new(Doc::<Choices>::parse(&raw).unwrap().value).to_bytes()
        };
        assert_eq!(out.unwrap(), raw, "{rel}");
    }
    let p = PersonalFile::open(&fix("personal/claude-sync-personal.json")).unwrap();
    assert_eq!(
        Doc::new(PersonalFile::new()).to_bytes().unwrap(),
        p.to_bytes().unwrap()
    );
    // v1 leaves proposalMode out when off.
    let mut g = GroupFile::new("T", "alice");
    assert!(!String::from_utf8(Doc::new(g.clone()).to_bytes().unwrap())
        .unwrap()
        .contains("proposalMode"));
    g.proposal_mode = true;
    assert!(String::from_utf8(Doc::new(g).to_bytes().unwrap())
        .unwrap()
        .contains("\"proposalMode\": true"));
}

/// The lines (with their ends) of `old` and `new` differ in exactly `n` places.
fn lines_changed(old: &[u8], new: &[u8], n: usize) {
    let (o, w): (Vec<_>, Vec<_>) = (
        old.split_inclusive(|b| *b == b'\n').collect(),
        new.split_inclusive(|b| *b == b'\n').collect(),
    );
    assert_eq!(o.len(), w.len(), "line count");
    let diff = o.iter().zip(&w).filter(|(a, b)| a != b).count();
    assert_eq!(
        diff,
        n,
        "changed lines\n--- old\n{}\n--- new\n{}",
        String::from_utf8_lossy(old),
        String::from_utf8_lossy(new)
    );
}

#[test]
fn one_edit_changes_one_value_and_keeps_unknown_keys() {
    // group file
    let raw = fix("group/unknown/claude-sync.json");
    let mut d = GroupFile::open(&raw).unwrap();
    d.value.auto_accept_claude_md = false;
    let out = d.to_bytes().unwrap();
    lines_changed(&raw, &out, 1);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("\"autoAcceptClaudeMd\": false") && text.contains("futureKey"));
    // proposalMode on -> off removes exactly its line, as v1 does
    let mut d = GroupFile::open(&raw).unwrap();
    d.value.proposal_mode = false;
    let out = d.to_bytes().unwrap();
    assert_eq!(
        out.split(|b| *b == b'\n').count(),
        raw.split(|b| *b == b'\n').count() - 1
    );
    // members: a new member adds one line, the host key stays
    let mut d = GroupFile::open(&raw).unwrap();
    d.value.members.push("carol".into());
    let out = String::from_utf8(d.to_bytes().unwrap()).unwrap();
    assert!(out.contains("\"carol\"") && out.contains("\"host\": \"alice\""));

    // choices
    let raw = fix("personal/unknown/state/claude-sync/choices.json");
    let mut d = Doc::<Choices>::parse(&raw).unwrap();
    d.value.app.paused = true;
    let out = d.to_bytes().unwrap();
    lines_changed(&raw, &out, 1);
    assert!(String::from_utf8(out).unwrap().contains("futureSection"));

    // devices, personal file, pack
    let raw = fix("personal/unknown/devices.json");
    let mut d = Doc::<DevicesFile>::parse(&raw).unwrap();
    d.value.devices[1].name = "Renamed".into();
    let out = d.to_bytes().unwrap();
    lines_changed(&raw, &out, 1);
    assert!(String::from_utf8(out)
        .unwrap()
        .contains("\"color\": \"blue\""));
    let raw = fix("personal/unknown/claude-sync-personal.json");
    let mut d = PersonalFile::open(&raw).unwrap();
    d.value.new_devices_join = true;
    lines_changed(&raw, &d.to_bytes().unwrap(), 1);
    let raw = fix("group/unknown/packs/demo/pack.json");
    let mut d = PackManifest::open(&raw).unwrap();
    d.value.title = "Renamed pack".into();
    let out = d.to_bytes().unwrap();
    lines_changed(&raw, &out, 1);
    assert!(String::from_utf8(out).unwrap().contains("retries"));

    // value-shaped files
    for (rel, ptr, v) in [
        (
            "group/unknown/settings.shared.json",
            "/model",
            json!("opus"),
        ),
        (
            "group/unknown/schedules.json",
            "/jobs/0/timeoutMinutes",
            json!(45),
        ),
        (
            "group/unknown/changes/before-now.json",
            "/89abcdef0123456789abcdef0123456789abcdef/now",
            json!("Faster."),
        ),
    ] {
        let raw = fix(rel);
        let mut d = Doc::<Value>::parse(&raw).unwrap();
        *d.value.pointer_mut(ptr).unwrap() = v;
        lines_changed(&raw, &d.to_bytes().unwrap(), 1);
    }
}

#[test]
fn a_rewrite_keeps_crlf_tabs_and_compact_layout() {
    let raw = fix("group/crlf/claude-sync.json");
    let mut d = GroupFile::open(&raw).unwrap();
    d.value.name = "Renamed".into();
    let out = d.to_bytes().unwrap();
    lines_changed(&raw, &out, 1);
    assert!(out.ends_with(b"}\r\n") && !out.windows(2).any(|w| w[1] == b'\n' && w[0] != b'\r'));

    let raw = fix("group/tab/settings.shared.json");
    assert!(!raw.ends_with(b"\n"));
    let mut d = Doc::<Value>::parse(&raw).unwrap();
    d.value["model"] = json!("opus");
    let out = d.to_bytes().unwrap();
    lines_changed(&raw, &out, 1);
    assert!(out.starts_with(b"{\n\t\"model\"") && !out.ends_with(b"\n"));

    let raw = fix("group/compact/claude-sync.json");
    let mut d = GroupFile::open(&raw).unwrap();
    d.value.name = "Renamed".into();
    let out = String::from_utf8(d.to_bytes().unwrap()).unwrap();
    assert!(!out.contains('\n') && out.contains("\"name\":\"Renamed\""));
    // a key the file did not have is added only when it differs from the default
    assert!(!out.contains("autoAcceptClaudeMd") && !out.contains("sync"));
}

#[test]
fn a_newer_version_is_an_error_not_a_downgrade() {
    assert!(matches!(
        GroupFile::open(br#"{"schema":2}"#),
        Err(FormatError::TooNew { found: 2, .. })
    ));
    assert!(matches!(
        PersonalFile::open(br#"{"format":9}"#),
        Err(FormatError::TooNew { .. })
    ));
    assert!(group::open_settings(b"[1]").is_err());
    assert!(GroupFile::open(b"nope").is_err());
}

#[test]
fn settings_pointers() {
    assert_eq!(group::settings_pointer("settings.shared.json"), Some(""));
    assert_eq!(
        group::settings_pointer("settings.shared.json#/permissions/allow"),
        Some("/permissions/allow")
    );
    assert_eq!(group::settings_pointer("CLAUDE.md"), None);
}

#[test]
fn change_notes_read_the_usable_entries() {
    let d = Doc::<Value>::parse(&fix("group/unknown/changes/before-now.json")).unwrap();
    let notes = group::change_notes(&d.value);
    assert_eq!(notes.len(), 2); // "extraTopLevel" has neither text
    let n = &notes["0123456789abcdef0123456789abcdef01234567"];
    assert_eq!(n.before.as_deref(), Some("Notes were saved by hand."));
    assert_eq!(n.now.as_deref(), Some("Notes save themselves."));
    assert!(group::change_notes(&json!([1])).is_empty());
}

fn job(extra: Value) -> Value {
    let mut j = json!({"id":"x","cron":"17 10 * * 1,4","prompt":"p"});
    j.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    j
}

fn problems(j: Value) -> Vec<String> {
    validate(&json!({"jobs": [j]}))
        .1
        .into_iter()
        .map(|p| p.message)
        .collect()
}

#[test]
fn schedules_limits() {
    let (jobs, probs) = validate(
        &Doc::<Value>::parse(&fix("group/schedules.json"))
            .unwrap()
            .value,
    );
    assert_eq!((jobs.len(), probs.len()), (2, 0));
    assert!(validate(&json!({})).0.is_empty());
    assert_eq!(validate(&json!([])).1.len(), 1);
    assert_eq!(validate(&json!({"jobs": 3})).1.len(), 1);

    let many: Vec<Value> = (0..21)
        .map(|i| job(json!({"id": format!("j{i}")})))
        .collect();
    let (ok, bad) = validate(&json!({ "jobs": many }));
    assert_eq!((ok.len(), bad.len()), (20, 1));
    assert!(bad[0].message.contains("more than 20"));

    let at = |n: usize| "x".repeat(n);
    assert!(problems(job(json!({"prompt": at(4096)}))).is_empty());
    assert!(!problems(job(json!({"prompt": at(4097)}))).is_empty());
    assert!(!problems(job(json!({"prompt": "-rf"}))).is_empty());
    assert!(!problems(job(json!({"prompt": "  "}))).is_empty());
    assert!(problems(job(json!({"timeoutMinutes": 240}))).is_empty());
    assert!(problems(job(json!({"timeoutMinutes": 1}))).is_empty());
    assert!(!problems(job(json!({"timeoutMinutes": 241}))).is_empty());
    assert!(!problems(job(json!({"timeoutMinutes": 0}))).is_empty());
    assert!(problems(job(json!({"id": at(41)}))).is_empty());
    for bad_id in [at(42), "-a".into(), "A".into(), "".into(), "a_b".into()] {
        assert!(
            !problems(job(json!({ "id": bad_id }))).is_empty(),
            "{bad_id}"
        );
    }
    assert!(!problems(job(json!({"permissionMode": "yolo"}))).is_empty());
    assert!(problems(job(json!({"permissionMode": "acceptEdits"}))).is_empty());
    assert!(problems(job(
        json!({"args": ["--chrome", "--model", "claude-x", "--effort", "max"]})
    ))
    .is_empty());
    for bad in [
        json!(["--dangerously-skip-permissions"]),
        json!(["--chrome", "--chrome"]),
        json!(["--effort", "extreme"]),
        json!(["--model"]),
        json!(["--model", "-x"]),
    ] {
        assert!(!problems(job(json!({ "args": bad }))).is_empty(), "{bad}");
    }
    assert!(!problems(job(json!({"cwd": "/etc"}))).is_empty());
    assert!(!problems(job(json!({"cwd": "~/../x"}))).is_empty());
    assert!(!problems(job(json!({"report": "~"}))).is_empty());
    let (ok, bad) = validate(&json!({"jobs": [job(json!({})), job(json!({}))]}));
    assert_eq!((ok.len(), bad.len()), (1, 1));
    // a bad job never takes the others down
    let (ok, bad) = validate(&json!({"jobs": [{"nope": 1}, job(json!({"id": "y"}))]}));
    assert_eq!((ok.len(), bad.len()), (1, 1));
}

#[test]
fn cron_subset() {
    for good in [
        "17 10 * * 1,4",
        "0 9 * * 1-5",
        "  1   2 * * 3 ",
        "5 7 * * 0-6",
    ] {
        assert!(schedules::valid_cron(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "17 10 * *",
        "* 10 * * *",
        "*/5 10 * * *",
        "60 10 * * *",
        "0 24 * * *",
        "0 10 1 * *",
        "0 10 * 1 *",
        "0 10 * * 7",
        "0 10 * * mon",
        "0 10 * * 5-1",
        "0 10 * * 1,",
        "+1 10 * * *",
    ] {
        assert!(schedules::valid_cron(bad).is_err(), "{bad:?}");
    }
    let hours = (0..24).map(|h| h.to_string()).collect::<Vec<_>>().join(",");
    assert!(schedules::valid_cron(&format!("0,30 {hours} * * *")).is_ok());
    assert!(schedules::valid_cron(&format!("0,30,45 {hours} * * *")).is_err());
}

#[test]
fn pack_limits() {
    let d = PackManifest::open(&fix("group/packs/demo/pack.json")).unwrap();
    assert!(d.value.check("demo").is_ok());
    assert!(d.value.check("other").is_err());
    let with = |f: &dyn Fn(&mut PackManifest)| {
        let mut p = d.value.clone();
        f(&mut p);
        p.check("demo")
    };
    assert!(with(&|p| p.title = "x".repeat(41)).is_err());
    assert!(with(&|p| p.summary = "".into()).is_err());
    assert!(with(&|p| p.os = Some(vec!["beos".into()])).is_err());
    assert!(with(&|p| p.plugins.push("nomarket".into())).is_err());
    assert!(with(&|p| p.files = vec!["commands/x.md".into()]).is_err());
    assert!(with(&|p| p.setup = "other.md".into()).is_err());
    assert!(with(&|p| p.check.run = vec!["a".into(); 9]).is_err());
    assert!(with(&|p| p.check.timeout_seconds = Some(121)).is_err());
    assert!(with(&|p| p.needs[0].url = Some("http://x".into())).is_err());
    assert!(PackManifest::open(&vec![b' '; 65 * 1024]).is_err());
}

#[test]
fn claude_md_block_never_touches_text_outside() {
    let raw = fix("machine/CLAUDE.md");
    let lines = block_lines(&raw);
    assert_eq!(
        lines,
        ["@~/.claude-sync/groups/sample__team-data/CLAUDE.md"]
    );
    // new lines replace the block; the user's text stays byte for byte
    let out = render_claude_md(&raw, &["@a".into(), "@b".into()]);
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("@a\n@b\n") && text.ends_with("\n# My own notes\n\nPrefer short replies.\n")
    );
    // removing the block leaves the user's text
    let none = render_claude_md(&raw, &[]);
    assert_eq!(none, b"# My own notes\n\nPrefer short replies.\n");
    // a block in the middle
    let mid = fix("machine/mid-block/CLAUDE.md");
    let out = String::from_utf8(render_claude_md(&mid, &["@z".into()])).unwrap();
    assert!(out.starts_with("# Mine first\n\n") && out.ends_with("\n# Mine after\n"));
    // no block: a new one goes on top, followed by a blank line; removing it restores the file
    let plain = fix("personal/CLAUDE.md");
    let with = render_claude_md(&plain, &["@g".into()]);
    assert!(with.starts_with(BLOCK_BEGIN.as_bytes()));
    assert_eq!(render_claude_md(&with, &[]), plain);
    // CRLF files get CRLF blocks
    let crlf = fix("machine/crlf/CLAUDE.md");
    let out = render_claude_md(&crlf, &["@a".into()]);
    assert!(!out.windows(2).any(|w| w[1] == b'\n' && w[0] != b'\r'));
    // the begin marker must start a line
    let inline = b"text <!-- claude-sync:begin (managed, do not edit inside) --> x\n";
    assert!(block_lines(inline).is_empty());
}

#[test]
fn peers_block_keeps_foreign_keys() {
    let raw = String::from_utf8(fix("machine/authorized_keys")).unwrap();
    let lines = peers_lines(&raw);
    assert_eq!(lines.len(), 1);
    let out = replace_peers_block(&raw, &["a".into(), "b".into()]);
    assert!(out.starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEXAMPLE"));
    assert!(out.contains(&format!("{PEERS_BEGIN}\na\nb\n{PEERS_END}\nssh-rsa")));
    let gone = replace_peers_block(&raw, &[]);
    assert!(!gone.contains("claude-sync") && gone.lines().count() == 2);
    assert_eq!(replace_peers_block("", &[]), "");
    assert_eq!(
        replace_peers_block("k\n", &["a".into()]),
        format!("k\n{PEERS_BEGIN}\na\n{PEERS_END}\n")
    );
}

#[test]
fn proposal_branch_and_commit_marker() {
    assert_eq!(
        proposal_branch("bo-b", 1_795_000_000).as_deref(),
        Some("proposal/bo-b-20261118110640")
    );
    assert_eq!(
        parse_proposal_branch("proposal/bo-b-20261118110640"),
        Some(("bo-b", "20261118110640"))
    );
    for bad in [
        "main",
        "proposal/bo",
        "proposal/bo-123",
        "proposal/-20261118110640",
        "proposal/x-2026111809464a",
    ] {
        assert_eq!(parse_proposal_branch(bad), None, "{bad}");
    }
    let c = CommitMarker::parse(b"abc\n").unwrap();
    assert_eq!((c.commit.as_str(), c.date), ("abc", None));
    assert_eq!(CommitMarker::parse(b"abc\n").unwrap().to_bytes(), b"abc\n");
    assert!(CommitMarker::parse(b"").is_none());
}
