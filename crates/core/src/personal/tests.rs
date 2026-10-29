//! Tests for personal memory. Every device lives in a temp dir; the real `~/.claude` is
//! never read or written.
use super::*;
use crate::apply::{BLOCK_BEGIN, BLOCK_END};
use crate::github::GitHub;
use crate::group::{self, GroupFile};
use crate::remote::fake::{FakeGitHub, FakeRepo};
use crate::Paths;
use std::sync::Arc;

const WIN_HOME: &str = r"C:\Users\Ana";
const MAC_HOME: &str = "/Users/ana";
const WIN_ENC: &str = "C--Users-Ana";
const MAC_ENC: &str = "-Users-ana";

// ---------------------------------------------------------------- keys

#[test]
fn encode_matches_claude_code_folder_names() {
    assert_eq!(
        keys::encode(r"C:\Users\Mehdi\Projects\klawly"),
        "C--Users-Mehdi-Projects-klawly"
    );
    assert_eq!(
        keys::encode("/Users/mehdi/Projects/klawly"),
        "-Users-mehdi-Projects-klawly"
    );
    assert_eq!(keys::encode("/Users/a/my_app.v2"), "-Users-a-my-app-v2");
    // One `-` per UTF-16 unit: `é` is one, an emoji is two.
    assert_eq!(keys::encode("/é/😀"), "-----");
    assert_eq!(keys::encode(WIN_HOME), WIN_ENC);
    assert_eq!(keys::encode(MAC_HOME), MAC_ENC);
}

#[test]
fn keys_are_relative_to_home_on_both_systems() {
    let win = keys::key_for("C--Users-Ana-Projects-klawly", WIN_ENC);
    let mac = keys::key_for("-Users-ana-Projects-klawly", MAC_ENC);
    assert_eq!(win.as_deref(), Some("home-Projects-klawly"));
    assert_eq!(win, mac);
    assert_eq!(keys::key_for(WIN_ENC, WIN_ENC).as_deref(), Some("home"));
    assert_eq!(keys::key_for(MAC_ENC, MAC_ENC).as_deref(), Some("home"));
    // Back to each system's own folder.
    assert_eq!(
        keys::folder_for("home-Projects-klawly", WIN_ENC).as_deref(),
        Some("C--Users-Ana-Projects-klawly")
    );
    assert_eq!(
        keys::folder_for("home-Projects-klawly", MAC_ENC).as_deref(),
        Some("-Users-ana-Projects-klawly")
    );
    assert_eq!(keys::folder_for("home", MAC_ENC).as_deref(), Some(MAC_ENC));
}

#[test]
fn keys_ignore_the_case_of_the_drive_letter() {
    assert_eq!(
        keys::key_for("c--Users-Ana-Projects-klawly", WIN_ENC).as_deref(),
        Some("home-Projects-klawly")
    );
    assert_eq!(
        keys::key_for("c--users-ana-Projects-klawly", WIN_ENC).as_deref(),
        Some("home-Projects-klawly")
    );
}

#[test]
fn keys_skip_folders_outside_home_and_temp_folders() {
    for folder in [
        "C--Users-Anabel-Projects-x",        // another home that starts the same
        "C--Users-Bo-Projects-x",            // someone else's home
        "D--work-x",                         // another drive
        "C--Users-Ana-AppData-Local-Temp-x", // temp
        "C--Users-Ana-AppData-Roaming-x",    // app data
        "C--Users-Ana--claude-jobs-8c99d407", // background jobs
        "C--Users-Ana-Projects-x--claude-worktrees-y", // worktrees
        "C--Users-Ana-Projects-x--worktrees-y",
        "-tmp-x",                      // macOS /tmp
        "-private-var-folders-xy-T-x", // macOS temp
        "-private-tmp-x",
        "C--Users-Ana-Projects-é", // not what Claude Code writes
    ] {
        assert_eq!(keys::key_for(folder, WIN_ENC), None, "{folder}");
        assert_eq!(keys::key_for(folder, MAC_ENC), None, "{folder}");
    }
    assert_eq!(keys::key_for("-Users-ana-.claude-jobs-x", MAC_ENC), None);
    assert_eq!(keys::key_for("-Users-ana--claude-jobs-x", MAC_ENC), None);
    assert_eq!(
        keys::key_for("-Users-ana-Library-x", MAC_ENC).as_deref(),
        Some("home-Library-x")
    );
}

#[test]
fn keys_from_a_repo_are_checked() {
    for bad in [
        "",
        "home/x",
        "home..",
        "home-../x",
        "homeX",
        "Home",
        "tmp",
        "home-a b",
        "home-AppData-x",
    ] {
        assert!(!keys::is_key(bad), "{bad}");
        assert_eq!(keys::folder_for(bad, WIN_ENC), None, "{bad}");
    }
    assert!(keys::is_key("home"));
    assert!(keys::is_key("home-Projects-klawly"));
    assert!(!keys::is_key(&format!("home-{}", "a".repeat(300))));
}

/// Two folders that differ only in case need a case-sensitive file system.
#[cfg(target_os = "linux")]
#[test]
fn choose_reuses_the_case_variant_that_has_memory() {
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path();
    std::fs::create_dir_all(projects.join("c--Users-Ana-Projects-x/memory")).unwrap();
    std::fs::write(
        projects.join("c--Users-Ana-Projects-x/memory/MEMORY.md"),
        "x",
    )
    .unwrap();
    std::fs::create_dir_all(projects.join("C--Users-Ana-Projects-x")).unwrap();
    let names = vec![
        "C--Users-Ana-Projects-x".to_string(),
        "c--Users-Ana-Projects-x".to_string(),
    ];
    assert_eq!(
        keys::choose(projects, names.clone(), WIN_ENC),
        "c--Users-Ana-Projects-x"
    );
    // With no memory anywhere, the one spelled like the home folder wins.
    std::fs::remove_dir_all(projects.join("c--Users-Ana-Projects-x/memory")).unwrap();
    assert_eq!(
        keys::choose(projects, names, WIN_ENC),
        "C--Users-Ana-Projects-x"
    );
}

#[test]
fn personal_paths() {
    for ok in [
        FILE,
        DEVICES,
        README,
        CLAUDE_MD,
        "settings.json",
        "memory/home/MEMORY.md",
        "memory/home-Projects-x/MEMORY.md",
        "memory/home-Projects-x/notes/a.md",
    ] {
        assert!(is_personal_path(ok), "{ok}");
    }
    for bad in [
        "memory/home",
        "memory/home/",
        "memory/home/.hidden",
        "memory/home/../x.md",
        "memory/tmp/x.md",
        "memory/home-AppData-Local-Temp/x.md",
        "agents/a.md",
        "skills/x/SKILL.md",
        "group.json",
    ] {
        assert!(!is_personal_path(bad), "{bad}");
    }
}

#[test]
fn group_allowlist_rejects_personal_files() {
    for p in [
        FILE,
        DEVICES,
        "memory/home/MEMORY.md",
        "memory/home-Projects-x/a.md",
    ] {
        assert!(!group::is_allowlisted(p), "{p}");
    }
}

// ---------------------------------------------------------------- CLAUDE.md

fn with_block(lines: &[&str], rest: &str, eol: &str) -> String {
    let mut s = format!("{BLOCK_BEGIN}{eol}");
    for l in lines {
        s.push_str(l);
        s.push_str(eol);
    }
    s.push_str(BLOCK_END);
    s.push_str(eol);
    if !rest.is_empty() {
        s.push_str(eol);
        s.push_str(&rest.replace('\n', eol));
    }
    s
}

const IMPORT: &str = "@~/.claude-sync/groups/ana__claude-sync-data/CLAUDE.md";

#[test]
fn strip_removes_the_managed_block_and_group_imports() {
    let text = with_block(&[IMPORT], "# Me\n\nI like apples.\n", "\n");
    assert_eq!(
        claude_md::strip(text.as_bytes()).unwrap(),
        b"# Me\n\nI like apples.\n"
    );
    // A stray import outside the block (older app versions) stays on the device too.
    let stray = format!("{IMPORT}\n# Me\n");
    assert_eq!(claude_md::strip(stray.as_bytes()).unwrap(), b"# Me\n");
    let windows = "@C:\\Users\\Ana\\.claude-sync\\groups\\a__b\\CLAUDE.md\r\n# Me\r\n";
    assert_eq!(claude_md::strip(windows.as_bytes()).unwrap(), b"# Me\n");
    // Line ends are normalized, so a Windows and a Mac file compare equal.
    let crlf = with_block(&[IMPORT], "# Me\n", "\r\n");
    assert_eq!(claude_md::strip(crlf.as_bytes()).unwrap(), b"# Me\n");
    // Nothing but the block: nothing to sync.
    assert_eq!(
        claude_md::strip(with_block(&[IMPORT], "", "\n").as_bytes()),
        None
    );
    assert_eq!(claude_md::strip(b"\n\n"), None);
    // Mentioning the folder in prose is not an import line.
    let prose = b"Never edit ~/.claude-sync/groups/x/CLAUDE.md by hand.\n";
    assert_eq!(claude_md::strip(prose).unwrap(), prose);
}

