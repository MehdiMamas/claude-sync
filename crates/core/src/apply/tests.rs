//! Apply layer tests (issue #16; settings parts of #17). Every test uses its own temp
//! `CLAUDE_HOME` and state dir; the real `~/.claude` is never touched.
use super::*;
use serde_json::json;
use std::time::SystemTime;

struct Env {
    _tmp: tempfile::TempDir,
    paths: Paths,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths {
        claude_home: tmp.path().join("home").join(".claude"),
        state_dir: tmp.path().join("home").join(".claude-sync"),
    };
    Env { _tmp: tmp, paths }
}

fn g(name: &str) -> GroupId {
    GroupId::new("owner", name)
}

/// The order when `g` is the only joined group.
fn only(g: &GroupId) -> &[GroupId] {
    std::slice::from_ref(g)
}

fn set(files: &[(&str, &[u8])]) -> FileSet {
    files
        .iter()
        .map(|(p, b)| (p.to_string(), b.to_vec()))
        .collect()
}

fn home(e: &Env, rel: &str) -> PathBuf {
    target_path(&e.paths.claude_home, rel)
}

fn read(p: &Path) -> Option<Vec<u8>> {
    fs::read(p).ok()
}

fn put(p: &Path, b: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
}

fn mtime(p: &Path) -> SystemTime {
    fs::metadata(p).unwrap().modified().unwrap()
}

fn settings_json(e: &Env) -> Value {
    serde_json::from_slice(&read(&home(e, "settings.json")).unwrap()).unwrap()
}

fn backups(e: &Env) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![e.paths.state_dir.join("backups")];
    while let Some(d) = stack.pop() {
        for ent in fs::read_dir(&d).into_iter().flatten().flatten() {
            if ent.file_type().unwrap().is_dir() {
                stack.push(ent.path());
            } else {
                out.push(ent.path());
            }
        }
    }
    out.sort();
    out
}

fn tree() -> FileSet {
    set(&[
        ("skills/alpha/SKILL.md", b"# alpha\n"),
        ("skills/alpha/scripts/run.sh", b"echo hi\n"),
        ("agents/reviewer.md", b"reviewer\n"),
        ("commands/ship.md", b"ship\n"),
        ("output-styles/terse.md", b"terse\n"),
    ])
}

// ---------------------------------------------------------------- tree files

#[test]
fn fresh_apply_writes_every_file_and_records_it() {
    let e = env();
    let a = g("a");
    let r = apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    assert_eq!(r.collisions, vec![]);
    assert_eq!(r.removed, Vec::<String>::new());
    for (rel, bytes) in tree() {
        assert_eq!(
            read(&home(&e, &rel)).as_deref(),
            Some(bytes.as_slice()),
            "{rel}"
        );
        assert!(r.written.contains(&rel), "{rel}");
    }
    let m = Manifest::load(&e.paths).unwrap();
    assert_eq!(m.files.len(), 5);
    assert_eq!(m.files["agents/reviewer.md"].group, a);
    assert_eq!(
        m.files["agents/reviewer.md"].sha256,
        sha256_hex(b"reviewer\n")
    );
    assert_eq!(m.order, vec![a]);
}

#[test]
fn second_apply_writes_nothing() {
    let e = env();
    let a = g("a");
    let mut files = tree();
    files.insert(CLAUDE_MD.into(), b"group rules\n".to_vec());
    files.insert(
        SHARED_SETTINGS.into(),
        br#"{"model": "opus", "permissions": {"allow": ["Read"]}}"#.to_vec(),
    );
    put(&home(&e, "settings.json"), b"{\"theme\": \"dark\"}\n");
    put(&home(&e, CLAUDE_MD), b"# mine\n");
    apply_group(&e.paths, &a, only(&a), &files).unwrap();

    let watched = [
        home(&e, "skills/alpha/SKILL.md"),
        home(&e, "settings.json"),
        home(&e, CLAUDE_MD),
        e.paths.group_dir(&a).join(CLAUDE_MD),
        e.paths.state_dir.join(MANIFEST_FILE),
        e.paths.state_dir.join(SETTINGS_OWNED_FILE),
    ];
    let times: Vec<SystemTime> = watched.iter().map(|p| mtime(p)).collect();
    let n_backups = backups(&e).len();
    let w = writes();
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert_eq!(writes(), w, "second apply wrote or deleted something");
    assert_eq!(r, ApplyReport::default());
    assert_eq!(watched.iter().map(|p| mtime(p)).collect::<Vec<_>>(), times);
    assert_eq!(backups(&e).len(), n_backups);
}

#[test]
fn update_rewrites_changed_files_only() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    let mut files = tree();
    files.insert("agents/reviewer.md".into(), b"reviewer v2\n".to_vec());
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert_eq!(r.written, vec!["agents/reviewer.md".to_string()]);
    assert_eq!(
        read(&home(&e, "agents/reviewer.md")).unwrap(),
        b"reviewer v2\n"
    );
    let m = Manifest::load(&e.paths).unwrap();
    assert_eq!(
        m.files["agents/reviewer.md"].sha256,
        sha256_hex(b"reviewer v2\n")
    );
}

#[test]
fn dropped_files_are_deleted_and_created_folders_pruned() {
    let e = env();
    let a = g("a");
    put(&home(&e, "skills/mine/SKILL.md"), b"mine\n");
    apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    let files = set(&[("agents/reviewer.md", b"reviewer\n")]);
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    let mut removed = r.removed.clone();
    removed.sort();
    assert_eq!(
        removed,
        vec![
            "commands/ship.md",
            "output-styles/terse.md",
            "skills/alpha/SKILL.md",
            "skills/alpha/scripts/run.sh"
        ]
    );
    assert!(
        !home(&e, "skills/alpha").exists(),
        "created skill folder left behind"
    );
    assert!(home(&e, "skills").is_dir(), "top-level folder removed");
    assert!(home(&e, "commands").is_dir(), "top-level folder removed");
    assert_eq!(read(&home(&e, "skills/mine/SKILL.md")).unwrap(), b"mine\n");
    let m = Manifest::load(&e.paths).unwrap();
    assert_eq!(
        m.files.keys().collect::<Vec<_>>(),
        vec!["agents/reviewer.md"]
    );
    assert!(m.created_dirs.is_empty(), "{:?}", m.created_dirs);
}

#[test]
fn a_dropped_file_the_user_edited_is_kept() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    put(&home(&e, "commands/ship.md"), b"ship, my way\n");
    let mut files = tree();
    files.remove("commands/ship.md");
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert_eq!(r.kept, vec!["commands/ship.md".to_string()]);
    assert!(r.removed.is_empty());
    assert_eq!(
        read(&home(&e, "commands/ship.md")).unwrap(),
        b"ship, my way\n"
    );
    // Still managed, so the edit is offered for sharing.
    assert_eq!(
        local_files(&e.paths, &a).unwrap()["commands/ship.md"],
        b"ship, my way\n"
    );
}

#[test]
fn user_file_in_the_way_is_a_collision_and_untouched() {
    let e = env();
    let a = g("a");
    put(&home(&e, "agents/reviewer.md"), b"my own reviewer\r\n");
    let r = apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    assert_eq!(
        r.collisions,
        vec![Collision::UserFile {
            path: "agents/reviewer.md".into()
        }]
    );
    assert!(!r.written.contains(&"agents/reviewer.md".to_string()));
    assert_eq!(
        read(&home(&e, "agents/reviewer.md")).unwrap(),
        b"my own reviewer\r\n"
    );
    assert!(!Manifest::load(&e.paths)
        .unwrap()
        .files
        .contains_key("agents/reviewer.md"));
    // Not read back as the group's file, and the engine knows to leave it alone.
    assert!(!local_files(&e.paths, &a)
        .unwrap()
        .contains_key("agents/reviewer.md"));
    assert!(excluded_paths(&e.paths, &a)
        .unwrap()
        .contains("agents/reviewer.md"));
    // Removing the group leaves the user's file.
    remove_group(&e.paths, &a).unwrap();
    assert_eq!(
        read(&home(&e, "agents/reviewer.md")).unwrap(),
        b"my own reviewer\r\n"
    );
}

#[test]
fn collision_serializes_with_kind_tag() {
    let c = Collision::OtherGroup {
        path: "agents/x.md".into(),
        owner: g("a"),
    };
    assert_eq!(
        serde_json::to_value(&c).unwrap(),
        json!({"kind": "otherGroup", "path": "agents/x.md", "owner": {"owner": "owner", "repo": "a"}})
    );
}

