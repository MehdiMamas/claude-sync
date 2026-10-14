//! End to end against real GitHub: the real `GitHubRepo`/`GitHubRemotes`, the real apply layer,
//! and two "devices" (separate `~/.claude` and state dirs) signed in with the same token.
//!
//! Ignored by default. Run with:
//! `CLAUDE_SYNC_E2E_TOKEN=$(gh auth token) cargo test -p claude-sync-core --test github_e2e -- --ignored`
//! The token needs `repo` and `delete_repo`. Each run creates one private repo named
//! `claude-sync-e2e-<hex>` on the token's account and deletes exactly that repo at the end,
//! also when the test fails.
use claude_sync_core::engine::CommitInfo;
use claude_sync_core::github::GitHub;
use claude_sync_core::remote::{Changes, Collaborator, GitHubRepo, RemoteRepo};
use claude_sync_core::watch::EditAction;
use claude_sync_core::{Engine, Error, GroupId, Paths};
use reqwest::Method;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PREFIX: &str = "claude-sync-e2e-";
const TOKEN_VAR: &str = "CLAUDE_SYNC_E2E_TOKEN";

/// Deletes the run's repo when dropped, on success and on panic.
struct Cleanup {
    token: String,
    id: GroupId,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        // Never anything but the repo this run named.
        assert!(
            self.id.repo.starts_with(PREFIX) && self.id.repo.len() > PREFIX.len(),
            "refusing to delete {}",
            self.id
        );
        let (token, id) = (self.token.clone(), self.id.clone());
        let done = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let path = format!("/repos/{}/{}", id.owner, id.repo);
                match GitHub::new(token).call(Method::DELETE, &path, None).await {
                    Ok(()) | Err(Error::GitHub { status: 404, .. }) => {
                        eprintln!("e2e: deleted {id}")
                    }
                    Err(e) => eprintln!("e2e: could not delete {id}: {e}"),
                }
            })
        })
        .join();
        if done.is_err() {
            eprintln!("e2e: cleanup thread panicked");
        }
    }
}

fn random_hex() -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    );
    h.write_u32(std::process::id());
    format!("{:012x}", h.finish() & 0xffff_ffff_ffff)
}

fn device(token: &str, login: &str, name: &str, root: &Path) -> Engine {
    let paths = Paths {
        claude_home: root.join(name).join("claude"),
        state_dir: root.join(name).join("state"),
    };
    Engine::new(paths, GitHub::new(token.into()), login.into(), name.into())
}

fn home_file(e: &Engine, rel: &str) -> std::path::PathBuf {
    rel.split('/')
        .fold(e.paths.claude_home.clone(), |p, s| p.join(s))
}

fn read(e: &Engine, rel: &str) -> Option<String> {
    std::fs::read_to_string(home_file(e, rel)).ok()
}

fn write(e: &Engine, rel: &str, text: &str) {
    let p = home_file(e, rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn settings(e: &Engine) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(e.paths.claude_home.join("settings.json")).unwrap())
        .unwrap()
}

fn set_env(e: &Engine, key: &str, value: &str) {
    let mut s = settings(e);
    s["env"][key] = value.into();
    std::fs::write(
        e.paths.claude_home.join("settings.json"),
        serde_json::to_vec_pretty(&s).unwrap(),
    )
    .unwrap();
}

/// Share every pending local edit of `e`.
async fn share_all(e: &Engine) {
    let edits = e.local_edits().unwrap();
    assert!(!edits.is_empty(), "{}: nothing to share", e.device);
    for edit in &edits {
        e.resolve_edit(edit, EditAction::Share)
            .await
            .unwrap_or_else(|err| panic!("{}: share {}: {err}", e.device, edit.path));
    }
}

async fn sync_ok(e: &Engine) {
    for g in e.sync_all().await.unwrap().groups {
        assert_eq!(g.error, None, "{}: sync of {}", e.device, g.id);
        assert!(g.conflicts.is_empty(), "{}: {:?}", e.device, g.conflicts);
    }
}

/// GitHub reads can lag a write for a moment, so checks across writers retry for a while.
const TRIES: usize = 20;