#[test]
fn compose_keeps_this_devices_block() {
    let mac_import = "@~/.claude-sync/groups/bo__team/CLAUDE.md";
    let existing = with_block(&[mac_import], "# Old\n", "\n");
    let out = claude_md::compose(Some(existing.as_bytes()), Some(b"# Me\n")).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        with_block(&[mac_import], "# Me\n", "\n")
    );
    // CRLF files stay CRLF.
    let existing = with_block(&[mac_import], "# Old\n", "\r\n");
    let out = claude_md::compose(Some(existing.as_bytes()), Some(b"# Me\nmore\n")).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        with_block(&[mac_import], "# Me\nmore\n", "\r\n")
    );
    // No block on this device: just the text.
    assert_eq!(
        claude_md::compose(None, Some(b"# Me\n")).unwrap(),
        b"# Me\n"
    );
    assert_eq!(
        claude_md::compose(Some(b"# Old\n"), Some(b"# Me\n")).unwrap(),
        b"# Me\n"
    );
    // Synced text removed: the block stays, or the file goes.
    assert_eq!(
        String::from_utf8(claude_md::compose(Some(existing.as_bytes()), None).unwrap()).unwrap(),
        with_block(&[mac_import], "", "\r\n")
    );
    assert_eq!(claude_md::compose(Some(b"# Old\n"), None), None);
}

#[test]
fn strip_then_compose_gives_the_file_back() {
    for text in [
        with_block(&[IMPORT], "# Me\n\nI like apples.\n", "\n"),
        with_block(
            &[IMPORT, "@~/.claude-sync/groups/x__y/CLAUDE.md"],
            "# Me\n",
            "\r\n",
        ),
        "# Me only\n".to_string(),
    ] {
        let synced = claude_md::strip(text.as_bytes());
        let back = claude_md::compose(Some(text.as_bytes()), synced.as_deref()).unwrap();
        assert_eq!(String::from_utf8(back).unwrap(), text);
    }
}

// ---------------------------------------------------------------- MEMORY.md

fn lm(base: Option<&str>, local: Option<&str>, remote: Option<&str>) -> Option<String> {
    lines::merge(
        base.map(str::as_bytes),
        local.map(str::as_bytes),
        remote.map(str::as_bytes),
    )
    .map(|b| String::from_utf8(b).unwrap())
}

#[test]
fn memory_index_with_no_base_is_the_union() {
    let local = "- [A](a.md) — a\n- [B](b.md) — b\n";
    let remote = "- [B](b.md) — b\n- [C](c.md) — c\n";
    assert_eq!(
        lm(None, Some(local), Some(remote)).unwrap(),
        "- [A](a.md) — a\n- [B](b.md) — b\n- [C](c.md) — c\n"
    );
}

#[test]
fn memory_index_three_way() {
    let base = "- [A](a.md) — a\n- [B](b.md) — b\n";
    // Both added a line at the end: both kept, local first.
    assert_eq!(
        lm(
            Some(base),
            Some("- [A](a.md) — a\n- [B](b.md) — b\n- [L](l.md) — l\n"),
            Some("- [A](a.md) — a\n- [B](b.md) — b\n- [R](r.md) — r\n"),
        )
        .unwrap(),
        "- [A](a.md) — a\n- [B](b.md) — b\n- [L](l.md) — l\n- [R](r.md) — r\n"
    );
    // Remote removed A and added R after it: A goes, R lands where it was.
    assert_eq!(
        lm(
            Some(base),
            Some(base),
            Some("- [R](r.md) — r\n- [B](b.md) — b\n")
        )
        .unwrap(),
        "- [R](r.md) — r\n- [B](b.md) — b\n"
    );
    // Local removed B, remote edited A's hook: the old A line goes (edited), new one stays.
    assert_eq!(
        lm(
            Some(base),
            Some("- [A](a.md) — a\n"),
            Some("- [A](a.md) — a2\n- [B](b.md) — b\n")
        )
        .unwrap(),
        "- [A](a.md) — a2\n"
    );
    // Exact duplicates go; blank lines and line ends stay.
    assert_eq!(
        lm(
            None,
            Some("# Index\r\n\r\n- x\r\n- x\r\n"),
            Some("- x\n- y\n")
        )
        .unwrap(),
        "# Index\r\n\r\n- x\r\n- y\r\n"
    );
    // Deleted on one side and untouched on the other: deleted.
    assert_eq!(lm(Some(base), None, Some(base)), None);
    // Deleted on one side and changed on the other: the change wins.
    assert_eq!(
        lm(Some(base), Some("- new\n"), None).as_deref(),
        Some("- new\n")
    );
}

// ---------------------------------------------------------------- devices

struct Dev {
    engine: Engine,
    _dir: tempfile::TempDir,
}

impl std::ops::Deref for Dev {
    type Target = Engine;
    fn deref(&self) -> &Engine {
        &self.engine
    }
}

impl Dev {
    fn home(&self) -> &Path {
        &self.engine.paths.claude_home
    }
    fn claude_md(&self) -> Option<String> {
        std::fs::read_to_string(self.home().join(CLAUDE_MD)).ok()
    }
    fn set_claude_md(&self, text: &str) {
        write_atomic(&self.home().join(CLAUDE_MD), text.as_bytes()).unwrap();
    }
    fn mem_path(&self, folder: &str, rel: &str) -> PathBuf {
        self.home()
            .join("projects")
            .join(folder)
            .join("memory")
            .join(rel)
    }
    fn mem(&self, folder: &str, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.mem_path(folder, rel)).ok()
    }
    fn set_mem(&self, folder: &str, rel: &str, text: &str) {
        write_atomic(&self.mem_path(folder, rel), text.as_bytes()).unwrap();
    }
    async fn sync(&self) -> PersonalSyncReport {
        let r = self.personal_sync(false).await.unwrap().expect("enabled");
        assert_eq!(r.error, None);
        r
    }
}

/// `login`'s device `name` with home folder `home` (only used for keys).
fn dev(world: &FakeGitHub, login: &str, name: &str, home: &str) -> Dev {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        claude_home: dir.path().join("claude"),
        state_dir: dir.path().join("state"),
    };
    let gh = GitHub::with_api(String::new(), "http://127.0.0.1:9");
    let engine = Engine::with_remotes(
        paths,
        gh,
        login.into(),
        name.into(),
        Arc::new(world.account(login)),
    )
    .with_user_home(PathBuf::from(home));
    Dev { engine, _dir: dir }
}

fn personal_repo(world: &FakeGitHub, login: &str) -> FakeRepo {
    world
        .repo(&GroupId::new(login, REPO))
        .expect("personal repo exists")
}

const KLAWLY_WIN: &str = "C--Users-Ana-Projects-klawly";
const KLAWLY_MAC: &str = "-Users-ana-Projects-klawly";
const KLAWLY_KEY: &str = "memory/home-Projects-klawly";

