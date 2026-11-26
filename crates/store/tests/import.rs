//! `import_v1` against a fixture v1 state folder that holds every v1 file (sample data only).
use claude_sync_engine::traits::Store;
use claude_sync_store::{classify, import_v1, Class, DaemonLockHeld, Db, Outcome, RequestRow};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    fn go(dir: &Path, rel: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if e.file_type().unwrap().is_dir() {
                go(&e.path(), &r, out);
            } else {
                out.insert(r, std::fs::read(e.path()).unwrap());
            }
        }
    }
    go(root, "", &mut out);
    out
}

fn copy(from: &Path, to: &Path) {
    for (rel, bytes) in files(from) {
        let p = to.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
}

struct Home {
    dir: tempfile::TempDir,
}
impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        copy(&fixtures().join("v1-state"), &dir.path().join("sync"));
        copy(&fixtures().join("v1-claude"), &dir.path().join("claude"));
        Self { dir }
    }
    fn sync(&self) -> PathBuf {
        self.dir.path().join("sync")
    }
    fn claude(&self) -> PathBuf {
        self.dir.path().join("claude")
    }
    fn db(&self) -> Db {
        Db::open(&self.dir.path().join("sync/store.db")).unwrap()
    }
    fn import(&self, db: &Db) -> claude_sync_store::Report {
        match import_v1(db, &self.sync(), &self.claude(), &DaemonLockHeld::claim()).unwrap() {
            Outcome::Done(r) => r,
            Outcome::AlreadyDone => panic!("ran before"),
        }
    }
}