#[test]
fn two_groups_same_path_first_joined_wins() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let order = [a.clone(), b.clone()];
    apply_group(&e.paths, &a, &order, &set(&[("agents/x.md", b"from a\n")])).unwrap();
    let r = apply_group(
        &e.paths,
        &b,
        &order,
        &set(&[("agents/x.md", b"from b\n"), ("agents/y.md", b"y\n")]),
    )
    .unwrap();
    assert_eq!(
        r.collisions,
        vec![Collision::OtherGroup {
            path: "agents/x.md".into(),
            owner: a.clone()
        }]
    );
    assert_eq!(read(&home(&e, "agents/x.md")).unwrap(), b"from a\n");
    assert_eq!(read(&home(&e, "agents/y.md")).unwrap(), b"y\n");
    assert!(excluded_paths(&e.paths, &b)
        .unwrap()
        .contains("agents/x.md"));
    assert!(!local_files(&e.paths, &b)
        .unwrap()
        .contains_key("agents/x.md"));
    // Re-applying B again is stable and reports the same collision.
    let w = writes();
    let r2 = apply_group(
        &e.paths,
        &b,
        &order,
        &set(&[("agents/x.md", b"from b\n"), ("agents/y.md", b"y\n")]),
    )
    .unwrap();
    assert_eq!(r2.collisions, r.collisions);
    assert_eq!(writes(), w);
}

#[test]
fn earlier_group_applied_later_takes_the_path_over() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let order = [a.clone(), b.clone()];
    // B is applied first (say A's pull failed), then A arrives.
    apply_group(&e.paths, &b, &order, &set(&[("agents/x.md", b"from b\n")])).unwrap();
    let r = apply_group(&e.paths, &a, &order, &set(&[("agents/x.md", b"from a\n")])).unwrap();
    assert!(r.collisions.is_empty());
    assert_eq!(read(&home(&e, "agents/x.md")).unwrap(), b"from a\n");
    assert_eq!(
        Manifest::load(&e.paths).unwrap().files["agents/x.md"].group,
        a
    );
    // B's next apply reports A as the owner and does not delete A's file.
    let r = apply_group(&e.paths, &b, &order, &set(&[("agents/x.md", b"from b\n")])).unwrap();
    assert_eq!(
        r.collisions,
        vec![Collision::OtherGroup {
            path: "agents/x.md".into(),
            owner: a.clone()
        }]
    );
    assert!(r.removed.is_empty());
    assert_eq!(read(&home(&e, "agents/x.md")).unwrap(), b"from a\n");
    // A leaves: the file goes; B gets it on its next apply.
    remove_group(&e.paths, &a).unwrap();
    assert!(!home(&e, "agents/x.md").exists());
    let r = apply_group(
        &e.paths,
        &b,
        only(&b),
        &set(&[("agents/x.md", b"from b\n")]),
    )
    .unwrap();
    assert!(r.collisions.is_empty());
    assert_eq!(read(&home(&e, "agents/x.md")).unwrap(), b"from b\n");
}

#[test]
fn a_later_group_dropping_a_path_does_not_delete_the_winner() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let order = [a.clone(), b.clone()];
    apply_group(&e.paths, &a, &order, &set(&[("agents/x.md", b"from a\n")])).unwrap();
    apply_group(&e.paths, &b, &order, &set(&[("agents/x.md", b"from b\n")])).unwrap();
    let r = apply_group(&e.paths, &b, &order, &FileSet::new()).unwrap();
    assert!(r.removed.is_empty());
    assert_eq!(read(&home(&e, "agents/x.md")).unwrap(), b"from a\n");
}

#[test]
fn unsafe_and_unknown_paths_are_not_applied() {
    let e = env();
    let a = g("a");
    let bad = [
        "skills/../evil.md",
        "skills/./x/SKILL.md",
        "skills//x.md",
        "skills/alpha",
        "skills/a\\b/SKILL.md",
        "skills/c:/SKILL.md",
        "agents/notes.txt",
        "agents/CON.md",
        "agents/nul.md",
        "agents/x.md.",
        "commands/sub /x.md",
        "/agents/x.md",
        "README.md",
        "hooks/run.sh",
        ".github/workflows/ci.yml",
    ];
    let files: FileSet = bad
        .iter()
        .map(|p| (p.to_string(), b"x\n".to_vec()))
        .collect();
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert!(r.written.is_empty(), "{:?}", r.written);
    assert!(!e.paths.claude_home.join("evil.md").exists());
    assert!(!e.paths.claude_home.join("README.md").exists());
    let ex = excluded_paths(&e.paths, &a).unwrap();
    for p in bad {
        assert!(ex.contains(p), "{p}");
    }
    // Nested agents and commands are fine.
    assert!(is_managed_target("commands/git/commit.md"));
    assert!(is_managed_target("skills/alpha/scripts/run.sh"));
    assert!(is_managed_target("agents/COM10.md"));
    assert!(!is_managed_target("agents/COM1.md"));
}

#[test]
fn content_is_copied_byte_for_byte() {
    let e = env();
    let a = g("a");
    let crlf = b"line one\r\nline two\r\n\r\nno newline at end".to_vec();
    let binary: Vec<u8> = (0u8..=255).collect();
    let files = set(&[
        ("skills/alpha/SKILL.md", &crlf),
        ("skills/alpha/icon.bin", &binary),
    ]);
    apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert_eq!(read(&home(&e, "skills/alpha/SKILL.md")).unwrap(), crlf);
    assert_eq!(read(&home(&e, "skills/alpha/icon.bin")).unwrap(), binary);
    assert_eq!(local_files(&e.paths, &a).unwrap(), files);
}

#[test]
fn windows_paths_use_native_separators_on_disk() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    let native = e
        .paths
        .claude_home
        .join("skills")
        .join("alpha")
        .join("scripts")
        .join("run.sh");
    assert_eq!(home(&e, "skills/alpha/scripts/run.sh"), native);
    assert!(native.is_file());
    // Manifest keys stay forward-slash on every OS.
    assert!(Manifest::load(&e.paths)
        .unwrap()
        .files
        .keys()
        .all(|k| !k.contains('\\')));
}

#[test]
fn a_local_edit_is_not_reverted_until_the_group_changes() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    put(&home(&e, "agents/reviewer.md"), b"reviewer, edited\n");
    let r = apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    assert!(r.written.is_empty());
    assert_eq!(
        read(&home(&e, "agents/reviewer.md")).unwrap(),
        b"reviewer, edited\n"
    );
    assert_eq!(
        local_files(&e.paths, &a).unwrap()["agents/reviewer.md"],
        b"reviewer, edited\n"
    );
    // The group ships a new version (after the merge saw the edit): it is written.
    let mut files = tree();
    files.insert("agents/reviewer.md".into(), b"reviewer, merged\n".to_vec());
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert_eq!(r.written, vec!["agents/reviewer.md".to_string()]);
    assert_eq!(
        read(&home(&e, "agents/reviewer.md")).unwrap(),
        b"reviewer, merged\n"
    );
}

#[test]
fn a_locally_deleted_file_is_not_put_back() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    fs::remove_file(home(&e, "commands/ship.md")).unwrap();
    let r = apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    assert!(r.written.is_empty());
    assert!(!home(&e, "commands/ship.md").exists());
    let local = local_files(&e.paths, &a).unwrap();
    assert!(!local.contains_key("commands/ship.md"));
    assert!(!excluded_paths(&e.paths, &a)
        .unwrap()
        .contains("commands/ship.md"));
    assert!(Manifest::load(&e.paths)
        .unwrap()
        .files
        .contains_key("commands/ship.md"));
}

#[test]
fn a_folder_where_a_managed_file_was_is_a_collision() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    fs::remove_file(home(&e, "agents/reviewer.md")).unwrap();
    fs::create_dir(home(&e, "agents/reviewer.md")).unwrap();
    let mut files = tree();
    files.insert("agents/reviewer.md".into(), b"v2\n".to_vec());
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert_eq!(
        r.collisions,
        vec![Collision::UserFile {
            path: "agents/reviewer.md".into()
        }]
    );
    assert!(home(&e, "agents/reviewer.md").is_dir());
}

// ---------------------------------------------------------------- keep local / undo