/// Every file under `backups/personal/<stamp>/`, keyed `<stamp-less path>`, with its text.
fn personal_backups(d: &Dev) -> Vec<(String, String)> {
    let root = d.paths.backups_dir().join("personal");
    let mut out: Vec<(String, String)> = crate::engine::read_tree(&root)
        .unwrap_or_default()
        .into_iter()
        .map(|(p, b)| {
            let rest = p.split_once('/').map_or(p.clone(), |(_, r)| r.to_string());
            (rest, String::from_utf8(b).unwrap())
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn memory_deleted_or_overwritten_by_a_sync_is_backed_up() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.set_mem(KLAWLY_WIN, "a.md", "apples\n");
    pc.set_mem(KLAWLY_WIN, "b.md", "bananas\n");
    pc.personal_enable().await.unwrap();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.personal_enable().await.unwrap();
    assert_eq!(mac.mem(KLAWLY_MAC, "a.md").as_deref(), Some("apples\n"));
    assert!(personal_backups(&pc).is_empty());

    // The mac deletes one memory and changes the other.
    std::fs::remove_file(mac.mem_path(KLAWLY_MAC, "a.md")).unwrap();
    mac.set_mem(KLAWLY_MAC, "b.md", "blueberries\n");
    mac.sync().await;
    pc.sync().await;

    assert_eq!(pc.mem(KLAWLY_WIN, "a.md"), None);
    assert_eq!(pc.mem(KLAWLY_WIN, "b.md").as_deref(), Some("blueberries\n"));
    assert_eq!(
        personal_backups(&pc),
        [
            (
                "memory/home-Projects-klawly/a.md".to_string(),
                "apples\n".to_string()
            ),
            (
                "memory/home-Projects-klawly/b.md".to_string(),
                "bananas\n".to_string()
            ),
        ]
    );
}

#[tokio::test]
async fn enable_creates_a_private_repo_without_the_group_topic() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    assert!(!pc.personal_status().enabled);
    pc.set_claude_md(&with_block(&[IMPORT], "I like apples.\n", "\n"));
    pc.set_mem(KLAWLY_WIN, "MEMORY.md", "- [A](a.md) — a\n");
    pc.set_mem(KLAWLY_WIN, "a.md", "apples\n");

    let status = pc.personal_enable().await.unwrap();
    assert!(status.enabled && status.decided);
    assert_eq!(status.last_error, None);
    assert_eq!(status.repo, "ana/claude-sync-personal");
    assert_eq!(
        status.repo_url,
        "https://github.com/ana/claude-sync-personal"
    );
    assert_eq!(status.new_devices_join, Some(true));
    assert_eq!(status.devices.len(), 1);
    assert!(status.devices[0].this_device);

    let repo = personal_repo(&world, "ana");
    assert!(repo.topics().is_empty());
    assert!(repo.is_private().await.unwrap());
    let pf = PersonalFile::parse(repo.file(FILE).unwrap().as_bytes()).unwrap();
    assert_eq!((pf.format, pf.new_devices_join), (1, true));
    assert_eq!(repo.file(CLAUDE_MD).as_deref(), Some("I like apples.\n"));
    assert_eq!(
        repo.file(&format!("{KLAWLY_KEY}/MEMORY.md")).as_deref(),
        Some("- [A](a.md) — a\n")
    );
    assert_eq!(
        repo.file(&format!("{KLAWLY_KEY}/a.md")).as_deref(),
        Some("apples\n")
    );
    assert!(repo.file(README).is_some());
    let devices: DevicesFile = serde_json::from_str(&repo.file(DEVICES).unwrap()).unwrap();
    assert_eq!(devices.devices.len(), 1);
    assert_eq!(devices.devices[0].name, "pc");
    // The local file is untouched.
    assert_eq!(
        pc.claude_md().unwrap(),
        with_block(&[IMPORT], "I like apples.\n", "\n")
    );

    // It is not a group: not discovered, can't be joined.
    assert!(pc.discover().await.unwrap().is_empty());
    assert!(pc.join(&GroupId::new("ana", REPO)).await.is_err());
    assert!(pc.groups().await.unwrap().is_empty());

    // Made public later: nothing more is pushed.
    repo.set_private(false);
    pc.set_mem(KLAWLY_WIN, "b.md", "bananas\n");
    let r = pc.personal_sync(false).await.unwrap().unwrap();
    assert!(r.error.is_some_and(|e| e.contains("public")));
    assert!(!r.pushed);
    assert!(repo.file(&format!("{KLAWLY_KEY}/b.md")).is_none());
    assert!(pc.personal_status().last_error.is_some());
}

#[tokio::test]
async fn enable_refuses_a_repo_that_is_not_a_personal_repo() {
    let world = FakeGitHub::new();
    let other = FakeRepo::with_files(GroupId::new("ana", REPO), &[("notes.md", "mine\n")]);
    world.add(&other);
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    assert!(pc.personal_enable().await.is_err());
    assert!(!pc.personal_status().enabled);
    assert_eq!(other.files().len(), 1);

    let world = FakeGitHub::new();
    let public = FakeRepo::new(GroupId::new("ana", REPO));
    public.set_private(false);
    world.add(&public);
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    assert!(matches!(
        pc.personal_enable().await,
        Err(Error::PublicRepo(_))
    ));
    assert!(public.files().is_empty());
}

#[tokio::test]
async fn enable_names_the_personal_rules_mod_when_it_owns_the_repo() {
    let world = FakeGitHub::new();
    let mod_repo = FakeRepo::with_files(
        GroupId::new("ana", REPO),
        &[
            ("README.md", "# Rules\n\nKept by the personal-rules mod.\n"),
            ("owners/ana.md", "rule\n"),
            ("projects/x/rules.md", "rule\n"),
        ],
    );
    world.add(&mod_repo);
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    let Err(Error::Invalid(m)) = pc.personal_enable().await else {
        panic!("expected Invalid")
    };
    assert!(m.contains("used by the personal-rules mod"), "{m}");

    let world = FakeGitHub::new();
    let other = FakeRepo::with_files(GroupId::new("ana", REPO), &[("notes.md", "mine\n")]);
    world.add(&other);
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    let Err(Error::Invalid(m)) = pc.personal_enable().await else {
        panic!("expected Invalid")
    };
    assert!(m.contains("wasn't made by claude-sync"), "{m}");
}

#[tokio::test]
async fn two_devices_share_memory_across_windows_and_macos() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.set_claude_md(&with_block(&[IMPORT], "I like apples.\n", "\r\n"));
    pc.set_mem(KLAWLY_WIN, "MEMORY.md", "- [A](a.md) — a\n");
    pc.set_mem(KLAWLY_WIN, "a.md", "apples\n");
    pc.set_mem(WIN_ENC, "MEMORY.md", "- [H](h.md) — home\n");
    pc.set_mem(WIN_ENC, "h.md", "home\n");
    pc.personal_enable().await.unwrap();

    // The Mac has its own group block; the app joins it after sign-in.
    let mac_import = "@~/.claude-sync/groups/bo__team/CLAUDE.md";
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.set_claude_md(&with_block(&[mac_import], "", "\n"));
    assert_eq!(
        mac.personal_check_new_device().await.unwrap(),
        NewDevice::Joined
    );
    let st = mac.personal_status();
    assert!(st.enabled && st.decided);
    assert_eq!(st.devices.len(), 2);
    assert_eq!(st.devices.iter().filter(|d| d.this_device).count(), 1);

    assert_eq!(
        mac.claude_md().unwrap(),
        with_block(&[mac_import], "I like apples.\n", "\n")
    );
    assert_eq!(mac.mem(KLAWLY_MAC, "a.md").as_deref(), Some("apples\n"));
    assert_eq!(mac.mem(MAC_ENC, "h.md").as_deref(), Some("home\n"));

    // Both add a memory at the same time.
    mac.set_mem(
        KLAWLY_MAC,
        "MEMORY.md",
        "- [A](a.md) — a\n- [M](m.md) — mac\n",
    );
    mac.set_mem(KLAWLY_MAC, "m.md", "from the mac\n");
    pc.set_mem(
        KLAWLY_WIN,
        "MEMORY.md",
        "- [A](a.md) — a\n- [P](p.md) — pc\n",
    );
    pc.set_mem(KLAWLY_WIN, "p.md", "from the pc\n");
    let r = mac.sync().await;
    assert!(r.pushed);
    let r = pc.sync().await;
    assert!(r.pulled && r.pushed && r.conflicts.is_empty());
    mac.sync().await;
    let both = "- [A](a.md) — a\n- [P](p.md) — pc\n- [M](m.md) — mac\n";
    assert_eq!(pc.mem(KLAWLY_WIN, "MEMORY.md").as_deref(), Some(both));
    assert_eq!(mac.mem(KLAWLY_MAC, "MEMORY.md").as_deref(), Some(both));
    assert_eq!(
        pc.mem(KLAWLY_WIN, "m.md").as_deref(),
        Some("from the mac\n")
    );
    assert_eq!(
        mac.mem(KLAWLY_MAC, "p.md").as_deref(),
        Some("from the pc\n")
    );

    // CLAUDE.md edits flow both ways and keep each device's block and line ends.
    mac.set_claude_md(&with_block(
        &[mac_import],
        "I like apples and pears.\n",
        "\n",
    ));
    mac.sync().await;
    pc.sync().await;
    assert_eq!(
        pc.claude_md().unwrap(),
        with_block(&[IMPORT], "I like apples and pears.\n", "\r\n")
    );

    // A deletion is synced, and the empty folder goes too.
    pc.set_mem(KLAWLY_WIN, "notes/x.md", "x\n");
    pc.sync().await;
    mac.sync().await;
    assert_eq!(mac.mem(KLAWLY_MAC, "notes/x.md").as_deref(), Some("x\n"));
    std::fs::remove_file(pc.mem_path(KLAWLY_WIN, "notes/x.md")).unwrap();
    pc.sync().await;
    mac.sync().await;
    assert!(!mac.mem_path(KLAWLY_MAC, "notes").exists());
    assert!(personal_repo(&world, "ana")
        .file(&format!("{KLAWLY_KEY}/notes/x.md"))
        .is_none());

    // Quiet when nothing changed (the first check has no ETag yet).
    pc.personal_sync(true).await.unwrap();
    mac.personal_sync(true).await.unwrap();
    assert!(pc.personal_sync(true).await.unwrap().is_none());
    assert!(mac.personal_sync(true).await.unwrap().is_none());
    // A local edit wakes it even when the repo did not change.
    pc.set_mem(KLAWLY_WIN, "a.md", "apples!\n");
    let r = pc.personal_sync(true).await.unwrap().expect("ran");
    assert!(r.pushed);
}

