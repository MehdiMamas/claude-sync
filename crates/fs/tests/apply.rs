use claude_sync_fs::apply::*;
use claude_sync_fs::pass::SyncLock;
use claude_sync_fs::Home;
use claude_sync_store::Db;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

fn setup() -> (tempfile::TempDir, Home, Db) {
    let t = tempfile::tempdir().unwrap();
    let h = Home::under(t.path());
    std::fs::create_dir_all(&h.state).unwrap();
    let db = Db::open(&h.state.join("claude-sync.db")).unwrap();
    (t, h, db)
}

#[cfg(unix)]
#[test]
fn nothing_is_written_through_a_link_and_the_mode_is_kept() {
    use claude_sync_engine::traits::Disk;
    use claude_sync_fs::atomic::write_atomic;
    use claude_sync_fs::{FsDisk, Scope};
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (t, h, _db) = setup();
    let outside = t.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(&h.claude).unwrap();
    symlink(&outside, h.claude.join("skills")).unwrap();
    let d = FsDisk::new(&h, Scope::Group("o/r".into()));
    assert!(d.write("skills/a/SKILL.md", b"x").is_err());
    assert!(!outside.join("a").exists());
    assert_eq!(d.links().unwrap(), ["skills"]);
    // ..and a path with .. or an absolute one is no managed path
    assert!(d.write("skills/../../evil", b"x").is_err());
    // the mode of an existing file survives a replace
    let f = t.path().join("f");
    std::fs::write(&f, "a").unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o640)).unwrap();
    write_atomic(&f, b"b").unwrap();
    assert_eq!(
        std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(std::fs::read(&f).unwrap(), b"b");
}

#[test]
fn the_block_holds_group_imports_and_the_users_text_stays() {
    let (_t, h, _db) = setup();
    std::fs::create_dir_all(&h.claude).unwrap();
    std::fs::write(h.claude.join("CLAUDE.md"), "my own rules\n").unwrap();
    write_claude_md_copy(&h, "acme/team", Some(b"team rules\n"), None, &[], 1).unwrap();
    assert!(update_block(&h, &[("acme/team".into(), false)], 10).unwrap());
    let t = std::fs::read_to_string(h.claude.join("CLAUDE.md")).unwrap();
    assert!(
        t.contains("my own rules") && t.contains("claude-sync/groups/acme__team/CLAUDE.md"),
        "{t}"
    );
    assert!(
        !update_block(&h, &[("acme/team".into(), false)], 11).unwrap(),
        "same again: no change"
    );
    // a disabled group leaves only the user's text
    update_block(&h, &[("acme/team".into(), true)], 12).unwrap();
    assert_eq!(
        std::fs::read_to_string(h.claude.join("CLAUDE.md"))
            .unwrap()
            .trim(),
        "my own rules"
    );
    assert!(h.state.join("backups").exists());
}

#[test]
fn a_file_the_app_created_goes_away_when_the_block_empties() {
    let (_t, h, _db) = setup();
    write_claude_md_copy(&h, "acme/team", Some(b"x\n"), None, &[], 1).unwrap();
    update_block(&h, &[("acme/team".into(), false)], 10).unwrap();
    assert!(h.claude.join("CLAUDE.md").exists());
    update_block(&h, &[("acme/team".into(), true)], 11).unwrap();
    assert!(!h.claude.join("CLAUDE.md").exists());
}

#[test]
fn settings_merge_keeps_the_users_keys_reports_edits_and_undoes_them() {
    let (_t, h, db) = setup();
    std::fs::create_dir_all(&h.claude).unwrap();
    std::fs::write(h.claude.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
    let none = BTreeMap::new();
    let pin = BTreeSet::new();
    let o = apply_settings(
        &h,
        &db,
        "acme/team",
        Some(br#"{"model":"m1"}"#),
        &none,
        &pin,
        10,
    )
    .unwrap();
    assert!(o.changed);
    let read = || {
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(h.claude.join("settings.json")).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(read()["theme"], "dark");
    assert_eq!(read()["model"], "m1");
    // the user edits an owned value: it is an edit, kept on the next merge, and undo puts it back
    std::fs::write(
        h.claude.join("settings.json"),
        r#"{"theme":"dark","model":"mine"}"#,
    )
    .unwrap();
    assert_eq!(
        edited(&h, &db, "acme/team", &pin).unwrap(),
        [("/model".to_string(), false)]
    );
    apply_settings(
        &h,
        &db,
        "acme/team",
        Some(br#"{"model":"m1"}"#),
        &none,
        &pin,
        11,
    )
    .unwrap();
    assert_eq!(read()["model"], "mine");
    restore_settings(&h, &db, "acme/team", "", 12).unwrap();
    assert_eq!(read()["model"], "m1");
    // group off: its values leave, the user's stay
    unapply_settings(&h, &db, "acme/team", 13).unwrap();
    assert_eq!(read(), serde_json::json!({"theme":"dark"}));
}

#[test]
fn settings_that_are_not_an_object_are_never_touched() {
    let (_t, h, db) = setup();
    std::fs::create_dir_all(&h.claude).unwrap();
    std::fs::write(h.claude.join("settings.json"), "[1]").unwrap();
    let r = apply_settings(
        &h,
        &db,
        "a/b",
        Some(br#"{"model":"m"}"#),
        &BTreeMap::new(),
        &BTreeSet::new(),
        1,
    );
    assert_eq!(r.err().as_deref(), Some(claude_sync_fs::NOT_OBJECT_LOCAL));
    assert_eq!(
        std::fs::read_to_string(h.claude.join("settings.json")).unwrap(),
        "[1]"
    );
}

#[test]
fn the_sync_lock_is_shared_and_a_second_pass_finds_it_busy() {
    let (_t, h, _db) = setup();
    let a = SyncLock::acquire(&h.lock_file(), Duration::ZERO).unwrap();
    assert!(SyncLock::acquire(&h.lock_file(), Duration::from_millis(100)).is_err());
    drop(a);
    assert!(SyncLock::acquire(&h.lock_file(), Duration::ZERO).is_ok());
}

#[test]
fn v1_owned_file_is_converted_so_the_first_pass_keeps_user_edits_and_drops_old_keys() {
    use claude_sync_store::{import_v1, DaemonLockHeld};
    let (t, h, db) = setup();
    let sync = t.path().join("v1sync");
    std::fs::create_dir_all(&sync).unwrap();
    std::fs::write(
        sync.join("settings-owned.json"),
        r#"{"acme/team":{"values":{"/model":"m1","/old":"x"},"array_items":{}}}"#,
    )
    .unwrap();
    import_v1(&db, &sync, &t.path().join("c"), &DaemonLockHeld::claim()).unwrap();
    std::fs::create_dir_all(&h.claude).unwrap();
    std::fs::write(
        h.claude.join("settings.json"),
        r#"{"model":"mine","old":"x"}"#,
    )
    .unwrap();
    let (none, pin) = (BTreeMap::new(), BTreeSet::new());
    apply_settings(
        &h,
        &db,
        "acme/team",
        Some(br#"{"model":"m1"}"#),
        &none,
        &pin,
        10,
    )
    .unwrap();
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(h.claude.join("settings.json")).unwrap()).unwrap();
    assert_eq!(v["model"], "mine"); // the user's edit of a group-owned key survives
    assert!(v.get("old").is_none()); // a key the group dropped is removed
}