#[test]
fn keep_local_stops_managing_a_file() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    put(&home(&e, "agents/reviewer.md"), b"mine now\n");
    keep_local(&e.paths, &a, "agents/reviewer.md").unwrap();
    let mut files = tree();
    files.insert("agents/reviewer.md".into(), b"reviewer v2\n".to_vec());
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert!(r.written.is_empty() && r.collisions.is_empty(), "{r:?}");
    assert_eq!(
        read(&home(&e, "agents/reviewer.md")).unwrap(),
        b"mine now\n"
    );
    assert!(!local_files(&e.paths, &a)
        .unwrap()
        .contains_key("agents/reviewer.md"));
    assert!(excluded_paths(&e.paths, &a)
        .unwrap()
        .contains("agents/reviewer.md"));
    // Removing the group leaves it: it is the user's file now.
    remove_group(&e.paths, &a).unwrap();
    assert_eq!(
        read(&home(&e, "agents/reviewer.md")).unwrap(),
        b"mine now\n"
    );
}

#[test]
fn keep_local_rejects_unmanaged_paths() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    apply_group(&e.paths, &a, &[a.clone(), b.clone()], &tree()).unwrap();
    assert!(matches!(
        keep_local(&e.paths, &a, "agents/none.md"),
        Err(crate::Error::Invalid(_))
    ));
    assert!(matches!(
        keep_local(&e.paths, &b, "agents/reviewer.md"),
        Err(crate::Error::Invalid(_))
    ));
}

#[test]
fn undo_puts_the_group_version_back() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    put(&home(&e, "agents/reviewer.md"), b"oops\n");
    undo(&e.paths, &a, "agents/reviewer.md", Some(b"reviewer\n")).unwrap();
    assert_eq!(
        read(&home(&e, "agents/reviewer.md")).unwrap(),
        b"reviewer\n"
    );
    // A deleted file comes back.
    fs::remove_file(home(&e, "skills/alpha/scripts/run.sh")).unwrap();
    fs::remove_dir(home(&e, "skills/alpha/scripts")).unwrap();
    undo(
        &e.paths,
        &a,
        "skills/alpha/scripts/run.sh",
        Some(b"echo hi\n"),
    )
    .unwrap();
    assert_eq!(
        read(&home(&e, "skills/alpha/scripts/run.sh")).unwrap(),
        b"echo hi\n"
    );
    // Group no longer has it: undo deletes the local copy.
    undo(&e.paths, &a, "commands/ship.md", None).unwrap();
    assert!(!home(&e, "commands/ship.md").exists());
    assert!(!Manifest::load(&e.paths)
        .unwrap()
        .files
        .contains_key("commands/ship.md"));
    let w = writes();
    apply_group(&e.paths, &a, only(&a), &tree()).unwrap();
    // The next apply sees nothing to do for the undone files except re-adding ship.md.
    assert_eq!(read(&home(&e, "commands/ship.md")).unwrap(), b"ship\n");
    assert!(writes() > w);
    assert!(matches!(
        undo(&e.paths, &a, "agents/none.md", None),
        Err(crate::Error::Invalid(_))
    ));
}

// ---------------------------------------------------------------- CLAUDE.md

fn block(lines: &[&str], eol: &str) -> String {
    let mut s = format!("{BLOCK_BEGIN}{eol}");
    for l in lines {
        s.push_str(l);
        s.push_str(eol);
    }
    s.push_str(BLOCK_END);
    s.push_str(eol);
    s
}

#[test]
fn render_inserts_updates_and_removes_the_block() {
    let user = b"# My rules\r\n\r\nBe brief.\r\n".to_vec();
    let one = render_claude_md(&user, &["@~/a".into()]);
    assert_eq!(
        String::from_utf8(one.clone()).unwrap(),
        format!(
            "{}\r\n# My rules\r\n\r\nBe brief.\r\n",
            block(&["@~/a"], "\r\n")
        )
    );
    let two = render_claude_md(&one, &["@~/a".into(), "@~/b".into()]);
    assert_eq!(
        String::from_utf8(two.clone()).unwrap(),
        format!(
            "{}\r\n# My rules\r\n\r\nBe brief.\r\n",
            block(&["@~/a", "@~/b"], "\r\n")
        )
    );
    assert_eq!(
        render_claude_md(&two, &[]),
        user,
        "removal must restore the file byte for byte"
    );
    // Empty or missing file: only the block, LF.
    assert_eq!(
        String::from_utf8(render_claude_md(b"", &["@~/a".into()])).unwrap(),
        block(&["@~/a"], "\n")
    );
    assert_eq!(
        render_claude_md(block(&["@~/a"], "\n").as_bytes(), &[]),
        b""
    );
    // No block and no lines: untouched.
    assert_eq!(render_claude_md(&user, &[]), user);
}

#[test]
fn render_updates_a_block_the_user_moved() {
    let text = format!("intro\n\n{}tail\n", block(&["@~/old"], "\n"));
    let out = render_claude_md(text.as_bytes(), &["@~/new".into()]);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        format!("intro\n\n{}tail\n", block(&["@~/new"], "\n"))
    );
    let removed = render_claude_md(text.as_bytes(), &[]);
    assert_eq!(String::from_utf8(removed).unwrap(), "intro\n\ntail\n");
    // A marker quoted mid-line is not the block.
    let quoted = format!("see `{BLOCK_BEGIN}` here\n");
    let out = render_claude_md(quoted.as_bytes(), &["@~/a".into()]);
    assert!(String::from_utf8(out)
        .unwrap()
        .ends_with(&format!("\n{quoted}")));
}

#[test]
fn import_line_is_home_relative_with_forward_slashes() {
    let home = Path::new("/home/me");
    let file = home
        .join(".claude-sync")
        .join("groups")
        .join("ana__claude-sync-data")
        .join(CLAUDE_MD);
    assert_eq!(
        import_line(&file, Some(home)),
        "@~/.claude-sync/groups/ana__claude-sync-data/CLAUDE.md"
    );
    assert_eq!(
        import_line(Path::new("/srv/state/CLAUDE.md"), Some(home)),
        "@/srv/state/CLAUDE.md"
    );
    assert_eq!(
        import_line(Path::new("/srv/state/CLAUDE.md"), None),
        "@/srv/state/CLAUDE.md"
    );
    #[cfg(windows)]
    {
        let home = Path::new(r"C:\Users\me");
        let file = Path::new(r"C:\Users\me\.claude-sync\groups\o__r\CLAUDE.md");
        assert_eq!(
            import_line(file, Some(home)),
            "@~/.claude-sync/groups/o__r/CLAUDE.md"
        );
        assert_eq!(
            import_line(Path::new(r"D:\state\CLAUDE.md"), Some(home)),
            "@D:/state/CLAUDE.md"
        );
    }
}

#[test]
fn claude_md_copy_and_block_lifecycle() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let order = [a.clone(), b.clone()];
    let user = b"# Mine\r\nkeep this\r\n".to_vec();
    put(&home(&e, CLAUDE_MD), &user);
    let la = import_line(
        &e.paths.group_dir(&a).join(CLAUDE_MD),
        home_dir().as_deref(),
    );
    let lb = import_line(
        &e.paths.group_dir(&b).join(CLAUDE_MD),
        home_dir().as_deref(),
    );

    let r = apply_group(&e.paths, &a, &order, &set(&[(CLAUDE_MD, b"A rules\n")])).unwrap();
    assert!(r.written.contains(&CLAUDE_MD.to_string()));
    assert_eq!(
        read(&e.paths.group_dir(&a).join(CLAUDE_MD)).unwrap(),
        b"A rules\n"
    );
    let expect_a = format!("{}\r\n# Mine\r\nkeep this\r\n", block(&[&la], "\r\n"));
    assert_eq!(
        String::from_utf8(read(&home(&e, CLAUDE_MD)).unwrap()).unwrap(),
        expect_a
    );
    // The first change to the user's CLAUDE.md is backed up.
    assert!(backups(&e)
        .iter()
        .any(|p| p.ends_with(CLAUDE_MD) && read(p).unwrap() == user));

    // B is applied before A in time but after it in the order: lines follow the order.
    apply_group(&e.paths, &b, &order, &set(&[(CLAUDE_MD, b"B rules\n")])).unwrap();
    let expect_ab = format!("{}\r\n# Mine\r\nkeep this\r\n", block(&[&la, &lb], "\r\n"));
    assert_eq!(
        String::from_utf8(read(&home(&e, CLAUDE_MD)).unwrap()).unwrap(),
        expect_ab
    );

    // Update A's text: only the copy changes.
    let before = read(&home(&e, CLAUDE_MD)).unwrap();
    apply_group(&e.paths, &a, &order, &set(&[(CLAUDE_MD, b"A rules v2\n")])).unwrap();
    assert_eq!(
        read(&e.paths.group_dir(&a).join(CLAUDE_MD)).unwrap(),
        b"A rules v2\n"
    );
    assert_eq!(read(&home(&e, CLAUDE_MD)).unwrap(), before);

    // A stops shipping CLAUDE.md: its line goes.
    let r = apply_group(&e.paths, &a, &order, &FileSet::new()).unwrap();
    assert!(r.removed.contains(&CLAUDE_MD.to_string()));
    assert!(!e.paths.group_dir(&a).join(CLAUDE_MD).exists());
    let expect_b = format!("{}\r\n# Mine\r\nkeep this\r\n", block(&[&lb], "\r\n"));
    assert_eq!(
        String::from_utf8(read(&home(&e, CLAUDE_MD)).unwrap()).unwrap(),
        expect_b
    );

    // Last group removed: the block and its blank line go, the user's file is exact again.
    remove_group(&e.paths, &b).unwrap();
    assert_eq!(read(&home(&e, CLAUDE_MD)).unwrap(), user);
    assert!(!e.paths.group_dir(&b).join(CLAUDE_MD).exists());
}