#[tokio::test]
async fn a_new_device_merges_and_never_overwrites_local_memory() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.set_mem(
        KLAWLY_WIN,
        "MEMORY.md",
        "- [A](a.md) — a\n- [F](feedback.md) — f\n",
    );
    pc.set_mem(KLAWLY_WIN, "feedback.md", "from the pc\n");
    pc.set_claude_md("pc text\n");
    pc.personal_enable().await.unwrap();

    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.set_mem(
        KLAWLY_MAC,
        "MEMORY.md",
        "- [F](feedback.md) — f\n- [O](own.md) — own\n",
    );
    mac.set_mem(KLAWLY_MAC, "feedback.md", "from the mac\n");
    mac.set_mem(KLAWLY_MAC, "own.md", "only on the mac\n");
    mac.set_claude_md("mac text\n");
    assert_eq!(
        mac.personal_check_new_device().await.unwrap(),
        NewDevice::Joined
    );

    // The index is the union.
    assert_eq!(
        mac.mem(KLAWLY_MAC, "MEMORY.md").as_deref(),
        Some("- [A](a.md) — a\n- [F](feedback.md) — f\n- [O](own.md) — own\n")
    );
    // A file both have with different content is a conflict: the repo's copy wins here
    // and the local copy is saved.
    assert_eq!(
        mac.mem(KLAWLY_MAC, "feedback.md").as_deref(),
        Some("from the pc\n")
    );
    assert_eq!(mac.claude_md().as_deref(), Some("pc text\n"));
    let st = mac.personal_status();
    let mut paths: Vec<&str> = st.conflicts.iter().map(|c| c.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, [CLAUDE_MD, &format!("{KLAWLY_KEY}/feedback.md")]);
    for c in &st.conflicts {
        let saved = std::fs::read_to_string(&c.saved_to).unwrap();
        assert!(
            saved == "from the mac\n" || saved == "mac text\n",
            "{saved}"
        );
        assert!(Path::new(&c.saved_to).starts_with(&mac.paths.state_dir));
    }
    // What only the Mac had reaches the repo and the PC.
    let repo = personal_repo(&world, "ana");
    assert_eq!(
        repo.file(&format!("{KLAWLY_KEY}/own.md")).as_deref(),
        Some("only on the mac\n")
    );
    pc.sync().await;
    assert_eq!(
        pc.mem(KLAWLY_WIN, "own.md").as_deref(),
        Some("only on the mac\n")
    );
    assert_eq!(
        pc.mem(KLAWLY_WIN, "MEMORY.md").as_deref(),
        Some("- [A](a.md) — a\n- [F](feedback.md) — f\n- [O](own.md) — own\n")
    );
}

#[tokio::test]
async fn an_existing_lowercase_drive_folder_is_reused() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    let lower = "c--Users-Ana-Projects-klawly";
    pc.set_mem(lower, "a.md", "one\n");
    // A project with sessions but no memory yet.
    std::fs::create_dir_all(pc.home().join("projects").join("c--Users-Ana-Projects-y")).unwrap();
    pc.personal_enable().await.unwrap();
    let repo = personal_repo(&world, "ana");
    assert!(repo.file(&format!("{KLAWLY_KEY}/a.md")).is_some());
    repo.set_file(&format!("{KLAWLY_KEY}/b.md"), "two\n");
    repo.set_file("memory/home-Projects-y/c.md", "three\n");
    repo.set_file("memory/home-Projects-z/d.md", "four\n");
    pc.sync().await;
    let mut names: Vec<String> = std::fs::read_dir(pc.home().join("projects"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "C--Users-Ana-Projects-z",
            "c--Users-Ana-Projects-klawly",
            "c--Users-Ana-Projects-y"
        ]
    );
    assert_eq!(pc.mem(lower, "b.md").as_deref(), Some("two\n"));
    assert_eq!(
        pc.mem("c--Users-Ana-Projects-y", "c.md").as_deref(),
        Some("three\n")
    );
}

#[tokio::test]
async fn new_devices_can_be_offered_instead_of_joining() {
    let world = FakeGitHub::new();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    // No repo: nothing to offer, and nothing is created.
    assert_eq!(
        mac.personal_check_new_device().await.unwrap(),
        NewDevice::Nothing
    );
    assert!(world.repo(&GroupId::new("ana", REPO)).is_none());

    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.personal_enable().await.unwrap();
    let st = pc.personal_set_new_devices_join(false).await.unwrap();
    assert_eq!(st.new_devices_join, Some(false));
    let pf =
        PersonalFile::parse(personal_repo(&world, "ana").file(FILE).unwrap().as_bytes()).unwrap();
    assert!(!pf.new_devices_join);

    mac.set_mem(KLAWLY_MAC, "a.md", "mac\n");
    assert_eq!(
        mac.personal_check_new_device().await.unwrap(),
        NewDevice::Offer
    );
    let st = mac.personal_status();
    assert!(!st.enabled);
    assert!(st.offer, "the offer stays until the user answers");
    assert!(personal_repo(&world, "ana")
        .file(&format!("{KLAWLY_KEY}/a.md"))
        .is_none());
    mac.personal_dismiss_offer().await.unwrap();
    assert!(!mac.personal_status().offer);
    assert_eq!(
        mac.personal_check_new_device().await.unwrap(),
        NewDevice::Nothing
    );
    // The user can still turn it on.
    assert!(mac.personal_enable().await.unwrap().enabled);
    assert!(personal_repo(&world, "ana")
        .file(&format!("{KLAWLY_KEY}/a.md"))
        .is_some());

    // Another account never sees ana's repo.
    let bo = dev(&world, "bo", "pc", WIN_HOME);
    assert_eq!(
        bo.personal_check_new_device().await.unwrap(),
        NewDevice::Nothing
    );

    pc.personal_set_new_devices_join(true).await.unwrap();
    let third = dev(&world, "ana", "laptop", MAC_HOME);
    assert_eq!(
        third.personal_check_new_device().await.unwrap(),
        NewDevice::Joined
    );
}

#[tokio::test]
async fn a_device_that_turned_it_off_is_not_turned_back_on() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.set_mem(KLAWLY_WIN, "a.md", "one\n");
    pc.personal_enable().await.unwrap();
    let repo = personal_repo(&world, "ana");
    let head = repo.current_head();

    let st = pc.personal_disable().await.unwrap();
    assert!(!st.enabled && st.decided);
    pc.set_mem(KLAWLY_WIN, "a.md", "two\n");
    assert!(pc.personal_sync(false).await.unwrap().is_none());
    assert!(pc.personal_sync(true).await.unwrap().is_none());
    assert_eq!(
        pc.personal_check_new_device().await.unwrap(),
        NewDevice::Nothing
    );
    // Local files stay as they are; the repo only learns that it is off (for every device).
    assert_ne!(repo.current_head(), head);
    assert!(repo.file(&format!("{KLAWLY_KEY}/a.md")).as_deref() == Some("one\n"));
    assert_eq!(pc.mem(KLAWLY_WIN, "a.md").as_deref(), Some("two\n"));

    // On again: what changed meanwhile is merged, not lost.
    repo.set_file(&format!("{KLAWLY_KEY}/b.md"), "remote\n");
    pc.personal_enable().await.unwrap();
    assert_eq!(pc.mem(KLAWLY_WIN, "b.md").as_deref(), Some("remote\n"));
    assert_eq!(pc.personal_status().conflicts.len(), 1);
    assert_eq!(pc.mem(KLAWLY_WIN, "a.md").as_deref(), Some("one\n"));
}