/// Rows of `sql` as text, read through a second connection.
fn rows(h: &Home, sql: &str) -> Vec<Vec<String>> {
    let c = rusqlite::Connection::open(h.sync().join("store.db")).unwrap();
    let mut st = c.prepare(sql).unwrap();
    let n = st.column_count();
    st.query_map([], |r| {
        (0..n)
            .map(|i| {
                Ok(match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => "NULL".into(),
                    rusqlite::types::ValueRef::Integer(v) => v.to_string(),
                    rusqlite::types::ValueRef::Real(v) => v.to_string(),
                    rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
                        String::from_utf8_lossy(t).into_owned()
                    }
                })
            })
            .collect()
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn one(h: &Home, sql: &str) -> String {
    rows(h, sql).remove(0).remove(0)
}

#[test]
fn every_fixture_file_is_known_to_the_importer() {
    let state = files(&fixtures().join("v1-state"));
    assert!(state.len() > 40, "the fixture holds every v1 file");
    let unknown: Vec<_> = state
        .keys()
        .filter(|f| classify(f) == Class::Unknown)
        .collect();
    assert!(
        unknown.is_empty(),
        "files the importer does not know: {unknown:?}"
    );
    // choices.json lives under ~/.claude, not in the state folder.
    let claude: Vec<_> = files(&fixtures().join("v1-claude")).into_keys().collect();
    assert_eq!(claude, ["personal-state/claude-sync/choices.json"]);
}

#[test]
fn a_file_the_importer_does_not_know_is_reported_and_left() {
    assert_eq!(classify("surprise.json"), Class::Unknown);
    assert_eq!(classify("groups/o__r/new-thing.json"), Class::Unknown);
    let h = Home::new();
    std::fs::write(h.sync().join("surprise.json"), "{}").unwrap();
    let r = h.import(&h.db());
    assert_eq!(r.unknown, ["surprise.json"]);
    assert!(h.sync().join("surprise.json").exists());
}

#[test]
fn imports_every_v1_file_into_the_tables() {
    let h = Home::new();
    let db = h.db();
    let r = h.import(&db);
    assert!(r.skipped.is_empty(), "{:?}", r.skipped);
    assert!(r.unknown.is_empty());
    assert_eq!(r.choices_from, "file");
    // groups in the order of choices.json; the host is the signed-in login
    assert_eq!(
        rows(
            &h,
            "SELECT repo, role, state, order_idx, off FROM groups ORDER BY order_idx"
        ),
        [
            ["sample-org/team-data", "member", "active", "0", "0"],
            ["sample-org/friends-data", "host", "active", "1", "0"]
        ]
    );
    // base head with its ETag, and every base file by git blob sha ("demo skill\n")
    assert_eq!(
        rows(
            &h,
            "SELECT repo, head_sha, etag, synced_at FROM bases WHERE etag IS NOT NULL"
        ),
        [[
            "sample-org/friends-data",
            "c0ffee0000000000000000000000000000000001",
            "W/\"abc\"",
            "1790000400"
        ]]
    );
    assert_eq!(
        rows(
            &h,
            "SELECT path, blob_sha FROM base_files WHERE repo = 'sample-org/friends-data'"
        ),
        [[
            "skills/demo/SKILL.md",
            "43ce1532dd726ce96417f295f2972cdc15d5e019"
        ]]
    );
    assert_eq!(one(&h, "SELECT count(*) FROM bases"), "2");
    // review: a dropped line, a pending item and a declined id
    assert_eq!(
        rows(
            &h,
            "SELECT line, author, status FROM review_items ORDER BY id"
        ),
        [
            ["Always use tabs", "", "declined"],
            ["Prefer small PRs", "sample-friend", "pending"],
            ["i0", "", "declined"]
        ]
    );
    // jobs: the accepted hash and the on/off switch come from choices.json
    assert_eq!(
        rows(&h, "SELECT repo, job_id, spec_hash, accepted_hash, enabled, last_result, last_run FROM jobs ORDER BY job_id"),
        [
            ["sample-org/friends-data", "nightly", "spec-1", "hash-new", "1", "ok", "1790000060"],
            ["sample-org/team-data", "weekly", "", "NULL", "1", "", "0"]
        ]
    );
    assert_eq!(
        rows(
            &h,
            "SELECT id, kind, text, read, title, time, repo FROM notifications ORDER BY id"
        ),
        [
            [
                "2",
                "group",
                "You joined friends-data",
                "1",
                "Joined",
                "1790000200",
                "sample-org/friends-data"
            ],
            [
                "3",
                "update",
                "claude-sync 0.1.27 is ready",
                "0",
                "Update ready",
                "1790000300",
                "NULL"
            ]
        ]
    );
    assert_eq!(
        one(
            &h,
            "SELECT value FROM settings WHERE key = 'notifications.next_id'"
        ),
        "4"
    );
    assert_eq!(
        rows(
            &h,
            "SELECT caller, endpoint, status, n FROM requests ORDER BY n DESC"
        ),
        [
            ["tick", "GET repos/:repo", "304", "40"],
            ["sync", "GET repos/:repo/git/trees/:sha", "200", "7"]
        ]
    );
    assert_eq!(rows(&h, "SELECT body, tries FROM feedback_queue").len(), 1);
    assert_eq!(
        rows(
            &h,
            "SELECT repo, path, saved_at FROM conflicts ORDER BY repo"
        ),
        [
            ["personal", "CLAUDE.md", "1791972900"],
            [
                "sample-org/friends-data",
                "skills/demo/SKILL.md",
                "1791884701"
            ]
        ]
    );
    // files that have no table are kept whole
    for k in [
        "v1/state.json",
        "v1/manifest.json",
        "v1/app.json",
        "v1/activity.json",
        "v1/ssh-peers.json",
        "v1/settings-owned.json",
        "v1/plugins.json",
        "v1/packs.json",
        "v1/jobs.json",
        "v1/personal/state.json",
        "v1/personal/base/head.json",
        "v1/groups/sample-org__friends-data/status.json",
        "v1/groups/sample-org__friends-data/claude-md-review.json",
        "choices",
    ] {
        assert!(db.read(k).unwrap().is_some(), "{k}");
    }
    // a key a newer version wrote is kept
    let c = String::from_utf8(db.read("choices").unwrap().unwrap()).unwrap();
    assert!(c.contains("futureKey"));
}

#[test]
fn choices_json_wins_over_the_old_fields() {
    // state.json says team-data is off, paused, 120 s; jobs.json says nightly is off with
    // another accepted hash; choices.json says the opposite. A consent taken back stays back.
    let h = Home::new();
    h.import(&h.db());
    assert_eq!(
        one(
            &h,
            "SELECT off FROM groups WHERE repo = 'sample-org/team-data'"
        ),
        "0"
    );
    assert_eq!(
        one(
            &h,
            "SELECT accepted_hash FROM jobs WHERE job_id = 'nightly'"
        ),
        "hash-new"
    );
    assert_eq!(
        one(&h, "SELECT enabled FROM jobs WHERE job_id = 'nightly'"),
        "1"
    );
    let c = h.db().read("choices").unwrap().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&c).unwrap();
    assert_eq!(v["app"]["paused"], false);
    assert_eq!(v["app"]["intervalSecs"], 60);
}