#[test]
fn claude_md_created_when_missing_and_deleted_again() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &set(&[(CLAUDE_MD, b"A\n")])).unwrap();
    let text = String::from_utf8(read(&home(&e, CLAUDE_MD)).unwrap()).unwrap();
    assert!(
        text.starts_with(BLOCK_BEGIN) && text.ends_with(&format!("{BLOCK_END}\n")),
        "{text}"
    );
    assert!(Manifest::load(&e.paths).unwrap().created_claude_md);
    remove_group(&e.paths, &a).unwrap();
    assert!(!home(&e, CLAUDE_MD).exists());
}

#[test]
fn claude_md_the_user_added_to_is_kept_after_the_block_goes() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &set(&[(CLAUDE_MD, b"A\n")])).unwrap();
    let mut text = read(&home(&e, CLAUDE_MD)).unwrap();
    text.extend_from_slice(b"\nmy own line\n");
    put(&home(&e, CLAUDE_MD), &text);
    remove_group(&e.paths, &a).unwrap();
    // The blank line after the block is the separator the app adds; it goes with it.
    assert_eq!(read(&home(&e, CLAUDE_MD)).unwrap(), b"my own line\n");
    assert!(!Manifest::load(&e.paths).unwrap().created_claude_md);
}

#[test]
fn edited_claude_md_copy_is_a_local_edit() {
    let e = env();
    let a = g("a");
    let files = set(&[(CLAUDE_MD, b"A\r\n")]);
    apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert_eq!(local_files(&e.paths, &a).unwrap()[CLAUDE_MD], b"A\r\n");
    let copy = e.paths.group_dir(&a).join(CLAUDE_MD);
    put(&copy, b"A, edited\r\n");
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert!(r.written.is_empty());
    assert_eq!(read(&copy).unwrap(), b"A, edited\r\n");
    assert_eq!(
        local_files(&e.paths, &a).unwrap()[CLAUDE_MD],
        b"A, edited\r\n"
    );
    undo(&e.paths, &a, CLAUDE_MD, Some(b"A\r\n")).unwrap();
    assert_eq!(read(&copy).unwrap(), b"A\r\n");
    // Keep local: the copy is no longer updated, but stays imported.
    keep_local(&e.paths, &a, CLAUDE_MD).unwrap();
    apply_group(&e.paths, &a, only(&a), &set(&[(CLAUDE_MD, b"A v2\n")])).unwrap();
    assert_eq!(read(&copy).unwrap(), b"A\r\n");
    assert!(!local_files(&e.paths, &a).unwrap().contains_key(CLAUDE_MD));
    assert!(String::from_utf8(read(&home(&e, CLAUDE_MD)).unwrap())
        .unwrap()
        .contains("a/CLAUDE.md"));
}

// ---------------------------------------------------------------- settings

fn shared(v: Value) -> FileSet {
    let mut f = FileSet::new();
    f.insert(
        SHARED_SETTINGS.into(),
        serde_json::to_vec_pretty(&v).unwrap(),
    );
    f
}

#[test]
fn settings_are_merged_with_a_backup_and_ownership() {
    let e = env();
    let a = g("a");
    let original = b"{\r\n  \"theme\": \"dark\",\r\n  \"model\": \"sonnet\"\r\n}\r\n".to_vec();
    put(&home(&e, "settings.json"), &original);
    let r = apply_group(
        &e.paths,
        &a,
        only(&a),
        &shared(json!({"model": "opus", "permissions": {"allow": ["Read"]}})),
    )
    .unwrap();
    assert!(r.written.contains(&"settings.json".to_string()));
    assert_eq!(
        settings_json(&e),
        json!({"theme": "dark", "model": "opus", "permissions": {"allow": ["Read"]}})
    );
    assert!(backups(&e)
        .iter()
        .any(|p| p.ends_with("settings.json") && read(p).unwrap() == original));
    let owned = load_owned(&e.paths).unwrap();
    assert_eq!(owned["owner/a"].values["/model"], json!("opus"));
    assert_eq!(
        owned["owner/a"].array_items["/permissions/allow"],
        vec![json!("Read")]
    );
    // The applied copy is stored byte for byte.
    assert_eq!(
        read(&e.paths.group_dir(&a).join(SHARED_SETTINGS)).unwrap(),
        shared(json!({"model": "opus", "permissions": {"allow": ["Read"]}}))[SHARED_SETTINGS]
    );
}

#[test]
fn settings_created_when_missing() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &shared(json!({"model": "opus"}))).unwrap();
    assert_eq!(settings_json(&e), json!({"model": "opus"}));
    assert!(backups(&e).is_empty(), "nothing to back up");
}

#[test]
fn invalid_settings_on_either_side_write_nothing() {
    let e = env();
    let a = g("a");
    put(&home(&e, "settings.json"), b"{ not json");
    let r = apply_group(&e.paths, &a, only(&a), &shared(json!({"model": "opus"}))).unwrap();
    assert_eq!(read(&home(&e, "settings.json")).unwrap(), b"{ not json");
    assert!(
        r.settings_skipped
            .iter()
            .any(|s| s.contains("settings.json is not one JSON object")),
        "{r:?}"
    );

    put(&home(&e, "settings.json"), b"{\"theme\": \"dark\"}\n");
    let mut files = FileSet::new();
    files.insert(SHARED_SETTINGS.into(), b"[1, 2]".to_vec());
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert_eq!(
        read(&home(&e, "settings.json")).unwrap(),
        b"{\"theme\": \"dark\"}\n"
    );
    assert!(
        r.settings_skipped
            .iter()
            .any(|s| s.contains("settings.shared.json is not one JSON object")),
        "{r:?}"
    );
    // local_files returns the invalid copy unchanged, so the merge sees no local edit.
    assert_eq!(
        local_files(&e.paths, &a).unwrap()[SHARED_SETTINGS],
        b"[1, 2]"
    );
}

#[test]
fn settings_group_removes_a_key_and_an_array_item() {
    let e = env();
    let a = g("a");
    put(
        &home(&e, "settings.json"),
        b"{\"theme\": \"dark\", \"permissions\": {\"allow\": [\"Mine\"]}}",
    );
    apply_group(
        &e.paths,
        &a,
        only(&a),
        &shared(json!({"model": "opus", "permissions": {"allow": ["Read", "Edit"]}})),
    )
    .unwrap();
    apply_group(
        &e.paths,
        &a,
        only(&a),
        &shared(json!({"permissions": {"allow": ["Edit"]}})),
    )
    .unwrap();
    assert_eq!(
        settings_json(&e),
        json!({"theme": "dark", "permissions": {"allow": ["Mine", "Edit"]}})
    );
    // The group stops shipping settings: what it set goes, the user's stays.
    let r = apply_group(&e.paths, &a, only(&a), &FileSet::new()).unwrap();
    assert!(r.written.contains(&"settings.json".to_string()));
    assert_eq!(
        settings_json(&e),
        json!({"theme": "dark", "permissions": {"allow": ["Mine"]}})
    );
    assert!(!load_owned(&e.paths).unwrap().contains_key("owner/a"));
    assert!(!e.paths.group_dir(&a).join(SHARED_SETTINGS).exists());
}