#[tokio::test]
async fn turning_it_on_clears_an_old_error() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.set_mem(KLAWLY_WIN, "a.md", "one\n");
    pc.personal_enable().await.unwrap();
    let repo = personal_repo(&world, "ana");

    // A sync fails while the repo is public, and the error is kept.
    repo.set_private(false);
    pc.personal_sync(false).await.unwrap();
    assert!(pc.personal_status().last_error.is_some());

    // Fixed and turned on again: the old error is gone.
    repo.set_private(true);
    pc.personal_disable().await.unwrap();
    let st = pc.personal_enable().await.unwrap();
    assert_eq!(st.last_error, None);
    assert_eq!(pc.personal_status().last_error, None);
}

#[tokio::test]
async fn kinds_choose_what_syncs() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.set_claude_md("text\n");
    pc.set_mem(KLAWLY_WIN, "a.md", "one\n");
    pc.personal_enable().await.unwrap();
    let repo = personal_repo(&world, "ana");

    pc.personal_set_kinds(Kinds {
        claude_md: true,
        memory: false,
        state: true,
        settings: true,
    })
    .await
    .unwrap();
    pc.set_mem(KLAWLY_WIN, "a.md", "two\n");
    pc.set_claude_md("text 2\n");
    pc.sync().await;
    assert_eq!(
        repo.file(&format!("{KLAWLY_KEY}/a.md")).as_deref(),
        Some("one\n")
    );
    assert_eq!(repo.file(CLAUDE_MD).as_deref(), Some("text 2\n"));
    // Remote memory changes are not written either.
    repo.set_file(&format!("{KLAWLY_KEY}/b.md"), "remote\n");
    pc.sync().await;
    assert_eq!(pc.mem(KLAWLY_WIN, "b.md"), None);

    pc.personal_set_kinds(Kinds {
        claude_md: false,
        memory: true,
        state: true,
        settings: true,
    })
    .await
    .unwrap();
    pc.set_claude_md("text 3\n");
    pc.sync().await;
    assert_eq!(repo.file(CLAUDE_MD).as_deref(), Some("text 2\n"));
    // Memory back on: the edit made while it was off is pushed, the remote file pulled.
    assert_eq!(
        repo.file(&format!("{KLAWLY_KEY}/a.md")).as_deref(),
        Some("two\n")
    );
    assert_eq!(pc.mem(KLAWLY_WIN, "b.md").as_deref(), Some("remote\n"));
    assert!(pc.personal_status().conflicts.is_empty());
}

#[tokio::test]
async fn devices_can_be_renamed_and_removed() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.personal_enable().await.unwrap();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.personal_enable().await.unwrap();
    pc.sync().await;
    let st = pc.personal_status();
    assert_eq!(st.devices.len(), 2);
    let mac_id = mac.personal_status().device_id;
    assert_ne!(mac_id, st.device_id);

    // Nothing changed: the background pass stays quiet.
    mac.personal_sync(true).await.unwrap();
    assert!(mac.personal_sync(true).await.unwrap().is_none());

    // A rename (the app's device name) reaches devices.json on the next background pass.
    let renamed = Engine {
        device: "studio".into(),
        ..mac.engine
    };
    let renamed = Dev {
        engine: renamed,
        _dir: mac._dir,
    };
    let r = renamed.personal_sync(true).await.unwrap();
    assert!(r.is_some_and(|r| r.pushed));
    pc.sync().await;
    let names: Vec<String> = pc
        .personal_status()
        .devices
        .iter()
        .map(|d| d.name.clone())
        .collect();
    assert_eq!(names, ["pc", "studio"]);

    assert!(pc.personal_remove_device(&st.device_id).await.is_err());
    let st = pc.personal_remove_device(&mac_id).await.unwrap();
    assert_eq!(st.devices.len(), 1);
    let devices: DevicesFile =
        serde_json::from_str(&personal_repo(&world, "ana").file(DEVICES).unwrap()).unwrap();
    assert_eq!(devices.devices.len(), 1);
}

#[tokio::test]
async fn temp_folders_links_large_files_and_secrets_stay_local() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    let pc = Dev {
        engine: pc.engine.with_scanner(|text: &str| {
            text.lines()
                .enumerate()
                .filter(|(_, l)| l.contains("ghp_"))
                .map(|(n, _)| crate::secrets::Hit {
                    line: n + 1,
                    pattern: "github-token",
                })
                .collect()
        }),
        _dir: pc._dir,
    };
    pc.set_mem("C--Users-Ana-AppData-Local-Temp-x", "a.md", "temp\n");
    pc.set_mem("C--Users-Ana--claude-jobs-1234", "a.md", "job\n");
    pc.set_mem("D--elsewhere", "a.md", "outside home\n");
    pc.set_mem(KLAWLY_WIN, "big.md", &"x".repeat(MAX_FILE + 1));
    pc.set_mem(KLAWLY_WIN, "token.md", "ghp_abc\n");
    pc.set_mem(KLAWLY_WIN, ".hidden", "dot\n");
    pc.set_mem(KLAWLY_WIN, "ok.md", "fine\n");
    write_atomic(
        &pc.home()
            .join("projects")
            .join(KLAWLY_WIN)
            .join("not-memory.md"),
        b"outside memory/\n",
    )
    .unwrap();
    pc.personal_enable().await.unwrap();

    let repo = personal_repo(&world, "ana");
    let memory: Vec<String> = repo
        .files()
        .into_keys()
        .filter(|p| p.starts_with("memory/"))
        .collect();
    assert_eq!(memory, [format!("{KLAWLY_KEY}/ok.md")]);
    let skipped = pc.personal_status().skipped;
    assert!(skipped.iter().any(|s| s.contains("big.md")), "{skipped:?}");
    assert!(skipped
        .iter()
        .any(|s| s.contains("token.md") && s.contains("secret")));
    // Held files are left alone even when the repo changes them.
    repo.set_file(&format!("{KLAWLY_KEY}/token.md"), "clean\n");
    pc.sync().await;
    assert_eq!(pc.mem(KLAWLY_WIN, "token.md").as_deref(), Some("ghp_abc\n"));
    assert_eq!(
        repo.file(&format!("{KLAWLY_KEY}/token.md")).as_deref(),
        Some("clean\n")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn linked_memory_folders_are_not_followed() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", MAC_HOME);
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.md"), "outside\n").unwrap();
    let folder = pc.home().join("projects").join(KLAWLY_MAC);
    std::fs::create_dir_all(&folder).unwrap();
    std::os::unix::fs::symlink(outside.path(), folder.join("memory")).unwrap();
    pc.personal_enable().await.unwrap();
    let repo = personal_repo(&world, "ana");
    assert!(repo.files().keys().all(|p| !p.starts_with("memory/")));
    repo.set_file(&format!("{KLAWLY_KEY}/new.md"), "remote\n");
    pc.sync().await;
    assert!(!outside.path().join("new.md").exists());
}

// ---------------------------------------------------------------- isolation from groups

fn group_with(world: &FakeGitHub, members: &[&str]) -> (GroupId, FakeRepo) {
    let id = GroupId::new(members[0], group::DEFAULT_REPO);
    let mut gf = GroupFile::new("Friends", members[0]);
    gf.members = members.iter().map(|m| m.to_string()).collect();
    let json = String::from_utf8(gf.to_bytes()).unwrap();
    let repo = FakeRepo::with_files(
        id.clone(),
        &[(group::FILE, &json), (group::CLAUDE_MD, "# Friends rule\n")],
    );
    for m in &members[1..] {
        repo.add_collaborator(m);
    }
    world.add(&repo);
    (id, repo)
}