#[test]
fn old_fields_are_used_only_when_choices_json_is_missing() {
    let h = Home::new();
    std::fs::remove_file(h.claude().join("personal-state/claude-sync/choices.json")).unwrap();
    let r = h.import(&h.db());
    assert_eq!(r.choices_from, "old-fields");
    assert_eq!(
        rows(
            &h,
            "SELECT repo, off, order_idx FROM groups ORDER BY order_idx"
        ),
        [
            ["sample-org/friends-data", "0", "0"],
            ["sample-org/team-data", "1", "1"]
        ]
    );
    assert_eq!(
        rows(
            &h,
            "SELECT job_id, accepted_hash, enabled FROM jobs WHERE job_id = 'nightly'"
        ),
        [["nightly", "old-hash", "0"]]
    );
    let v: serde_json::Value =
        serde_json::from_slice(&h.db().read("choices").unwrap().unwrap()).unwrap();
    assert_eq!(v["app"]["paused"], true);
    assert_eq!(v["app"]["intervalSecs"], 120);
    assert_eq!(v["groups"]["sample-org/team-data"]["autoRunJobs"], true);
    assert_eq!(
        v["groups"]["sample-org/friends-data"]["reviewClaudeMd"],
        true
    );
}

#[test]
fn an_unreadable_choices_json_never_brings_a_consent_back() {
    let h = Home::new();
    std::fs::write(
        h.claude().join("personal-state/claude-sync/choices.json"),
        "{\"groups\": ",
    )
    .unwrap();
    let r = h.import(&h.db());
    assert_eq!(r.choices_from, "unreadable");
    assert_eq!(r.skipped[0].file, "choices.json");
    assert_eq!(
        one(
            &h,
            "SELECT count(*) FROM jobs WHERE accepted_hash IS NOT NULL"
        ),
        "0"
    );
    let v: serde_json::Value =
        serde_json::from_slice(&h.db().read("choices").unwrap().unwrap()).unwrap();
    assert_eq!(v["groups"]["sample-org/team-data"]["autoRunJobs"], false);
}

#[test]
fn a_torn_file_is_reported_skipped_and_left_in_place() {
    let h = Home::new();
    let torn = b"{\"jobs\": {\"nightly\": ";
    std::fs::write(h.sync().join("jobs.json"), torn).unwrap();
    std::fs::write(
        h.sync().join("outbox/1790000000-4242.json"),
        b"{\"title\": ",
    )
    .unwrap();
    let r = h.import(&h.db());
    let names: Vec<_> = r.skipped.iter().map(|s| s.file.as_str()).collect();
    assert_eq!(names, ["jobs.json", "outbox/1790000000-4242.json"]);
    assert!(!r.imported.contains(&"jobs.json".to_string()));
    assert_eq!(std::fs::read(h.sync().join("jobs.json")).unwrap(), torn);
    assert_eq!(one(&h, "SELECT count(*) FROM feedback_queue"), "0");
    // everything else still came in
    assert_eq!(one(&h, "SELECT count(*) FROM groups"), "2");
    assert_eq!(h.db().import_report().unwrap().unwrap(), r);
}

#[test]
fn the_import_runs_once_and_never_touches_a_v1_file() {
    let h = Home::new();
    let before = (files(&h.sync()), files(&h.claude()));
    let db = h.db();
    h.import(&db);
    // only store.db and its WAL files are new
    let mut after = files(&h.sync());
    after.retain(|k, _| !k.starts_with("store.db"));
    assert_eq!(after, before.0, "v1 state files untouched");
    assert_eq!(files(&h.claude()), before.1);
    let counts = rows(
        &h,
        "SELECT (SELECT count(*) FROM requests), (SELECT count(*) FROM notifications)",
    );
    let second = import_v1(&db, &h.sync(), &h.claude(), &DaemonLockHeld::claim()).unwrap();
    assert!(matches!(second, Outcome::AlreadyDone));
    assert_eq!(
        rows(
            &h,
            "SELECT (SELECT count(*) FROM requests), (SELECT count(*) FROM notifications)"
        ),
        counts
    );
    // and after reopening the file
    drop(db);
    let again = import_v1(&h.db(), &h.sync(), &h.claude(), &DaemonLockHeld::claim()).unwrap();
    assert!(matches!(again, Outcome::AlreadyDone));
}