#[test]
fn settings_value_the_user_changed_is_kept_and_read_back() {
    let e = env();
    let a = g("a");
    let files = shared(json!({"model": "opus", "theme": "light"}));
    apply_group(&e.paths, &a, only(&a), &files).unwrap();
    // No edit: local_files returns the applied copy exactly.
    assert_eq!(
        local_files(&e.paths, &a).unwrap()[SHARED_SETTINGS],
        files[SHARED_SETTINGS]
    );
    let mut s = settings_json(&e);
    s["model"] = json!("haiku");
    s.as_object_mut().unwrap().remove("theme");
    s["mine"] = json!(1);
    put(&home(&e, "settings.json"), &settings::to_bytes(&s));
    let w = writes();
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert!(!r.written.contains(&"settings.json".to_string()));
    assert_eq!(settings_json(&e), json!({"model": "haiku", "mine": 1}));
    assert_eq!(writes(), w);
    let back: Value =
        serde_json::from_slice(&local_files(&e.paths, &a).unwrap()[SHARED_SETTINGS]).unwrap();
    assert_eq!(
        back,
        json!({"model": "haiku"}),
        "edited value carried, deleted one dropped, personal key left out"
    );
    // Undo one value, then all.
    undo(&e.paths, &a, "settings.shared.json#/model", None).unwrap();
    assert_eq!(settings_json(&e), json!({"model": "opus", "mine": 1}));
    undo(&e.paths, &a, SHARED_SETTINGS, None).unwrap();
    assert_eq!(
        settings_json(&e),
        json!({"model": "opus", "mine": 1, "theme": "light"})
    );
}

#[test]
fn settings_keep_local_pointer_is_not_applied_again() {
    let e = env();
    let a = g("a");
    apply_group(
        &e.paths,
        &a,
        only(&a),
        &shared(json!({"model": "opus", "theme": "light"})),
    )
    .unwrap();
    let mut s = settings_json(&e);
    s["model"] = json!("haiku");
    put(&home(&e, "settings.json"), &settings::to_bytes(&s));
    keep_local(&e.paths, &a, "settings.shared.json#/model").unwrap();
    let r = apply_group(
        &e.paths,
        &a,
        only(&a),
        &shared(json!({"model": "fable", "theme": "light"})),
    )
    .unwrap();
    assert_eq!(settings_json(&e)["model"], json!("haiku"));
    assert!(
        r.settings_skipped
            .iter()
            .any(|m| m == "model: kept local on this device; not applied"),
        "{r:?}"
    );
    assert!(!load_owned(&e.paths).unwrap()["owner/a"]
        .values
        .contains_key("/model"));
    // Removing the group leaves the kept value.
    remove_group(&e.paths, &a).unwrap();
    assert_eq!(settings_json(&e), json!({"model": "haiku"}));
}

#[test]
fn settings_keep_local_all() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &shared(json!({"model": "opus"}))).unwrap();
    keep_local(&e.paths, &a, SHARED_SETTINGS).unwrap();
    apply_group(&e.paths, &a, only(&a), &shared(json!({"model": "fable"}))).unwrap();
    assert_eq!(settings_json(&e), json!({"model": "opus"}));
    assert!(!local_files(&e.paths, &a)
        .unwrap()
        .contains_key(SHARED_SETTINGS));
    assert!(excluded_paths(&e.paths, &a)
        .unwrap()
        .contains(SHARED_SETTINGS));
}

#[test]
fn settings_first_joined_group_wins_a_key() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let order = [a.clone(), b.clone()];
    apply_group(&e.paths, &a, &order, &shared(json!({"model": "opus"}))).unwrap();
    let r = apply_group(
        &e.paths,
        &b,
        &order,
        &shared(json!({"model": "haiku", "theme": "light"})),
    )
    .unwrap();
    assert_eq!(
        settings_json(&e),
        json!({"model": "opus", "theme": "light"})
    );
    assert!(
        r.settings_skipped
            .iter()
            .any(|m| m == "model: set by group owner/a; not applied"),
        "{r:?}"
    );
    // Removing B removes only what B set.
    remove_group(&e.paths, &b).unwrap();
    assert_eq!(settings_json(&e), json!({"model": "opus"}));
}

#[test]
fn settings_blocked_keys_are_reported_not_applied() {
    let e = env();
    let a = g("a");
    let token = format!("ghp_{}", "a".repeat(36));
    let r = apply_group(
        &e.paths,
        &a,
        only(&a),
        &shared(json!({
            "apiKeyHelper": "/bin/get-key",
            "env": {"GH": token, "OK": "1"},
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/usr/local/bin/x"}]}]},
            "model": "opus"
        })),
    )
    .unwrap();
    let s = settings_json(&e);
    assert_eq!(s, json!({"env": {"OK": "1"}, "model": "opus"}));
    assert!(r.settings_skipped.len() >= 3, "{r:?}");
    assert!(
        r.settings_skipped.iter().all(|m| !m.contains(&token)),
        "messages must not show values"
    );
}

#[test]
fn settings_json_that_is_a_folder_is_left_alone() {
    let e = env();
    let a = g("a");
    fs::create_dir_all(home(&e, "settings.json")).unwrap();
    let r = apply_group(&e.paths, &a, only(&a), &shared(json!({"model": "opus"}))).unwrap();
    assert!(home(&e, "settings.json").is_dir());
    assert!(
        r.settings_skipped
            .iter()
            .any(|m| m.contains("not a regular file")),
        "{r:?}"
    );
}

#[cfg(unix)]
#[test]
fn links_are_never_written_through() {
    let e = env();
    let a = g("a");
    let outside = e._tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(home(&e, "skills")).unwrap();
    std::os::unix::fs::symlink(&outside, home(&e, "skills/alpha")).unwrap();
    let real = e._tmp.path().join("real.json");
    fs::write(&real, b"{}").unwrap();
    std::os::unix::fs::symlink(&real, home(&e, "settings.json")).unwrap();
    let mut files = tree();
    files.insert(SHARED_SETTINGS.into(), b"{\"model\": \"opus\"}".to_vec());
    let r = apply_group(&e.paths, &a, only(&a), &files).unwrap();
    assert!(r.collisions.contains(&Collision::UserFile {
        path: "skills/alpha/SKILL.md".into()
    }));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    assert_eq!(fs::read(&real).unwrap(), b"{}");
    assert!(r.settings_skipped.iter().any(|m| m.contains("link")));
}

// ---------------------------------------------------------------- remove_group

#[test]
fn remove_group_backs_up_and_removes_only_unchanged_files() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let order = [a.clone(), b.clone()];
    apply_group(&e.paths, &a, &order, &tree()).unwrap();
    apply_group(&e.paths, &b, &order, &set(&[("agents/other.md", b"b\n")])).unwrap();
    put(&home(&e, "commands/ship.md"), b"edited\n");
    let r = remove_group(&e.paths, &a).unwrap();
    assert_eq!(r.kept, vec!["commands/ship.md".to_string()]);
    assert_eq!(r.removed.len(), 4, "{r:?}");
    assert_eq!(read(&home(&e, "commands/ship.md")).unwrap(), b"edited\n");
    assert!(!home(&e, "skills/alpha").exists());
    assert_eq!(read(&home(&e, "agents/other.md")).unwrap(), b"b\n");
    let saved = backups(&e);
    assert!(
        saved
            .iter()
            .any(|p| p.ends_with(Path::new("owner__a/agents/reviewer.md"))),
        "{saved:?}"
    );
    assert!(
        saved
            .iter()
            .any(|p| p.ends_with(Path::new("owner__a/commands/ship.md"))),
        "{saved:?}"
    );
    let m = Manifest::load(&e.paths).unwrap();
    assert!(m.files.values().all(|f| f.group == b));
    assert!(!m.groups.contains_key("owner/a"));
    assert_eq!(m.order, vec![b.clone()]);
    // Removing again is a no-op.
    let w = writes();
    assert_eq!(remove_group(&e.paths, &a).unwrap(), ApplyReport::default());
    assert_eq!(writes(), w);
}

#[test]
fn remove_group_keeps_the_engine_base_folder() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &set(&[(CLAUDE_MD, b"A\n")])).unwrap();
    put(&e.paths.group_dir(&a).join("base").join("x"), b"base\n");
    remove_group(&e.paths, &a).unwrap();
    assert!(e.paths.group_dir(&a).join("base").join("x").exists());
}

// ---------------------------------------------------------------- helpers