#[tokio::test]
async fn personal_memory_never_reaches_a_group_and_back() {
    let world = FakeGitHub::new();
    let (gid, group_repo) = group_with(&world, &["ana", "bo"]);
    let pc = dev(&world, "bo", "pc", WIN_HOME);
    pc.join(&gid).await.unwrap();
    let joined = pc.claude_md().unwrap();
    assert!(joined.contains(BLOCK_BEGIN));
    pc.set_claude_md(&format!("{joined}\nI like apples.\n"));
    pc.set_mem(KLAWLY_WIN, "MEMORY.md", "- [A](a.md) — apples\n");
    pc.set_mem(KLAWLY_WIN, "a.md", "apples\n");
    pc.personal_enable().await.unwrap();
    pc.sync_all().await.unwrap();

    // Another device of bo adds a memory; pc pulls it.
    let mac = dev(&world, "bo", "mac", MAC_HOME);
    mac.personal_check_new_device().await.unwrap();
    mac.set_mem(KLAWLY_MAC, "pears.md", "pears\n");
    mac.sync().await;
    pc.sync().await;
    assert_eq!(pc.mem(KLAWLY_WIN, "pears.md").as_deref(), Some("pears\n"));
    pc.sync_all().await.unwrap();

    // The group repo has none of it.
    for (path, bytes) in group_repo.files() {
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !path.starts_with("memory/") && !is_personal_path(&path)
                || path == README
                || path == CLAUDE_MD,
            "{path}"
        );
        assert!(
            !text.contains("apples") && !text.contains("pears"),
            "{path}: {text}"
        );
    }
    // The personal repo has none of the group.
    let repo = personal_repo(&world, "bo");
    let personal_md = repo.file(CLAUDE_MD).unwrap();
    assert_eq!(personal_md, "I like apples.\n");
    for (path, bytes) in repo.files() {
        let text = String::from_utf8_lossy(&bytes);
        assert!(is_personal_path(&path), "{path}");
        assert!(
            !text.contains("Friends rule")
                && !text.contains(".claude-sync/groups")
                && !text.contains(BLOCK_BEGIN),
            "{path}: {text}"
        );
    }
    // Personal files are never offered as group edits, and the block is intact.
    assert!(pc.local_edits().unwrap().is_empty());
    assert!(mac.local_edits().unwrap().is_empty());
    assert_eq!(
        pc.claude_md().unwrap(),
        format!("{joined}\nI like apples.\n")
    );
    // Leaving the group keeps personal text.
    pc.leave(&gid).await.unwrap();
    assert_eq!(pc.claude_md().as_deref(), Some("I like apples.\n"));
    pc.sync().await;
    assert_eq!(repo.file(CLAUDE_MD).as_deref(), Some("I like apples.\n"));
}

#[tokio::test]
async fn another_account_starts_over() {
    let world = FakeGitHub::new();
    let ana = dev(&world, "ana", "pc", WIN_HOME);
    ana.personal_enable().await.unwrap();
    let id = ana.personal_status().device_id;
    let bo = Engine {
        login: "bo".into(),
        remotes: Arc::new(world.account("bo")),
        ..ana.engine
    };
    let st = bo.personal_status();
    assert!(!st.enabled && !st.decided);
    assert_eq!(st.repo, "bo/claude-sync-personal");
    assert_eq!(st.device_id, id);
}