async fn pause() {
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// Sync `e` until `check` holds.
async fn pull_until(e: &Engine, what: &str, check: impl Fn(&Engine) -> bool) {
    for _ in 0..TRIES {
        sync_ok(e).await;
        if check(e) {
            return;
        }
        pause().await;
    }
    panic!("{}: never got {what}", e.device);
}

/// The group's file at its head, read by a fresh client until `check` holds.
async fn remote_until(
    gh: &GitHub,
    id: &GroupId,
    path: &str,
    check: impl Fn(Option<&str>) -> bool,
) -> Option<String> {
    let mut last = None;
    for _ in 0..TRIES {
        let repo = GitHubRepo::new(gh.clone(), id.clone());
        let head = repo.head().await.unwrap();
        let files = repo.files_at(&head.commit).await.unwrap();
        last = files
            .get(path)
            .map(|b| String::from_utf8_lossy(b).into_owned());
        if check(last.as_deref()) {
            return last;
        }
        pause().await;
    }
    panic!("remote {path} never matched; last: {last:?}");
}

/// The group's last 5 commits once the newest has `message`.
async fn commits_until(e: &Engine, id: &GroupId, message: &str) -> Vec<CommitInfo> {
    for _ in 0..TRIES {
        let commits = e.recent_commits(id, 5).await.unwrap();
        if commits.first().is_some_and(|c| c.message == message) {
            return commits;
        }
        pause().await;
    }
    panic!("newest commit never became {message:?}");
}

fn remote_env(json: Option<&str>) -> serde_json::Value {
    json.map_or(serde_json::Value::Null, |j| {
        serde_json::from_str::<serde_json::Value>(j).unwrap()["env"].clone()
    })
}

#[tokio::test]
#[ignore = "real GitHub; run with CLAUDE_SYNC_E2E_TOKEN"]
async fn two_devices_against_real_github() {
    let Some(token) = std::env::var(TOKEN_VAR).ok().filter(|t| !t.is_empty()) else {
        eprintln!("e2e: {TOKEN_VAR} is not set, skipping");
        return;
    };
    let gh = GitHub::new(token.clone());
    let login = gh.user().await.expect("token works").login;
    let name = format!("{PREFIX}{}", random_hex());
    let id = GroupId::new(&login, &name);
    let _cleanup = Cleanup {
        token: token.clone(),
        id: id.clone(),
    };
    eprintln!("e2e: using {id}");

    let tmp = tempfile::tempdir().unwrap();
    let a = device(&token, &login, "pc-a", tmp.path());
    let b = device(&token, &login, "pc-b", tmp.path());
    let raw = GitHubRepo::new(gh.clone(), id.clone());

    // Create: private, tagged, initial commit on an empty repo, joined.
    let status = a.create_group("E2E", &name).await.expect("create group");
    assert_eq!(status.id, id);
    assert_eq!(status.last_error, None);
    assert_eq!(status.host, login);
    assert_eq!(status.members, vec![login.clone()]);
    assert!(raw.is_private().await.unwrap());
    assert!(gh
        .repo_topics(&id)
        .await
        .unwrap()
        .contains(&"claude-sync-group".to_string()));
    let head = raw.head().await.unwrap();
    let files = raw.files_at(&head.commit).await.unwrap();
    for f in ["claude-sync.json", "CLAUDE.md", "settings.shared.json"] {
        assert!(files.contains_key(f), "missing {f}: {:?}", files.keys());
    }
    let readme: serde_json::Value = gh
        .get(&format!("/repos/{login}/{name}/contents/README.md"))
        .await
        .expect("README.md in the repo");
    assert_eq!(readme["type"], "file");
    // Two commits, with different messages (#44).
    let created = commits_until(&a, &id, &format!("{login}@pc-a: create group")).await;
    assert_eq!(
        created.get(1).map(|c| c.message.as_str()),
        Some(format!("{login}@pc-a: create group (start the repo)").as_str())
    );

    // Seed content as another client would, then A pulls it.
    let seed: Changes = [
        ("agents/e2e-a.md", "agent a v1\n"),
        ("agents/e2e-gone.md", "to be deleted\n"),
        ("commands/e2e.md", "command v1\n"),
        ("skills/e2e/SKILL.md", "---\nname: e2e\n---\nskill v1\n"),
        (
            "settings.shared.json",
            "{\n  \"env\": {\n    \"E2E_A\": \"1\",\n    \"E2E_B\": \"1\"\n  }\n}\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (p.to_string(), Some(c.as_bytes().to_vec())))
    .collect();
    raw.commit(&head, &seed, &format!("{login}@seed: seed"))
        .await
        .expect("seed commit");
    let v1 = |e: &Engine| read(e, "agents/e2e-a.md").as_deref() == Some("agent a v1\n");
    pull_until(&a, "the seed", v1).await;
    assert_eq!(settings(&a)["env"]["E2E_A"], "1");

    // B joins and applies.
    let joined = b.join(&id).await.expect("join");
    assert_eq!(joined.last_error, None);
    pull_until(&b, "the seed", v1).await;
    assert_eq!(
        read(&b, "skills/e2e/SKILL.md").as_deref(),
        Some("---\nname: e2e\n---\nskill v1\n")
    );
    assert_eq!(read(&b, "commands/e2e.md").as_deref(), Some("command v1\n"));
    assert!(read(&b, "CLAUDE.md").is_some(), "import line written");
    assert!(b.local_edits().unwrap().is_empty());

    // A shares an edit, B pulls it.
    write(&a, "agents/e2e-a.md", "agent a v2\n");
    share_all(&a).await;
    remote_until(&gh, &id, "agents/e2e-a.md", |c| c == Some("agent a v2\n")).await;
    commits_until(&a, &id, &format!("{login}@pc-a: update agents/e2e-a.md")).await;
    pull_until(&b, "A's edit", |e| {
        read(e, "agents/e2e-a.md").as_deref() == Some("agent a v2\n")
    })
    .await;

    // ETag: unchanged remote answers 304, so the 60 s loop does nothing.
    let etag = GitHubRepo::new(gh.clone(), id.clone());
    assert!(etag.head_if_changed().await.unwrap().is_some());
    assert!(etag.head_if_changed().await.unwrap().is_none(), "304");
    assert!(b.sync_if_changed().await.unwrap().groups.is_empty());

    // A real non-fast-forward: two writers from the same head.
    let racer = GitHubRepo::new(gh.clone(), id.clone());
    let one: Changes = [("commands/race.md".to_string(), Some(b"one\n".to_vec()))].into();
    let two: Changes = [("commands/race.md".to_string(), Some(b"two\n".to_vec()))].into();
    let mut head = racer.head().await.unwrap();
    for _ in 0..TRIES {
        match racer
            .commit(&head, &one, &format!("{login}@race: one"))
            .await
        {
            Ok(_) => break,
            // A lagging read gave an old head; that is a race too.
            Err(Error::PushRace) => {
                pause().await;
                head = racer.head().await.unwrap();
            }
            Err(e) => panic!("race one: {e}"),
        }
    }
    let err = racer
        .commit(&head, &two, &format!("{login}@race: two"))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PushRace), "{err}");
    let raced = |e: &Engine| read(e, "commands/race.md").as_deref() == Some("one\n");
    pull_until(&a, "the race winner", raced).await;
    pull_until(&b, "the race winner", raced).await;

    // Concurrent shares from both devices on different files: one retries, both land.
    write(&a, "agents/e2e-a.md", "agent a v3\n");
    write(&b, "commands/e2e.md", "command v2 from b\n");
    let (ea, eb) = (a.local_edits().unwrap(), b.local_edits().unwrap());
    assert_eq!(ea.len(), 1, "{ea:?}");
    assert_eq!(eb.len(), 1, "{eb:?}");
    let (ra, rb) = tokio::join!(
        a.resolve_edit(&ea[0], EditAction::Share),
        b.resolve_edit(&eb[0], EditAction::Share)
    );
    ra.expect("a shares");
    rb.expect("b shares");
    remote_until(&gh, &id, "agents/e2e-a.md", |c| c == Some("agent a v3\n")).await;
    remote_until(&gh, &id, "commands/e2e.md", |c| {
        c == Some("command v2 from b\n")
    })
    .await;
    let both = |e: &Engine| {
        read(e, "agents/e2e-a.md").as_deref() == Some("agent a v3\n")
            && read(e, "commands/e2e.md").as_deref() == Some("command v2 from b\n")
    };
    for e in [&a, &b] {
        pull_until(e, "both shares", both).await;
        assert!(e.local_edits().unwrap().is_empty(), "{}", e.device);
    }

    // Settings: each device changes its own key; the group ends with both.
    set_env(&a, "E2E_A", "2");
    share_all(&a).await;
    set_env(&b, "E2E_B", "2");
    share_all(&b).await;
    remote_until(&gh, &id, "settings.shared.json", |c| {
        let env = remote_env(c);
        env["E2E_A"] == "2" && env["E2E_B"] == "2"
    })
    .await;
    let merged = |e: &Engine| {
        let env = &settings(e)["env"];
        env["E2E_A"] == "2" && env["E2E_B"] == "2"
    };
    pull_until(&a, "both settings", merged).await;
    pull_until(&b, "both settings", merged).await;

    // Delete: A deletes a file and shares the deletion; B loses it too.
    std::fs::remove_file(home_file(&a, "agents/e2e-gone.md")).unwrap();
    let edits = a.local_edits().unwrap();
    assert!(edits.iter().all(|e| e.deleted), "{edits:?}");
    share_all(&a).await;
    remote_until(&gh, &id, "agents/e2e-gone.md", |c| c.is_none()).await;
    pull_until(&b, "the delete", |e| {
        read(e, "agents/e2e-gone.md").is_none()
    })
    .await;

    // History.
    let commits = commits_until(&b, &id, &format!("{login}@pc-a: remove agents/e2e-gone.md")).await;
    assert_eq!(commits.len(), 5);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for c in &commits {
        assert_eq!(c.author, login);
        assert!(c.at_unix + 600 > now && c.at_unix < now + 600, "{c:?}");
    }

    // Members as the host sees them: only the owner, no invitations.
    let collaborators = raw.collaborators().await.unwrap();
    assert!(collaborators.contains(&Collaborator::Member(login.clone())));
    assert!(!collaborators
        .iter()
        .any(|c| matches!(c, Collaborator::Invited { .. })));
    let groups = a.groups().await.unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].invited, Vec::<String>::new());
    assert_eq!(groups[0].last_error, None);

    // Discovery lists the repo (the topic may take a moment to be indexed).
    let mut found = false;
    for _ in 0..5 {
        if gh.group_repos().await.unwrap().contains(&id) {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(found, "{id} not in group_repos()");

    // Leave removes B's managed content.
    b.leave(&id).await.unwrap();
    assert_eq!(read(&b, "agents/e2e-a.md"), None);
    assert!(b.groups().await.unwrap().is_empty());
}