#[test]
fn utc_stamp_has_no_colons() {
    let s = utc_stamp();
    assert_eq!(s.len(), 16, "{s}");
    assert!(
        s.starts_with("20") && s.ends_with('Z') && s.as_bytes()[8] == b'T',
        "{s}"
    );
    assert!(!s.contains(':'));
}

#[test]
fn backups_in_one_operation_share_a_folder_and_never_overwrite() {
    let e = env();
    let mut b1 = Backups::new(&e.paths);
    b1.save("settings.json", b"one").unwrap();
    b1.save("CLAUDE.md", b"two").unwrap();
    let mut b2 = Backups::new(&e.paths);
    b2.save("settings.json", b"three").unwrap();
    let all = backups(&e);
    assert_eq!(all.len(), 3, "{all:?}");
    assert_eq!(
        b1.dir.as_ref().unwrap(),
        &all[0].parent().unwrap().to_path_buf()
    );
    assert_ne!(b1.dir, b2.dir);
}

#[test]
fn sha256_matches_a_known_vector() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn write_atomic_leaves_no_temp_file() {
    let e = env();
    let p = e.paths.state_dir.join("x.json");
    write_atomic(&p, b"1").unwrap();
    write_atomic(&p, b"2").unwrap();
    assert_eq!(read(&p).unwrap(), b"2");
    assert_eq!(fs::read_dir(&e.paths.state_dir).unwrap().count(), 1);
}

// ---------------------------------------------------------------- plugins (§4, issue #30)

const MP_JSON: &str = "plugins/mods/.claude-plugin/marketplace.json";
const PLUGIN_JSON: &str = "plugins/mods/mods/router/.claude-plugin/plugin.json";
const HOOKS: &str = "plugins/mods/mods/router/hooks/hooks.json";

fn mp_files() -> FileSet {
    set(&[
        (
            MP_JSON,
            br#"{"name":"mods","plugins":[{"name":"router","source":"./mods/router"}]}"#,
        ),
        (PLUGIN_JSON, br#"{"name":"router"}"#),
        (HOOKS, b"{\"hooks\":{}}\n"),
        ("plugins/mods/.gitignore", b"node_modules/\n"),
        ("agents/reviewer.md", b"reviewer\n"),
    ])
}

fn mp_dir(e: &Env) -> PathBuf {
    e.paths.state_dir.join("marketplaces").join("mods")
}

fn mp_source(e: &Env) -> Value {
    json!({"source": {"source": "directory", "path": mp_dir(e).to_string_lossy()}})
}

#[test]
fn plugins_are_placed_under_the_state_dir_and_registered() {
    let e = env();
    let a = g("a");
    let r = apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    assert!(r.plugins_changed);
    assert!(r.skipped.is_empty(), "{:?}", r.skipped);
    assert_eq!(
        read(&mp_dir(&e).join("mods/router/hooks/hooks.json")).unwrap(),
        b"{\"hooks\":{}}\n"
    );
    assert!(mp_dir(&e).join(".gitignore").exists());
    assert!(mp_dir(&e).join(".claude-plugin/marketplace.json").exists());
    assert!(!e.paths.claude_home.join("plugins").exists());
    assert!(home(&e, "agents/reviewer.md").exists());
    assert_eq!(
        settings_json(&e)["extraKnownMarketplaces"]["mods"],
        mp_source(&e)
    );
    let m = Manifest::load(&e.paths).unwrap();
    assert_eq!(m.files[HOOKS].group, a);
    assert_eq!(m.marketplaces["mods"].group, a);
    assert_eq!(m.marketplaces["mods"].setting, Some(mp_source(&e)));
    assert!(target_of(&e.paths, HOOKS).ends_with("marketplaces/mods/mods/router/hooks/hooks.json"));
    assert!(crate::watch::local_edits(&e.paths).unwrap().is_empty());

    // Idempotent: nothing written, nothing to reload.
    let before = writes();
    let r = apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    assert_eq!(writes(), before);
    assert!(!r.plugins_changed);
    assert!(r.written.is_empty());
}

#[test]
fn a_changed_plugin_file_asks_for_a_reload() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    let mut f = mp_files();
    f.insert(HOOKS.into(), b"{\"hooks\":{\"v\":2}}\n".to_vec());
    let r = apply_group(&e.paths, &a, only(&a), &f).unwrap();
    assert_eq!(r.written, vec![HOOKS.to_string()]);
    assert!(r.plugins_changed);
    // A change outside plugins does not.
    f.insert("agents/reviewer.md".into(), b"v2\n".to_vec());
    let r = apply_group(&e.paths, &a, only(&a), &f).unwrap();
    assert!(!r.plugins_changed);
}

#[test]
fn a_dropped_marketplace_is_removed_and_unregistered() {
    let e = env();
    let a = g("a");
    put(&home(&e, "settings.json"), br#"{"theme": "dark"}"#);
    apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    let r = apply_group(
        &e.paths,
        &a,
        only(&a),
        &set(&[("agents/reviewer.md", b"reviewer\n")]),
    )
    .unwrap();
    assert!(r.plugins_changed);
    assert!(r.removed.contains(&HOOKS.to_string()));
    assert!(!mp_dir(&e).exists(), "created folders are pruned");
    assert_eq!(settings_json(&e), json!({"theme": "dark"}));
    let m = Manifest::load(&e.paths).unwrap();
    assert!(m.marketplaces.is_empty());
    assert!(!m.files.keys().any(|p| p.starts_with("plugins/")));
}

#[test]
fn removing_the_group_removes_its_marketplace() {
    let e = env();
    let a = g("a");
    put(&home(&e, "settings.json"), br#"{"theme": "dark"}"#);
    apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    let r = remove_group(&e.paths, &a).unwrap();
    assert!(r.plugins_changed);
    assert!(!mp_dir(&e).exists());
    assert_eq!(settings_json(&e), json!({"theme": "dark"}));
    assert!(Manifest::load(&e.paths).unwrap().marketplaces.is_empty());
}

#[test]
fn an_existing_local_entry_is_replaced_with_a_backup() {
    let e = env();
    let a = g("a");
    let old = br#"{"extraKnownMarketplaces": {"mods": {"source": {"source": "directory", "path": "C:/Users/me/Projects/mods"}}, "other": {"source": {"source": "github", "repo": "o/r"}}}}"#;
    put(&home(&e, "settings.json"), old);
    let r = apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    let s = settings_json(&e);
    assert_eq!(s["extraKnownMarketplaces"]["mods"], mp_source(&e));
    assert_eq!(
        s["extraKnownMarketplaces"]["other"]["source"]["repo"],
        "o/r"
    );
    assert!(
        r.skipped
            .iter()
            .any(|n| n.contains("extraKnownMarketplaces.mods")),
        "{:?}",
        r.skipped
    );
    let saved: Vec<_> = backups(&e)
        .into_iter()
        .filter(|p| p.ends_with("settings.json"))
        .collect();
    assert_eq!(saved.len(), 1);
    assert_eq!(read(&saved[0]).unwrap(), old);
    // Leaving removes the app's entry and keeps the user's other one.
    remove_group(&e.paths, &a).unwrap();
    let s = settings_json(&e);
    assert_eq!(s["extraKnownMarketplaces"].get("mods"), None);
    assert_eq!(
        s["extraKnownMarketplaces"]["other"]["source"]["repo"],
        "o/r"
    );
}

#[test]
fn an_entry_the_user_changed_after_the_app_set_it_stays_on_removal() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    let mine = json!({"source": {"source": "github", "repo": "me/mods"}});
    put(
        &home(&e, "settings.json"),
        &settings::to_bytes(&json!({"extraKnownMarketplaces": {"mods": mine}})),
    );
    remove_group(&e.paths, &a).unwrap();
    assert_eq!(settings_json(&e)["extraKnownMarketplaces"]["mods"], mine);
}

#[test]
fn the_same_marketplace_in_two_groups_higher_group_wins() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let order = [a.clone(), b.clone()];
    apply_group(&e.paths, &a, &order, &mp_files()).unwrap();
    let mut fb = mp_files();
    fb.insert("plugins/mods/only-in-b.txt".into(), b"b\n".to_vec());
    fb.insert(HOOKS.into(), b"{\"from\":\"b\"}\n".to_vec());
    let r = apply_group(&e.paths, &b, &order, &fb).unwrap();
    assert_eq!(
        r.collisions,
        vec![
            Collision::OtherGroup {
                path: "plugins/mods".into(),
                owner: a.clone()
            },
            Collision::OtherGroup {
                path: "agents/reviewer.md".into(),
                owner: a.clone()
            }
        ]
    );
    assert!(!r.plugins_changed);
    assert!(!mp_dir(&e).join("only-in-b.txt").exists());
    assert_eq!(
        read(&mp_dir(&e).join("mods/router/hooks/hooks.json")).unwrap(),
        b"{\"hooks\":{}}\n"
    );
    let excluded = excluded_paths(&e.paths, &b).unwrap();
    assert!(excluded.contains(HOOKS) && excluded.contains("plugins/mods/only-in-b.txt"));
    assert!(local_files(&e.paths, &b)
        .unwrap()
        .keys()
        .all(|p| !p.starts_with("plugins/")));

    // Once a leaves, b's next apply takes the marketplace.
    remove_group(&e.paths, &a).unwrap();
    let r = apply_group(&e.paths, &b, only(&b), &fb).unwrap();
    assert!(r.collisions.is_empty(), "{:?}", r.collisions);
    assert!(r.plugins_changed);
    assert_eq!(read(&mp_dir(&e).join("only-in-b.txt")).unwrap(), b"b\n");
    assert_eq!(
        settings_json(&e)["extraKnownMarketplaces"]["mods"],
        mp_source(&e)
    );
    assert_eq!(
        Manifest::load(&e.paths).unwrap().marketplaces["mods"].group,
        b
    );
}