#[test]
fn newer_formats_are_refused() {
    assert!(PersonalFile::parse(br#"{"format": 2}"#).is_err());
    let pf = PersonalFile::parse(br#"{"format": 1, "later": [1]}"#).unwrap();
    assert!(pf.new_devices_join);
    assert!(String::from_utf8(pf.to_bytes()).unwrap().contains("later"));
}

// ---------------------------------------------------------------- personal-state

const MODE_OLD: &str = r#"{"mode":"default","ts":"2026-10-17T04:00:00Z"}"#;
const MODE_NEW: &str = r#"{"mode":"opus-only","ts":"2026-10-17T05:00:00Z"}"#;
const MODE_PATH: &str = "state/routing-mode.json";
const LOG_PATH: &str = "state/routing-mode.jsonl";

fn set(pairs: &[(&str, &str)]) -> FileSet {
    pairs
        .iter()
        .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
        .collect()
}

impl Dev {
    fn state_path(&self, rel: &str) -> PathBuf {
        self.home().join("personal-state").join(rel)
    }
    fn state(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.state_path(rel)).ok()
    }
    fn set_state(&self, rel: &str, text: &str) {
        write_atomic(&self.state_path(rel), text.as_bytes()).unwrap();
    }
}

#[test]
fn state_paths_are_personal() {
    for ok in [MODE_PATH, LOG_PATH, "state/router/x.json"] {
        assert!(is_personal_path(ok), "{ok}");
    }
    for bad in ["state", "state/", "state/.hidden", "state/../x.json"] {
        assert!(!is_personal_path(bad), "{bad}");
    }
}

#[test]
fn state_json_newer_ts_wins_both_ways_without_conflict() {
    let base = set(&[(MODE_PATH, r#"{"mode":"x","ts":"2026-10-17T03:00:00Z"}"#)]);
    // Local newer: pushed.
    let r = merge_personal(
        &base,
        &set(&[(MODE_PATH, MODE_NEW)]),
        &set(&[(MODE_PATH, MODE_OLD)]),
    );
    assert!(r.conflicts.is_empty());
    assert_eq!(r.merged[MODE_PATH], MODE_NEW.as_bytes());
    assert_eq!(r.to_push[MODE_PATH].as_deref(), Some(MODE_NEW.as_bytes()));
    // Remote newer: written locally, nothing pushed.
    let r = merge_personal(
        &base,
        &set(&[(MODE_PATH, MODE_OLD)]),
        &set(&[(MODE_PATH, MODE_NEW)]),
    );
    assert!(r.conflicts.is_empty());
    assert_eq!(r.merged[MODE_PATH], MODE_NEW.as_bytes());
    assert!(r.to_push.is_empty());
    // Equal ts keeps the remote.
    let same = r#"{"mode":"other","ts":"2026-10-17T05:00:00.000+00:00"}"#;
    let r = merge_personal(
        &base,
        &set(&[(MODE_PATH, same)]),
        &set(&[(MODE_PATH, MODE_NEW)]),
    );
    assert!(r.conflicts.is_empty());
    assert_eq!(r.merged[MODE_PATH], MODE_NEW.as_bytes());
    // Offsets are compared as times, not text.
    assert!(state::parse_ts("2026-10-17T06:30:00+02:00") < state::parse_ts("2026-10-17T05:00:00Z"));
}

#[test]
fn state_json_without_ts_is_a_conflict() {
    let base = set(&[(MODE_PATH, r#"{"mode":"a"}"#)]);
    let r = merge_personal(
        &base,
        &set(&[(MODE_PATH, r#"{"mode":"b"}"#)]),
        &set(&[(MODE_PATH, MODE_NEW)]),
    );
    assert_eq!(r.conflicts.len(), 1);
    let r = merge_personal(
        &base,
        &set(&[(MODE_PATH, "not json")]),
        &set(&[(MODE_PATH, MODE_NEW)]),
    );
    assert_eq!(r.conflicts.len(), 1);
}

#[test]
fn state_jsonl_is_a_sorted_union() {
    let a = r#"{"ts":"2026-10-17T01:00:00Z","mode":"opus-only","host":"pc"}"#;
    let b = r#"{"ts":"2026-10-17T02:00:00Z","mode":"default","host":"mac"}"#;
    let c = r#"{"ts":"2026-10-17T03:00:00Z","mode":"opus-only","host":"pc"}"#;
    let local = format!("{a}\n{c}\n\n");
    let remote = format!("{b}\n{a}\nno ts here\n");
    let r = merge_personal(
        &set(&[(LOG_PATH, &format!("{a}\n"))]),
        &set(&[(LOG_PATH, &local)]),
        &set(&[(LOG_PATH, &remote)]),
    );
    assert!(r.conflicts.is_empty());
    let want = format!("no ts here\n{a}\n{b}\n{c}\n");
    assert_eq!(String::from_utf8_lossy(&r.merged[LOG_PATH]), want);
    assert_eq!(r.to_push[LOG_PATH].as_deref(), Some(want.as_bytes()));
}

#[tokio::test]
async fn personal_state_syncs_between_devices() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.set_state("routing-mode.json", MODE_OLD);
    pc.personal_enable().await.unwrap();
    let repo = personal_repo(&world, "ana");
    assert_eq!(repo.file(MODE_PATH).as_deref(), Some(MODE_OLD));

    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.personal_enable().await.unwrap();
    assert_eq!(mac.state("routing-mode.json").as_deref(), Some(MODE_OLD));

    // Both change it; the newer ts wins on both devices, with no conflict.
    mac.set_state("routing-mode.json", MODE_NEW);
    pc.set_state(
        "routing-mode.json",
        r#"{"mode":"x","ts":"2026-10-17T04:30:00Z"}"#,
    );
    mac.sync().await;
    pc.sync().await;
    assert_eq!(pc.state("routing-mode.json").as_deref(), Some(MODE_NEW));
    assert_eq!(repo.file(MODE_PATH).as_deref(), Some(MODE_NEW));
    assert!(pc.personal_status().conflicts.is_empty());

    // The overwritten copy is kept.
    assert!(personal_backups(&pc)
        .iter()
        .any(|(p, t)| p == MODE_PATH && t.contains("04:30")));
    // A deletion syncs too, and the deleted copy is kept.
    std::fs::remove_file(mac.state_path("routing-mode.json")).unwrap();
    mac.sync().await;
    pc.sync().await;
    assert_eq!(pc.state("routing-mode.json"), None);
    assert!(personal_backups(&pc)
        .iter()
        .any(|(p, t)| p == MODE_PATH && (t == MODE_NEW || t.contains("04:30"))));
}

#[tokio::test]
async fn a_git_clone_personal_state_syncs_only_claude_sync() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    for d in [&pc, &mac] {
        std::fs::create_dir_all(d.state_path(".git")).unwrap();
        d.set_state(".git/config", "x");
    }
    pc.personal_enable().await.unwrap();
    mac.personal_enable().await.unwrap();
    let repo = personal_repo(&world, "ana");

    pc.set_state("rules/a.json", MODE_OLD);
    pc.set_state("claude-sync/note.json", MODE_OLD);
    std::thread::sleep(std::time::Duration::from_millis(5));
    pc.sync().await;
    mac.sync().await;
    assert_eq!(repo.file("state/rules/a.json"), None);
    assert_eq!(repo.file("state/.git/config"), None);
    assert_eq!(
        repo.file("state/claude-sync/note.json").as_deref(),
        Some(MODE_OLD)
    );
    assert_eq!(
        mac.state("claude-sync/note.json").as_deref(),
        Some(MODE_OLD)
    );
    assert_eq!(mac.state("rules/a.json"), None);

    // A rule edited on the other device too is no conflict and no remote change.
    mac.set_state("rules/a.json", MODE_NEW);
    pc.set_state("rules/a.json", MODE_OLD);
    std::thread::sleep(std::time::Duration::from_millis(5));
    mac.sync().await;
    pc.sync().await;
    assert!(pc.personal_status().conflicts.is_empty());
    assert!(mac.personal_status().conflicts.is_empty());
    assert_eq!(pc.state("rules/a.json").as_deref(), Some(MODE_OLD));
    assert_eq!(repo.file("state/rules/a.json"), None);
}

#[tokio::test]
async fn a_git_clone_personal_state_clears_old_review_entries() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.personal_enable().await.unwrap();
    let entry = |path: &str| ConflictInfo {
        path: path.into(),
        saved_to: String::new(),
        at_unix: 1,
    };
    let mut st = pc.load_personal();
    st.conflicts = vec![
        entry("state/AGENTS.md"),
        entry(CLAUDE_MD),
        entry("state/claude-sync/note.json"),
    ];
    pc.save_personal(&st).unwrap();

    // Without .git nothing is cleared.
    pc.sync().await;
    assert_eq!(pc.personal_status().conflicts.len(), 3);

    std::fs::create_dir_all(pc.state_path(".git")).unwrap();
    pc.sync().await;
    let paths: Vec<String> = pc
        .personal_status()
        .conflicts
        .into_iter()
        .map(|c| c.path)
        .collect();
    assert_eq!(paths, [CLAUDE_MD, "state/claude-sync/note.json"]);
}

#[tokio::test]
async fn without_git_all_of_personal_state_still_syncs() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.personal_enable().await.unwrap();
    pc.set_state("rules/a.json", MODE_OLD);
    pc.sync().await;
    let repo = personal_repo(&world, "ana");
    assert_eq!(repo.file("state/rules/a.json").as_deref(), Some(MODE_OLD));
}

#[test]
fn kinds_default_includes_state() {
    assert!(Kinds::default().state);
    assert!(Kinds::default().settings);
    let old: Kinds = serde_json::from_str(r#"{"claudeMd":true,"memory":false}"#).unwrap();
    assert_eq!(
        old,
        Kinds {
            claude_md: true,
            memory: false,
            state: true,
            settings: true
        }
    );
}

#[tokio::test]
async fn state_off_keeps_state_local() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.personal_set_kinds(Kinds {
        claude_md: true,
        memory: true,
        state: false,
        settings: true,
    })
    .await
    .unwrap();
    pc.set_state("routing-mode.json", MODE_NEW);
    pc.personal_enable().await.unwrap();
    let repo = personal_repo(&world, "ana");
    assert_eq!(repo.file(MODE_PATH), None);
    repo.set_file("state/other.json", MODE_OLD);
    pc.sync().await;
    assert_eq!(pc.state("other.json"), None);
}

// ---------------------------------------------------------------- settings.json

impl Dev {
    fn settings_value(&self) -> serde_json::Value {
        let b = std::fs::read(self.home().join("settings.json")).unwrap_or(b"{}".to_vec());
        serde_json::from_slice(&b).unwrap()
    }
    fn set_settings(&self, v: serde_json::Value) {
        write_atomic(
            &self.home().join("settings.json"),
            &crate::settings::to_bytes(&v),
        )
        .unwrap();
    }
    fn set_group_owned(&self, ptr: &str, v: serde_json::Value) {
        let mut owned = crate::settings::Owned::default();
        owned.values.insert(ptr.into(), v);
        let all = BTreeMap::from([("org/team".to_string(), owned)]);
        write_atomic(
            &self.paths.state_dir.join(crate::apply::SETTINGS_OWNED_FILE),
            &serde_json::to_vec(&all).unwrap(),
        )
        .unwrap();
    }
}

fn repo_settings(repo: &FakeRepo) -> serde_json::Value {
    serde_json::from_str(&repo.file("settings.json").expect("settings.json pushed")).unwrap()
}

#[tokio::test]
async fn collect_drops_group_values_secrets_and_machine_values() {
    use serde_json::json;
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.set_settings(json!({
        "env": {
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW": "200000",
            "GROUP_KEY": "from-group",
            "TOKEN": "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "CLAUDE_CODE_PLUGIN_DIRS": "/Users/ana/x/mods/a"
        },
        "apiKeyHelper": "/bin/key",
        "statusLine": {"type": "command", "command": "/Users/ana/bin/status.sh"},
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/Users/ana/h.sh"}]}]},
        "permissions": {"additionalDirectories": ["/x/y"], "defaultMode": "plan"},
        "theme": "dark"
    }));
    pc.set_group_owned("/env/GROUP_KEY", json!("from-group"));
    pc.personal_enable().await.unwrap();
    let repo = personal_repo(&world, "ana");
    assert_eq!(
        repo_settings(&repo),
        json!({
            "env": {"CLAUDE_CODE_AUTO_COMPACT_WINDOW": "200000"},
            "permissions": {"defaultMode": "plan"},
            "theme": "dark"
        })
    );
}

#[tokio::test]
async fn a_setting_follows_to_the_other_machine_and_group_keys_win() {
    use serde_json::json;
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.set_settings(json!({"theme": "dark"}));
    pc.personal_enable().await.unwrap();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.set_settings(json!({
        "statusLine": {"type": "command", "command": "/Users/ana/mac.sh"},
        "model": "mac-model"
    }));
    mac.set_group_owned("/model", json!("mac-model"));
    mac.personal_enable().await.unwrap();
    pc.set_settings(json!({"theme": "dark", "model": "mine",
        "env": {"CLAUDE_CODE_AUTO_COMPACT_WINDOW": "1"}}));
    pc.sync().await;
    mac.sync().await;
    let m = mac.settings_value();
    assert_eq!(m["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "1");
    assert_eq!(m["theme"], "dark");
    // The group's value stays, and the machine's own statusLine is untouched.
    assert_eq!(m["model"], "mac-model");
    assert_eq!(m["statusLine"]["command"], "/Users/ana/mac.sh");
    assert!(personal_backups(&mac)
        .iter()
        .any(|(p, _)| p == "settings.json"));
}

#[tokio::test]
async fn a_removed_setting_goes_only_while_unchanged() {
    use serde_json::json;
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.set_settings(json!({"a": 1, "b": 1, "keep": true}));
    pc.personal_enable().await.unwrap();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.personal_enable().await.unwrap();
    assert_eq!(mac.settings_value()["a"], 1);
    mac.set_settings(json!({"a": 1, "b": 2, "keep": true}));
    pc.set_settings(json!({"keep": true}));
    pc.sync().await;
    mac.sync().await;
    let m = mac.settings_value();
    assert_eq!(m.get("a"), None);
    assert_eq!(m["b"], 2, "a changed value is not removed");
    assert_eq!(m["keep"], true);
}

#[test]
fn settings_merge_per_pointer_and_the_pushed_version_wins() {
    use serde_json::json;
    let b = |v: serde_json::Value| Some(crate::settings::to_bytes(&v));
    let base = b(json!({"x": 1, "y": 1, "env": {"A": "1"}}));
    let local = b(json!({"x": 2, "y": 1, "env": {"A": "1", "L": "l"}}));
    let remote = b(json!({"x": 3, "y": 2, "env": {"A": "1", "R": "r"}}));
    let out = settings_file::merge(base.as_ref(), local.as_ref(), remote.as_ref()).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        v,
        json!({"x": 3, "y": 2, "env": {"A": "1", "L": "l", "R": "r"}})
    );
}

// ---------------------------------------------------------------- choices (#119)

fn join(d: &Dev, groups: &[&str]) {
    let mut st = crate::state::AppState::load(&d.paths).unwrap();
    st.groups = groups.iter().map(|g| GroupId::parse(g).unwrap()).collect();
    st.save(&d.paths).unwrap();
}

#[tokio::test]
async fn a_group_turned_off_on_one_device_is_off_on_the_other_after_one_sync() {
    let world = FakeGitHub::new();
    let g = GroupId::parse("ana/team").unwrap();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    join(&pc, &["ana/team"]);
    pc.personal_enable().await.unwrap();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    join(&mac, &["ana/team"]);
    mac.personal_enable().await.unwrap();

    pc.set_group_enabled(&g, false).await.unwrap();
    assert!(crate::state::AppState::load(&pc.paths)
        .unwrap()
        .disabled
        .contains(&g));
    pc.sync().await;
    assert!(!crate::state::AppState::load(&mac.paths)
        .unwrap()
        .disabled
        .contains(&g));
    mac.sync().await;
    assert!(crate::choices::load(&mac.paths).unwrap().group(&g).off);
    assert!(crate::state::AppState::load(&mac.paths)
        .unwrap()
        .disabled
        .contains(&g));
}

#[tokio::test]
async fn the_review_and_auto_run_choices_follow_too_and_the_newer_one_wins() {
    let world = FakeGitHub::new();
    let g = GroupId::parse("ana/team").unwrap();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    join(&pc, &["ana/team"]);
    pc.personal_enable().await.unwrap();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    join(&mac, &["ana/team"]);
    mac.personal_enable().await.unwrap();

    crate::choices::set_group(&pc.paths, &g, |c| {
        c.review_claude_md = Some(false);
        c.auto_run_jobs = true;
    })
    .unwrap();
    pc.sync().await;
    mac.sync().await;
    let c = crate::choices::load(&mac.paths).unwrap().group(&g);
    assert_eq!((c.review_claude_md, c.auto_run_jobs), (Some(false), true));

    // Both change it before syncing: the later change wins on both.
    crate::choices::set_group(&pc.paths, &g, |c| c.auto_run_jobs = false).unwrap();
    // Stamps are milliseconds; in the same one the two would tie (and the remote wins).
    std::thread::sleep(std::time::Duration::from_millis(5));
    crate::choices::set_group(&mac.paths, &g, |c| c.review_claude_md = Some(true)).unwrap();
    pc.sync().await;
    mac.sync().await;
    pc.sync().await;
    for d in [&pc, &mac] {
        let c = crate::choices::load(&d.paths).unwrap().group(&g);
        assert_eq!(c.review_claude_md, Some(true), "{}", d.device);
    }
}

#[tokio::test]
async fn turning_personal_sync_off_on_one_device_stops_the_other_at_its_next_sync() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.personal_enable().await.unwrap();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.personal_enable().await.unwrap();
    // Stamps are milliseconds; in the same one the two would tie (and the remote wins).
    std::thread::sleep(std::time::Duration::from_millis(5));

    let st = pc.personal_disable().await.unwrap();
    assert!(!st.enabled);
    assert_eq!(st.new_devices_join, Some(false));
    assert!(mac.personal_status().enabled);
    let report = mac.personal_sync(false).await.unwrap().expect("still on");
    assert_eq!(report.error, None);
    assert!(!mac.personal_status().enabled);
    assert!(mac.personal_sync(false).await.unwrap().is_none());

    // Turning it on again here is newer than the old "off".
    assert!(mac.personal_enable().await.unwrap().enabled);
    mac.sync().await;
    assert!(mac.personal_status().enabled);
    assert!(!crate::choices::load(&mac.paths).unwrap().personal.off);
}

// ---------------------------------------------------------------- more choices (#127)

#[tokio::test]
async fn leaving_and_rejoining_follow_the_newer_choice_and_the_order_follows() {
    let world = FakeGitHub::new();
    let (g1, _) = group_with(&world, &["ana"]);
    let (g2, _) = group_with(&world, &["bo", "ana"]);
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.join(&g1).await.unwrap();
    pc.join(&g2).await.unwrap();
    pc.personal_enable().await.unwrap();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.personal_enable().await.unwrap();
    mac.sync().await;
    let joined = |d: &Dev| crate::state::AppState::load(&d.paths).unwrap().groups;
    assert_eq!(
        joined(&mac),
        [g1.clone(), g2.clone()],
        "joined by the pc's choice"
    );

    // The order set on the pc reaches the mac.
    pc.reorder_groups(vec![g2.clone(), g1.clone()])
        .await
        .unwrap();
    pc.sync().await;
    mac.sync().await;
    assert_eq!(joined(&mac), [g2.clone(), g1.clone()]);

    // Leaving on the pc leaves on the mac.
    pc.leave(&g1).await.unwrap();
    pc.sync().await;
    mac.sync().await;
    assert_eq!(joined(&mac), std::slice::from_ref(&g2));

    // Rejoining on the mac (a newer choice) rejoins on the pc.
    std::thread::sleep(std::time::Duration::from_millis(5));
    mac.join(&g1).await.unwrap();
    mac.sync().await;
    pc.sync().await;
    assert!(joined(&pc).contains(&g1));
    assert_eq!(joined(&pc), joined(&mac));
}

#[tokio::test]
async fn keep_one_on_one_device_leaves_the_line_out_on_the_other() {
    let world = FakeGitHub::new();
    let (g, _) = group_with(&world, &["ana"]);
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.join(&g).await.unwrap();
    pc.personal_enable().await.unwrap();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.personal_enable().await.unwrap();
    mac.sync().await;
    assert_eq!(mac.claude_md_review(&g).unwrap().declined, 0);

    pc.drop_claude_md_line(
        &crate::conflicts::Source::Group { id: g.clone() },
        "# Friends rule",
    )
    .await
    .unwrap();
    pc.sync().await;
    mac.sync().await;
    assert_eq!(mac.claude_md_review(&g).unwrap().declined, 1);
    assert!(!mac
        .claude_md()
        .unwrap_or_default()
        .contains("# Friends rule"));
}

#[tokio::test]
async fn pause_and_the_interval_set_on_one_device_are_in_choices_on_the_other() {
    let world = FakeGitHub::new();
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.personal_enable().await.unwrap();
    let mac = dev(&world, "ana", "mac", MAC_HOME);
    mac.personal_enable().await.unwrap();
    crate::choices::set_app(&pc.paths, |a| {
        a.paused = true;
        a.interval_secs = Some(300);
        a.start_at_login = Some(false);
    })
    .unwrap();
    pc.sync().await;
    mac.sync().await;
    let a = crate::choices::load(&mac.paths).unwrap().app;
    assert_eq!(
        (a.paused, a.interval_secs, a.start_at_login),
        (true, Some(300), Some(false))
    );

    // The later change wins on both.
    std::thread::sleep(std::time::Duration::from_millis(5));
    crate::choices::set_app(&mac.paths, |a| a.paused = false).unwrap();
    mac.sync().await;
    pc.sync().await;
    assert!(!crate::choices::load(&pc.paths).unwrap().app.paused);
}

#[tokio::test]
async fn a_choices_file_from_v0_1_13_is_filled_in_once() {
    let world = FakeGitHub::new();
    let (g1, _) = group_with(&world, &["ana"]);
    let (g2, _) = group_with(&world, &["bo", "ana"]);
    let pc = dev(&world, "ana", "pc", WIN_HOME);
    pc.join(&g1).await.unwrap();
    pc.join(&g2).await.unwrap();
    // What v0.1.13 left behind: choices with only off and ts, and the old jobs.json.
    let old = serde_json::json!({"groups": {
        g1.to_string(): {"off": false, "ts": 5},
        g2.to_string(): {"off": false, "ts": 5}}});
    std::fs::write(crate::choices::file(&pc.paths), old.to_string()).unwrap();
    let jobs = serde_json::json!({"jobs": {"setup-scout": {
        "group": {"owner": g1.owner, "repo": g1.repo}, "accepted": "abc123", "enabled": false}}});
    std::fs::write(crate::jobs::state_file(&pc.paths), jobs.to_string()).unwrap();

    pc.apply_group_choices().await;

    let c = crate::choices::load(&pc.paths).unwrap();
    assert_eq!(c.group(&g1).member, Some(true));
    assert_eq!(c.group(&g2).member, Some(true));
    assert_eq!(c.order.list, [g1.to_string(), g2.to_string()]);
    assert_eq!(c.group(&g1).accepted_jobs["setup-scout"], "abc123");
    assert!(c.group(&g1).off_jobs.contains("setup-scout"));
    let joined = crate::state::AppState::load(&pc.paths).unwrap().groups;
    assert_eq!(joined, [g1, g2], "no group was left");
}