#[test]
fn a_machine_with_no_v1_files_imports_nothing_and_records_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("store.db")).unwrap();
    let r = match import_v1(
        &db,
        &dir.path().join("none"),
        &dir.path().join("c"),
        &DaemonLockHeld::claim(),
    )
    .unwrap()
    {
        Outcome::Done(r) => r,
        Outcome::AlreadyDone => panic!(),
    };
    assert!(r.imported.is_empty() && r.skipped.is_empty());
    assert!(db.import_report().unwrap().is_some());
}

#[test]
fn the_file_is_private_wal_and_versioned_with_exactly_the_er_tables() {
    let h = Home::new();
    let db = h.db();
    assert_eq!(db.schema_version().unwrap(), 2);
    assert_eq!(one(&h, "PRAGMA journal_mode"), "wal");
    let tables: Vec<String> = rows(
        &h,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .into_iter()
    .flatten()
    .collect();
    assert_eq!(
        tables,
        [
            "base_files",
            "bases",
            "conflicts",
            "feedback_queue",
            "groups",
            "jobs",
            "notifications",
            "personal_base_files",
            "requests",
            "review_items",
            "settings"
        ]
    );
    assert_eq!(
        one(&h, "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'base_files' AND name NOT LIKE 'sqlite_%'"),
        "base_files_blob"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        db.write("k", b"v").unwrap();
        for f in ["store.db", "store.db-wal"] {
            let m = std::fs::metadata(h.sync().join(f))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(m, 0o600, "{f}");
        }
    }
}

#[test]
fn reopening_keeps_data_and_a_newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("store.db");
    {
        let db = Db::open(&p).unwrap();
        db.write("choices", b"{}").unwrap();
        assert_eq!(db.read("nope").unwrap(), None);
    }
    assert_eq!(
        Db::open(&p).unwrap().read("choices").unwrap().unwrap(),
        b"{}"
    );
    rusqlite::Connection::open(&p)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let e = Db::open(&p).err().unwrap().to_string();
    assert!(e.contains("99") && e.contains("update claude-sync"), "{e}");
}

#[test]
fn request_counts_are_added_per_flush() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("store.db")).unwrap();
    let row = |n| RequestRow {
        caller: "tick".into(),
        endpoint: "GET repos/:repo".into(),
        status: "304".into(),
        n,
    };
    db.add_requests(&[
        row(3),
        RequestRow {
            n: 1,
            endpoint: "GET user".into(),
            ..row(0)
        },
    ])
    .unwrap();
    db.add_requests(&[row(2)]).unwrap();
    let got = db.requests().unwrap();
    assert_eq!(got[0], row(5));
    assert_eq!(got.len(), 2);
}

#[test]
fn v1_review_state_becomes_the_v2_review_state() {
    use claude_sync_engine::review::ReviewState;
    let h = Home::new();
    let db = h.db();
    h.import(&db);
    let b = Store::read(&db, "review/sample-org/friends-data")
        .unwrap()
        .expect("review state imported");
    let st: ReviewState = serde_json::from_slice(&b).unwrap();
    // the pending line is held, not accepted; the per-group choice (review first) applies
    assert_eq!(st.pending.len(), 1);
    assert_eq!(st.pending[0].lines, ["Prefer small PRs"]);
    assert_eq!(st.pending[0].author, "sample-friend");
    assert_eq!(st.declined[0].id, "i0");
    assert_eq!(st.review, Some(true));
    assert_eq!(st.accepted, "# Friends rules\n");
    // a group with no v1 review file gets none (its first pass accepts, as in v1)
    assert!(Store::read(&db, "review/sample-org/team-data")
        .unwrap()
        .is_none());
}