#[test]
fn the_bundled_marketplace_wins_over_shared_settings() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let shared = br#"{"extraKnownMarketplaces": {"mods": {"source": {"source": "github", "repo": "o/mods"}}, "other": {"source": {"source": "github", "repo": "o/other"}}}, "enabledPlugins": {"router@mods": true}}"#;
    let mut f = mp_files();
    f.insert(SHARED_SETTINGS.into(), shared.to_vec());
    let r = apply_group(&e.paths, &a, only(&a), &f).unwrap();
    let s = settings_json(&e);
    assert_eq!(s["extraKnownMarketplaces"]["mods"], mp_source(&e));
    assert_eq!(
        s["extraKnownMarketplaces"]["other"]["source"]["repo"],
        "o/other"
    );
    assert_eq!(s["enabledPlugins"]["router@mods"], true);
    assert!(
        r.settings_skipped
            .iter()
            .any(|n| n.contains("extraKnownMarketplaces.mods") && n.contains("plugins/mods")),
        "{:?}",
        r.settings_skipped
    );
    assert!(crate::watch::local_edits(&e.paths).unwrap().is_empty());
    assert_eq!(
        local_files(&e.paths, &a).unwrap()[SHARED_SETTINGS],
        shared.to_vec()
    );
    let before = writes();
    apply_group(&e.paths, &a, only(&a), &f).unwrap();
    assert_eq!(writes(), before);

    // Another group's settings.shared.json naming the same marketplace loses too.
    let order = [a.clone(), b.clone()];
    let rb = apply_group(
        &e.paths,
        &b,
        &order,
        &set(&[(SHARED_SETTINGS, br#"{"extraKnownMarketplaces": {"mods": {"source": {"source": "github", "repo": "x/y"}}}}"#)]),
    )
    .unwrap();
    assert_eq!(
        settings_json(&e)["extraKnownMarketplaces"]["mods"],
        mp_source(&e)
    );
    assert!(
        rb.settings_skipped
            .iter()
            .any(|n| n.contains("plugins/mods")),
        "{:?}",
        rb.settings_skipped
    );
}

#[test]
fn a_group_that_set_the_entry_through_shared_settings_hands_it_over() {
    let e = env();
    let a = g("a");
    let shared = br#"{"extraKnownMarketplaces": {"mods": {"source": {"source": "github", "repo": "o/mods"}}}}"#;
    apply_group(&e.paths, &a, only(&a), &set(&[(SHARED_SETTINGS, shared)])).unwrap();
    assert_eq!(
        settings_json(&e)["extraKnownMarketplaces"]["mods"]["source"]["repo"],
        "o/mods"
    );
    // The group now bundles the marketplace instead.
    let mut f = mp_files();
    f.insert(SHARED_SETTINGS.into(), shared.to_vec());
    apply_group(&e.paths, &a, only(&a), &f).unwrap();
    assert_eq!(
        settings_json(&e)["extraKnownMarketplaces"]["mods"],
        mp_source(&e)
    );
    let owned = load_owned(&e.paths).unwrap();
    assert!(owned.values().all(|o| !o
        .values
        .keys()
        .any(|p| p.starts_with("/extraKnownMarketplaces/mods"))));
    assert!(crate::watch::local_edits(&e.paths).unwrap().is_empty());
}

#[test]
fn an_invalid_marketplace_is_not_installed() {
    let e = env();
    let a = g("a");
    for manifest in [&br#"{"name":"other"}"#[..], b"not json"] {
        let mut f = mp_files();
        f.insert(MP_JSON.into(), manifest.to_vec());
        let r = apply_group(&e.paths, &a, only(&a), &f).unwrap();
        assert!(!mp_dir(&e).exists());
        assert!(
            r.skipped.iter().any(|n| n.contains("plugins/mods")),
            "{:?}",
            r.skipped
        );
        assert!(excluded_paths(&e.paths, &a).unwrap().contains(HOOKS));
        assert!(Manifest::load(&e.paths).unwrap().marketplaces.is_empty());
    }
    let mut f = mp_files();
    f.remove(MP_JSON);
    let r = apply_group(&e.paths, &a, only(&a), &f).unwrap();
    assert!(!mp_dir(&e).exists());
    assert!(!r.skipped.is_empty());
}

#[test]
fn an_oversize_plugin_file_is_skipped() {
    let e = env();
    let a = g("a");
    let mut f = mp_files();
    f.insert(
        "plugins/mods/big.bin".into(),
        vec![0; group::MAX_PLUGIN_FILE + 1],
    );
    let r = apply_group(&e.paths, &a, only(&a), &f).unwrap();
    assert!(!mp_dir(&e).join("big.bin").exists());
    assert!(mp_dir(&e).join("mods/router/hooks/hooks.json").exists());
    assert!(
        r.skipped
            .iter()
            .any(|n| n.contains("plugins/mods/big.bin") && n.contains("1 MiB")),
        "{:?}",
        r.skipped
    );
}

#[test]
fn a_stray_file_in_the_marketplace_folder_is_backed_up_and_replaced() {
    let e = env();
    let a = g("a");
    put(&mp_dir(&e).join("mods/router/hooks/hooks.json"), b"stale\n");
    let r = apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    assert!(r.collisions.is_empty(), "{:?}", r.collisions);
    assert_eq!(
        read(&mp_dir(&e).join("mods/router/hooks/hooks.json")).unwrap(),
        b"{\"hooks\":{}}\n"
    );
    assert!(backups(&e)
        .iter()
        .any(|p| read(p).as_deref() == Some(&b"stale\n"[..])));
}

#[test]
fn a_file_added_in_the_marketplace_folder_is_a_local_change() {
    let e = env();
    let a = g("a");
    apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    let new = "plugins/mods/mods/router/hooks/extra.ts";
    put(&target_of(&e.paths, new), b"export {}\n");
    // Never synced: dependencies, git internals, env files, temp files.
    put(&mp_dir(&e).join("node_modules/x/index.js"), b"x");
    put(&mp_dir(&e).join(".env"), b"SECRET=1");
    put(&mp_dir(&e).join("mods/.x.claude-sync-tmp-1"), b"t");
    assert_eq!(local_files(&e.paths, &a).unwrap()[new], b"export {}\n");
    assert_eq!(
        crate::watch::local_edits(&e.paths).unwrap(),
        vec![crate::watch::LocalEdit {
            group: a.clone(),
            path: new.into(),
            deleted: false
        }]
    );
    // An apply that does not ship it leaves it alone.
    apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    assert!(target_of(&e.paths, new).exists());

    // Keep local: no longer listed, and it stays.
    keep_local(&e.paths, &a, new).unwrap();
    assert!(crate::watch::local_edits(&e.paths).unwrap().is_empty());
    apply_group(&e.paths, &a, only(&a), &mp_files()).unwrap();
    assert!(target_of(&e.paths, new).exists());
    assert!(!local_files(&e.paths, &a).unwrap().contains_key(new));

    // Undo of another new file removes it (with a backup).
    let other = "plugins/mods/notes.md";
    put(&target_of(&e.paths, other), b"draft\n");
    undo(&e.paths, &a, other, None).unwrap();
    assert!(!target_of(&e.paths, other).exists());
    assert!(backups(&e)
        .iter()
        .any(|p| read(p).as_deref() == Some(&b"draft\n"[..])));
    // A path outside an owned marketplace is still refused.
    assert!(undo(&e.paths, &a, "plugins/nope/x.md", None).is_err());
}

#[test]
fn a_backup_keeps_the_first_copy_of_a_path() {
    let e = env();
    let mut b = Backups::new(&e.paths);
    b.save("settings.json", b"original").unwrap();
    b.save("settings.json", b"second").unwrap();
    let all = backups(&e);
    assert_eq!(all.len(), 1);
    assert_eq!(read(&all[0]).unwrap(), b"original");
}

/// Apply every group in `order`, the way a reorder does.
fn apply_all(e: &Env, order: &[GroupId], files: &[&FileSet]) {
    for (id, f) in order.iter().zip(files) {
        apply_group(&e.paths, id, order, f).unwrap();
    }
}

#[test]
fn reorder_moves_a_shared_file_to_the_new_winner() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let fa = set(&[(
        "agents/x.md",
        b"from a
",
    )]);
    let fb = set(&[(
        "agents/x.md",
        b"from b
",
    )]);
    apply_all(&e, &[a.clone(), b.clone()], &[&fa, &fb]);
    assert_eq!(
        read(&home(&e, "agents/x.md")).unwrap(),
        b"from a
"
    );
    assert_eq!(
        Manifest::load(&e.paths).unwrap().files["agents/x.md"].group,
        a
    );

    // b moves above a.
    apply_all(&e, &[b.clone(), a.clone()], &[&fb, &fa]);
    assert_eq!(
        read(&home(&e, "agents/x.md")).unwrap(),
        b"from b
"
    );
    assert_eq!(
        Manifest::load(&e.paths).unwrap().files["agents/x.md"].group,
        b
    );

    // And back again.
    apply_all(&e, &[a.clone(), b.clone()], &[&fa, &fb]);
    assert_eq!(
        read(&home(&e, "agents/x.md")).unwrap(),
        b"from a
"
    );
    assert_eq!(
        Manifest::load(&e.paths).unwrap().files["agents/x.md"].group,
        a
    );
}

#[test]
fn reorder_changes_the_winner_of_a_setting() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let fa = shared(json!({"model": "opus"}));
    let fb = shared(json!({"model": "haiku", "theme": "light"}));
    apply_all(&e, &[a.clone(), b.clone()], &[&fa, &fb]);
    assert_eq!(
        settings_json(&e),
        json!({"model": "opus", "theme": "light"})
    );
    apply_all(&e, &[b.clone(), a.clone()], &[&fb, &fa]);
    assert_eq!(
        settings_json(&e),
        json!({"model": "haiku", "theme": "light"})
    );
    apply_all(&e, &[a.clone(), b.clone()], &[&fa, &fb]);
    assert_eq!(
        settings_json(&e),
        json!({"model": "opus", "theme": "light"})
    );
}

#[test]
fn reorder_moves_a_marketplace_to_the_new_winner() {
    let e = env();
    let (a, b) = (g("a"), g("b"));
    let fa = mp_files();
    let mut fb = mp_files();
    fb.insert(
        "plugins/mods/only-in-b.txt".into(),
        b"b
"
        .to_vec(),
    );
    apply_all(&e, &[a.clone(), b.clone()], &[&fa, &fb]);
    assert_eq!(
        Manifest::load(&e.paths).unwrap().marketplaces["mods"].group,
        a
    );
    assert!(!mp_dir(&e).join("only-in-b.txt").exists());

    apply_all(&e, &[b.clone(), a.clone()], &[&fb, &fa]);
    assert_eq!(
        Manifest::load(&e.paths).unwrap().marketplaces["mods"].group,
        b
    );
    assert_eq!(
        read(&mp_dir(&e).join("only-in-b.txt")).unwrap(),
        b"b
"
    );
    assert_eq!(
        settings_json(&e)["extraKnownMarketplaces"]["mods"],
        mp_source(&e)
    );

    apply_all(&e, &[a.clone(), b.clone()], &[&fa, &fb]);
    assert_eq!(
        Manifest::load(&e.paths).unwrap().marketplaces["mods"].group,
        a
    );
    assert!(!mp_dir(&e).join("only-in-b.txt").exists());
}

// ---------------------------------------------------------------- turning a group off

fn marketplace_files() -> [(&'static str, Vec<u8>); 2] {
    [
        (
            "plugins/mp/.claude-plugin/marketplace.json",
            br#"{"name":"mp","plugins":[]}"#.to_vec(),
        ),
        ("plugins/mp/p/plugin.json", b"{}".to_vec()),
    ]
}

fn group_files() -> FileSet {
    let mut f = tree();
    f.insert("CLAUDE.md".into(), b"# group rules\n".to_vec());
    f.insert(
        "settings.shared.json".into(),
        serde_json::to_vec(&json!({
            "enabledPlugins": {"tool@mp": true},
            "model": "opus",
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo group"}]}]},
        }))
        .unwrap(),
    );
    for (p, b) in marketplace_files() {
        f.insert(p.into(), b);
    }
    f
}

fn seed_user(e: &Env) {
    put(&home(e, "CLAUDE.md"), b"my own line\n\nsecond line\n");
    // Written the way the app writes settings.json (pretty, trailing newline).
    let mut settings = serde_json::to_vec_pretty(&json!({
        "theme": "dark",
        "enabledPlugins": {"mine@other": true},
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo mine"}]}]},
    }))
    .unwrap();
    settings.push(b'\n');
    put(&home(e, "settings.json"), &settings);
}

fn user_state(e: &Env) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (read(&home(e, "CLAUDE.md")), read(&home(e, "settings.json")))
}

#[test]
fn off_on_off_restores_the_files_exactly() {
    let e = env();
    let a = g("a");
    seed_user(&e);
    let pre = user_state(&e);

    let r = apply_group(&e.paths, &a, only(&a), &group_files()).unwrap();
    assert!(r.collisions.is_empty(), "{r:?}");
    let applied = user_state(&e);
    assert_ne!(applied, pre);
    assert!(home(&e, "skills/alpha/SKILL.md").exists());

    let off = disable_group(&e.paths, &a).unwrap();
    assert!(off.removed.contains(&"skills/alpha/SKILL.md".to_string()));
    assert_eq!(user_state(&e), pre, "off must restore the user's files");
    assert!(!home(&e, "skills/alpha/SKILL.md").exists());
    assert!(!home(&e, "agents/reviewer.md").exists());
    let plugin = e.paths.state_dir.join("marketplaces/mp/p/plugin.json");
    assert!(plugin.exists(), "plugin files stay for running sessions");
    assert!(is_disabled(&e.paths, &a).unwrap());

    let on = apply_group(&e.paths, &a, only(&a), &group_files()).unwrap();
    assert!(on.collisions.is_empty(), "{on:?}");
    assert!(on.kept.is_empty(), "{on:?}");
    assert_eq!(
        user_state(&e),
        applied,
        "on must give the first applied state back"
    );
    assert!(!is_disabled(&e.paths, &a).unwrap());
    assert_eq!(read(&plugin).unwrap(), b"{}");
    assert!(Manifest::load(&e.paths)
        .unwrap()
        .files
        .contains_key("plugins/mp/p/plugin.json"));

    disable_group(&e.paths, &a).unwrap();
    assert_eq!(user_state(&e), pre);
}

#[test]
fn keep_local_survives_off_and_on() {
    let e = env();
    let a = g("a");
    seed_user(&e);
    apply_group(&e.paths, &a, only(&a), &group_files()).unwrap();
    keep_local(&e.paths, &a, "agents/reviewer.md").unwrap();
    keep_local(&e.paths, &a, "settings.shared.json#/model").unwrap();
    let kept = |e: &Env| {
        Manifest::load(&e.paths).unwrap().groups["owner/a"]
            .keep_local
            .clone()
    };
    let before = kept(&e);
    assert_eq!(before.len(), 2);

    disable_group(&e.paths, &a).unwrap();
    assert_eq!(kept(&e), before);
    apply_group(&e.paths, &a, only(&a), &group_files()).unwrap();
    assert_eq!(kept(&e), before);
    // A kept setting is the user's own value now: off and on leave it be.
    assert_eq!(settings_json(&e)["model"], "opus");
}
