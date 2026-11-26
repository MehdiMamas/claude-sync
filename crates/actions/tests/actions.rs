//! Each command against a fake GitHub (wiremock) and a temp home; the loop with a fake clock.
use claude_sync_actions::*;
use claude_sync_github::secrets::SecretStore;
use claude_sync_github::{Config, Result as GhResult};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Default)]
struct Map(Mutex<HashMap<String, String>>);
impl SecretStore for Map {
    fn get(&self, a: &str) -> GhResult<Option<String>> {
        Ok(self.0.lock().unwrap().get(a).cloned())
    }
    fn set(&self, a: &str, s: &str) -> GhResult<()> {
        self.0.lock().unwrap().insert(a.into(), s.into());
        Ok(())
    }
    fn delete(&self, a: &str) -> GhResult<()> {
        self.0.lock().unwrap().remove(a);
        Ok(())
    }
}

struct T {
    server: MockServer,
    dir: tempfile::TempDir,
    clock: Arc<AtomicI64>,
    secrets: Arc<Map>,
    jobs: Mutex<Option<claude_sync_actions::jobs::JobsEnv>>,
    device: &'static str,
}

impl T {
    async fn new() -> T {
        T::named("laptop").await
    }
    async fn named(device: &'static str) -> T {
        T {
            device,
            server: MockServer::start().await,
            dir: tempfile::tempdir().unwrap(),
            clock: Arc::new(AtomicI64::new(1_000_000)),
            secrets: Arc::default(),
            jobs: Mutex::new(None),
        }
    }
    fn env(&self, client_id: &str) -> Env {
        let uri = self.server.uri();
        let clock = self.clock.clone();
        Env {
            home: self.dir.path().into(),
            cfg: Config {
                api: uri.clone(),
                login: uri.clone(),
                relay: uri,
                releases: String::new(),
                client_id: client_id.into(),
                retry_delay: Duration::from_millis(1),
            },
            app_client_id: String::new(),
            secrets: claude_sync_actions::Secrets(self.secrets.clone()),
            now: Arc::new(move || clock.load(Ordering::SeqCst)),
            device: format!("ann@{}", self.device),
        }
    }
    async fn mock(&self, verb: &str, p: &str, status: u16, body: serde_json::Value) {
        Mock::given(method(verb))
            .and(path(p))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&self.server)
            .await;
    }
    async fn signed_in(&self) -> Handle {
        self.signed_in_tick(Duration::from_secs(3600)).await
    }
    async fn signed_in_tick(&self, tick: Duration) -> Handle {
        self.mock("GET", "/user", 200, json!({"login": "ann", "id": 1}))
            .await;
        self.secrets
            .set("ann", &claude_sync_github::Tokens::plain("tok").to_secret())
            .unwrap();
        self.secrets.set("@current", "ann").unwrap();
        let jobs = self.jobs.lock().unwrap().clone();
        spawn_with(self.env("cid"), tick, jobs).await.unwrap()
    }
    /// The Git Data calls of one push to `o/r` on `main`.
    async fn push_chain(&self, repo: &str) {
        let r = format!("/repos/{repo}");
        self.mock(
            "GET",
            &format!("{r}/commits/main"),
            200,
            json!({"sha": "c1"}),
        )
        .await;
        self.mock("GET", &format!("{r}/git/trees/c1"), 200, json!({"sha": "t0", "tree": [{"path": "claude-sync.json", "mode": "100644", "type": "blob", "sha": "g1"}]})).await;
        let body =
            serde_json::to_string_pretty(&json!({"schema": 1, "name": "Team", "members": ["ann"]}))
                .unwrap();
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, body);
        self.mock(
            "GET",
            &format!("{r}/git/blobs/g1"),
            200,
            json!({"content": b64, "encoding": "base64"}),
        )
        .await;
        self.mock("POST", &format!("{r}/git/blobs"), 201, json!({"sha": "b2"}))
            .await;
        self.mock("POST", &format!("{r}/git/trees"), 201, json!({"sha": "t1"}))
            .await;
        self.mock(
            "POST",
            &format!("{r}/git/commits"),
            201,
            json!({"sha": "c2"}),
        )
        .await;
        self.mock("PATCH", &format!("{r}/git/refs/heads/main"), 200, json!({}))
            .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_cancel_and_sign_out() {
    let t = T::new().await;
    t.mock("POST", "/login/device/code", 200, json!({"device_code": "d", "user_code": "ABCD-1234", "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 1})).await;
    t.mock(
        "POST",
        "/login/oauth/access_token",
        200,
        json!({"error": "authorization_pending"}),
    )
    .await;
    let h = spawn(t.env("cid"), Duration::from_secs(3600))
        .await
        .unwrap();
    let Event::DeviceCode { user_code, .. } =
        h.send(Command::SignInStart { app: false }).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(user_code, "ABCD-1234");
    assert!(matches!(h.snapshot().sign_in, SignInState::Code { .. }));
    h.send(Command::SignInCancel).await.unwrap();
    for _ in 0..40 {
        if h.snapshot().sign_in == SignInState::SignedOut {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(h.snapshot().sign_in, SignInState::SignedOut);
    assert!(h.snapshot().login.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_completes_and_sign_out_forgets() {
    let t = T::new().await;
    t.mock("POST", "/login/device/code", 200, json!({"device_code": "d", "user_code": "WXYZ", "verification_uri": "u", "expires_in": 900, "interval": 1})).await;
    t.mock(
        "POST",
        "/login/oauth/access_token",
        200,
        json!({"access_token": "tok", "token_type": "bearer"}),
    )
    .await;
    t.mock("GET", "/user", 200, json!({"login": "ann", "id": 1}))
        .await;
    let h = spawn(t.env("cid"), Duration::from_secs(3600))
        .await
        .unwrap();
    h.send(Command::SignInStart { app: false }).await.unwrap();
    for _ in 0..60 {
        if h.snapshot().login.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(h.snapshot().login.as_deref(), Some("ann"));
    assert_eq!(t.secrets.get("@current").unwrap().as_deref(), Some("ann"));
    h.send(Command::SignOut).await.unwrap();
    assert!(h.snapshot().login.is_none());
    assert!(t.secrets.get("@current").unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn no_client_id_and_no_app_say_so() {
    let t = T::new().await;
    let h = spawn(t.env(""), Duration::from_secs(3600)).await.unwrap();
    let e = h
        .send(Command::SignInStart { app: false })
        .await
        .unwrap_err();
    assert!(e.0.contains("built without a GitHub client ID"));
    let e = h
        .send(Command::SignInStart { app: true })
        .await
        .unwrap_err();
    assert_eq!(e.0, "This build has no GitHub App.");
}

#[tokio::test(flavor = "multi_thread")]
async fn restores_the_saved_session() {
    let t = T::new().await;
    let h = t.signed_in().await;
    assert_eq!(h.snapshot().login.as_deref(), Some("ann"));
}

#[tokio::test(flavor = "multi_thread")]
async fn create_group_makes_repo_topic_ruleset_and_first_commit() {
    let t = T::new().await;
    let h = t.signed_in().await;
    t.mock(
        "POST",
        "/user/repos",
        201,
        json!({"name": "team", "full_name": "ann/team", "private": true, "default_branch": "main"}),
    )
    .await;
    t.mock(
        "PUT",
        "/repos/ann/team/topics",
        200,
        json!({"names": ["claude-sync-group"]}),
    )
    .await;
    t.mock("GET", "/repos/ann/team/rulesets", 200, json!([]))
        .await;
    t.mock("POST", "/repos/ann/team/rulesets", 201, json!({"id": 1}))
        .await;
    t.push_chain("ann/team").await;
    let e = h
        .send(Command::CreateGroup {
            name: "Team".into(),
            repo: "team".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        e,
        Event::Created {
            repo: "ann/team".into(),
            lock: "on".into()
        }
    );
    let g = &h.snapshot().groups;
    assert_eq!(g.len(), 1);
    assert!(g[0].is_host && g[0].repo == "ann/team");
    // the ruleset is not created twice
    let posts = t
        .server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/rulesets"))
        .count();
    assert_eq!(posts, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn members_transfer_leave_and_host_rules() {
    let t = T::new().await;
    let h = t.signed_in().await;
    t.mock(
        "GET",
        "/repos/ann/team",
        200,
        json!({"name": "team", "full_name": "ann/team", "private": true}),
    )
    .await;
    t.push_chain("ann/team").await;
    let e = h
        .send(Command::Join {
            repo: "ann/team".into(),
        })
        .await
        .unwrap();
    let Event::Joined { warning, .. } = e else {
        panic!()
    };
    assert!(warning.contains("run code on every member's machine"));
    assert!(h.snapshot().groups[0].is_host);

    // bad login
    let e = h
        .send(Command::AddMember {
            repo: "ann/team".into(),
            login: "bad name".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(e.0, "\"bad name\" is not a GitHub login");
    // add
    Mock::given(method("PUT"))
        .and(path_regex("/repos/ann/team/collaborators/bob"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&t.server)
        .await;
    let e = h
        .send(Command::AddMember {
            repo: "ann/team".into(),
            login: "bob".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        e,
        Event::Message("Invited bob. They get an invitation from GitHub.".into())
    );
    // the commit message carries login@device
    let reqs = t.server.received_requests().await.unwrap();
    let c = reqs
        .iter()
        .find(|r| r.url.path().ends_with("/git/commits"))
        .unwrap();
    assert!(String::from_utf8_lossy(&c.body).contains("ann@laptop: add bob"));
    // host cannot be removed or leave; transfer needs a member
    let e = h
        .send(Command::RemoveMember {
            repo: "ann/team".into(),
            login: "ann".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(e.0, "ann hosts ann/team and cannot be removed");
    let e = h
        .send(Command::Leave {
            repo: "ann/team".into(),
        })
        .await
        .unwrap_err();
    assert!(e.0.starts_with("You host ann/team"));
    let e = h
        .send(Command::Transfer {
            repo: "ann/team".into(),
            new_owner: "ann".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(e.0, "you already host ann/team");
    let e = h
        .send(Command::Transfer {
            repo: "ann/team".into(),
            new_owner: "zed".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(e.0, "zed is not a member of ann/team; add them first");
}

#[tokio::test(flavor = "multi_thread")]
async fn join_refuses_public_repos_and_non_groups_and_leave_removes() {
    let t = T::new().await;
    let h = t.signed_in().await;
    t.mock(
        "GET",
        "/repos/zed/pub",
        200,
        json!({"name": "pub", "full_name": "zed/pub", "private": false}),
    )
    .await;
    let e = h
        .send(Command::Join {
            repo: "zed/pub".into(),
        })
        .await
        .unwrap_err();
    assert!(e.0.contains("is public"));
    t.mock(
        "GET",
        "/repos/zed/plain",
        200,
        json!({"name": "plain", "full_name": "zed/plain", "private": true}),
    )
    .await;
    t.mock(
        "GET",
        "/repos/zed/plain/commits/main",
        200,
        json!({"sha": "c1"}),
    )
    .await;
    t.mock(
        "GET",
        "/repos/zed/plain/git/trees/c1",
        200,
        json!({"sha": "t", "tree": []}),
    )
    .await;
    let e = h
        .send(Command::Join {
            repo: "zed/plain".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        e.0,
        "zed/plain is not a claude-sync group (no claude-sync.json)"
    );
    // a real group by someone else: join, reorder, off, leave
    t.mock(
        "GET",
        "/repos/zed/team",
        200,
        json!({"name": "team", "full_name": "zed/team", "private": true}),
    )
    .await;
    t.push_chain("zed/team").await;
    h.send(Command::Join {
        repo: "zed/team".into(),
    })
    .await
    .unwrap();
    assert!(!h.snapshot().groups[0].is_host);
    let e = h
        .send(Command::ReorderGroups(vec!["a/b".into()]))
        .await
        .unwrap_err();
    assert_eq!(e.0, "the new order must list exactly the joined groups");
    h.send(Command::SetGroupOff {
        repo: "zed/team".into(),
        off: true,
    })
    .await
    .unwrap();
    assert_eq!(h.snapshot().groups[0].status, GroupStatus::Off);
    let e = h
        .send(Command::Leave {
            repo: "zed/team".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        e,
        Event::Message("Left team. Its files were removed from this device.".into())
    );
    assert!(h.snapshot().groups.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn discover_lists_unjoined_private_groups() {
    let t = T::new().await;
    let h = t.signed_in().await;
    t.mock(
        "GET",
        "/user/repos",
        200,
        json!([
        {"name": "a", "full_name": "zed/a", "private": true, "topics": ["claude-sync-group"]},
        {"name": "b", "full_name": "zed/b", "private": true, "topics": []},
        {"name": "c", "full_name": "zed/c", "private": false, "topics": ["claude-sync-group"]}]),
    )
    .await;
    assert_eq!(
        h.send(Command::Discover).await.unwrap(),
        Event::Discovered(vec!["zed/a".into()])
    );
    assert_eq!(h.snapshot().discoverable, vec!["zed/a".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn invitations_and_pause_and_a_401_stops_the_loop() {
    let t = T::new().await;
    let h = t.signed_in().await;
    t.mock("GET", "/user/repository_invitations", 200, json!([{"id": 7, "repository": {"name": "t", "full_name": "zed/t", "private": true}, "inviter": {"login": "zed"}}])).await;
    h.send(Command::WindowOpened).await.unwrap();
    assert_eq!(
        h.snapshot().invitations,
        vec![InvitationView {
            id: 7,
            repo: "zed/t".into(),
            inviter: "zed".into()
        }]
    );
    h.send(Command::SetInterval { secs: 5 }).await.unwrap();
    assert_eq!(h.snapshot().interval_secs, 30); // clamped to 30 s .. 15 min
    h.send(Command::Pause).await.unwrap();
    assert!(h.snapshot().paused);
    h.send(Command::Resume).await.unwrap();
    // GitHub says 401: the loop stops with a visible notice
    t.server.reset().await;
    t.mock(
        "GET",
        "/user/repository_invitations",
        401,
        json!({"message": "Bad credentials"}),
    )
    .await;
    t.mock("GET", "/user", 401, json!({"message": "Bad credentials"}))
        .await;
    t.clock.fetch_add(3600, Ordering::SeqCst);
    h.send(Command::WindowOpened).await.unwrap();
    let s = h.snapshot();
    assert_eq!(s.session_expired.as_deref(), Some("ann"));
    assert!(s.notifications.iter().any(|n| n
        .text
        .contains("GitHub no longer accepts the sign-in of ann")));
}

fn b64(s: &str) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, s)
}

impl T {
    /// A joined group `zed/team` whose main holds claude-sync.json and the given files.
    async fn group_with(&self, head: &str, files: &[(&str, &str)]) -> Handle {
        self.group_with_tick(head, files, Duration::from_secs(3600))
            .await
    }
    async fn group_with_tick(&self, head: &str, files: &[(&str, &str)], tick: Duration) -> Handle {
        let h = self.signed_in_tick(tick).await;
        let r = "/repos/zed/team";
        self.mock(
            "GET",
            r,
            200,
            json!({"name": "team", "full_name": "zed/team", "private": true}),
        )
        .await;
        self.push_chain("zed/team").await;
        h.send(Command::Join {
            repo: "zed/team".into(),
        })
        .await
        .unwrap();
        self.serve(head, files).await;
        h
    }
    /// Make main serve `head` with these files (plus claude-sync.json).
    async fn serve(&self, head: &str, files: &[(&str, &str)]) {
        self.server.reset().await;
        self.mock("GET", "/user", 200, json!({"login": "ann", "id": 1}))
            .await;
        self.mock(
            "GET",
            "/repos/zed/team",
            200,
            json!({"name": "team", "full_name": "zed/team", "private": true}),
        )
        .await;
        let r = "/repos/zed/team";
        self.mock(
            "GET",
            &format!("{r}/commits/main"),
            200,
            json!({"sha": head}),
        )
        .await;
        let who = json!({"name": "Zed", "date": "2026-01-02T03:04:05Z"});
        self.mock("GET", &format!("{r}/commits"), 200, json!([{"sha": head, "commit": {"message": "edit\n\nmore", "author": who, "committer": who}, "author": {"login": "zed"}}])).await;
        let gf = serde_json::to_string_pretty(
            &json!({"schema": 1, "name": "Team", "members": ["ann"], "autoAcceptClaudeMd": false}),
        )
        .unwrap();
        let mut tree = vec![
            json!({"path": "claude-sync.json", "mode": "100644", "type": "blob", "sha": "gf"}),
        ];
        self.mock(
            "GET",
            &format!("{r}/git/blobs/gf"),
            200,
            json!({"content": b64(&gf), "encoding": "base64"}),
        )
        .await;
        for (i, (p, c)) in files.iter().enumerate() {
            let sha = format!("f{i}{head}");
            tree.push(json!({"path": p, "mode": "100644", "type": "blob", "sha": sha}));
            self.mock(
                "GET",
                &format!("{r}/git/blobs/{sha}"),
                200,
                json!({"content": b64(c), "encoding": "base64"}),
            )
            .await;
        }
        self.mock(
            "GET",
            &format!("{r}/git/trees/{head}"),
            200,
            json!({"sha": format!("t{head}"), "tree": tree}),
        )
        .await;
    }
    fn file(&self, rel: &str) -> std::path::PathBuf {
        self.dir.path().join(".claude").join(rel)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_now_delivers_files_and_a_tick_does_too() {
    let t = T::new().await;
    let h = t
        .group_with("h1", &[("skills/demo/SKILL.md", "---\nname: demo\n---\n")])
        .await;
    let e = h.send(Command::SyncNow).await.unwrap();
    assert!(matches!(e, Event::Synced { failed: 0, .. }), "{e:?}");
    assert!(t.file("skills/demo/SKILL.md").exists());
    assert!(!h.snapshot().syncing);
    assert_eq!(h.snapshot().groups[0].status, GroupStatus::Synced);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tick_syncs_when_due_and_pause_stops_it() {
    let t = T::new().await;
    let h = t.signed_in_tick(Duration::from_millis(150)).await;
    h.send(Command::Pause).await.unwrap();
    t.mock(
        "GET",
        "/repos/zed/team",
        200,
        json!({"name": "team", "full_name": "zed/team", "private": true}),
    )
    .await;
    t.push_chain("zed/team").await;
    h.send(Command::Join {
        repo: "zed/team".into(),
    })
    .await
    .unwrap();
    t.serve("h1", &[("skills/a/SKILL.md", "---\nname: a\n---\n")])
        .await;
    t.clock.fetch_add(3600, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!t.file("skills/a/SKILL.md").exists(), "paused: no sync");
    h.send(Command::Resume).await.unwrap();
    t.clock.fetch_add(3600, Ordering::SeqCst);
    for _ in 0..40 {
        if t.file("skills/a/SKILL.md").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(t.file("skills/a/SKILL.md").exists());
}

fn md(lines: &[&str]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_md_changes_are_held_for_review_then_accepted() {
    let t = T::new().await;
    let h = t
        .group_with("h1", &[("CLAUDE.md", &md(&["- Be kind"]))])
        .await;
    h.send(Command::SyncNow).await.unwrap(); // first sight: accepted as is
    let copy = t.dir.path().join(".claude-sync/groups");
    let read_copy = || {
        let d = std::fs::read_dir(&copy)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::read_to_string(d.join("CLAUDE.md")).unwrap()
    };
    assert_eq!(read_copy(), md(&["- Be kind"]));
    t.serve("h2", &[("CLAUDE.md", &md(&["- Be kind", "- Use tabs"]))])
        .await;
    h.send(Command::SyncNow).await.unwrap();
    assert_eq!(read_copy(), md(&["- Be kind"]), "held back");
    assert_eq!(h.snapshot().groups[0].pending_review, 1);
    assert!(h.snapshot().notifications.iter().any(|n| n
        .text
        .contains("zed changed zed/team's CLAUDE.md: 1 addition to review")));
    let Event::Review(l) = h
        .send(Command::ReviewList {
            repo: "zed/team".into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(l[0].add && l[0].author == "zed" && l[0].lines == vec!["- Use tabs".to_string()]);
    h.send(Command::ReviewDecide {
        repo: "zed/team".into(),
        ids: None,
        accept: true,
    })
    .await
    .unwrap();
    assert_eq!(read_copy(), md(&["- Be kind", "- Use tabs"]));
    assert_eq!(h.snapshot().groups[0].pending_review, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn clashes_between_own_text_and_a_group_are_listed() {
    let t = T::new().await;
    std::fs::create_dir_all(t.dir.path().join(".claude")).unwrap();
    std::fs::write(t.file("CLAUDE.md"), md(&["- Always use tabs"])).unwrap();
    let h = t
        .group_with("h1", &[("CLAUDE.md", &md(&["- Never use tabs"]))])
        .await;
    h.send(Command::SyncNow).await.unwrap();
    let Event::Clashes(c) = h.send(Command::Clashes).await.unwrap() else {
        panic!()
    };
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].a.name, "Your CLAUDE.md");
    assert_eq!(c[0].b.source.as_deref(), Some("zed/team"));
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_marker_bytes_and_leave_removes_delivered_files() {
    let t = T::new().await;
    let h = t
        .group_with(
            "h1",
            &[
                ("plugins/mp/a.txt", "x"),
                ("skills/s/SKILL.md", "---\nname: s\n---\n"),
            ],
        )
        .await;
    h.send(Command::SyncNow).await.unwrap();
    let marker = t.dir.path().join(".claude-sync/marketplaces/mp/COMMIT");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "h1\n2026-01-02T03:04:05Z\n"
    );
    assert!(t.file("skills/s/SKILL.md").exists());
    h.send(Command::Leave {
        repo: "zed/team".into(),
    })
    .await
    .unwrap();
    assert!(!t.file("skills/s/SKILL.md").exists());
    assert!(!marker.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_mine_undo_and_a_secret_blocks_sharing() {
    let t = T::new().await;
    let orig = "---\nname: s\n---\nhello\n";
    let h = t.group_with("h1", &[("skills/s/SKILL.md", orig)]).await;
    h.send(Command::SyncNow).await.unwrap();
    let repo = "zed/team".to_string();
    let edits = || async {
        let Event::Edits(e) = h
            .send(Command::EditsList {
                repo: "zed/team".into(),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        e
    };
    assert!(edits().await.is_empty());
    let f = t.file("skills/s/SKILL.md");
    std::fs::write(&f, "mine\n").unwrap();
    // a later pass keeps my edit (nothing is pushed by a pass)
    h.send(Command::SyncNow).await.unwrap();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "mine\n");
    assert_eq!(
        edits().await,
        vec![EditView {
            path: "skills/s/SKILL.md".into(),
            kept: false
        }]
    );
    h.send(Command::KeepMine {
        repo: repo.clone(),
        path: "skills/s/SKILL.md".into(),
    })
    .await
    .unwrap();
    assert!(edits().await[0].kept);
    // a secret in the edit stops the share before anything is sent
    let token = format!("ghp_{}", "a".repeat(36));
    std::fs::write(&f, format!("key {token}\n")).unwrap();
    let e = h
        .send(Command::ShareEdit {
            repo: repo.clone(),
            path: "skills/s/SKILL.md".into(),
        })
        .await
        .unwrap_err();
    assert!(
        e.0.starts_with("secret found in skills/s/SKILL.md:1 (")
            && e.0.ends_with("); nothing was committed"),
        "{}",
        e.0
    );
    assert!(!edits().await[0].kept, "changed again, so not kept");
    // undo brings the group's version back and saves mine
    h.send(Command::UndoEdit {
        repo,
        path: "skills/s/SKILL.md".into(),
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), orig);
    assert!(edits().await.is_empty());
    let backups = t.dir.path().join(".claude-sync/backups/zed__team");
    let stamp = std::fs::read_dir(backups)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(std::fs::read_to_string(stamp.join("skills/s/SKILL.md"))
        .unwrap()
        .contains("key ghp_"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_changed_on_both_sides_is_listed_as_a_conflict() {
    let t = T::new().await;
    let h = t.group_with("h1", &[("skills/s/SKILL.md", "a\n")]).await;
    h.send(Command::SyncNow).await.unwrap();
    std::fs::write(t.file("skills/s/SKILL.md"), "mine\n").unwrap();
    t.serve("h2", &[("skills/s/SKILL.md", "theirs\n")]).await;
    h.send(Command::SyncNow).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(t.file("skills/s/SKILL.md")).unwrap(),
        "theirs\n"
    );
    let Event::Conflicts(c) = h.send(Command::Conflicts).await.unwrap() else {
        panic!()
    };
    assert_eq!(
        (c[0].repo.as_str(), c[0].path.as_str()),
        ("zed/team", "skills/s/SKILL.md")
    );
}

impl T {
    /// What each push to `zed/team` sent: blob contents in order.
    async fn pushed_blobs(&self) -> Vec<String> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/repos/zed/team/git/blobs")
            .map(|r| {
                let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                let b = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    v["content"].as_str().unwrap(),
                )
                .unwrap();
                String::from_utf8(b).unwrap()
            })
            .collect()
    }
    async fn tree_paths(&self) -> Vec<String> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/repos/zed/team/git/trees")
            .flat_map(|r| {
                let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                v["tree"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["path"].as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sharing_an_edit_a_skill_and_claude_md_pushes_the_expected_bytes() {
    let t = T::new().await;
    let h = t
        .group_with(
            "h1",
            &[
                ("skills/s/SKILL.md", "---\nname: s\n---\nhello\n"),
                ("CLAUDE.md", "# Team\n"),
            ],
        )
        .await;
    h.send(Command::SyncNow).await.unwrap();
    t.push_chain("zed/team").await;
    let repo = "zed/team".to_string();

    // an edited delivered file goes up as it is on disk
    std::fs::write(t.file("skills/s/SKILL.md"), "mine\n").unwrap();
    h.send(Command::ShareEdit {
        repo: repo.clone(),
        path: "skills/s/SKILL.md".into(),
    })
    .await
    .unwrap();
    assert_eq!(t.pushed_blobs().await, ["mine\n"]);
    assert_eq!(t.tree_paths().await, ["skills/s/SKILL.md"]);
    let Event::Edits(e) = h
        .send(Command::EditsList { repo: repo.clone() })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(e.is_empty(), "a shared edit is not an edit any more");

    // a new skill goes up with every file of its folder
    let dir = t.file("skills/new");
    std::fs::create_dir_all(dir.join("ref")).unwrap();
    std::fs::write(dir.join("SKILL.md"), "---\nname: new\n---\n").unwrap();
    std::fs::write(dir.join("ref/a.md"), "A\n").unwrap();
    h.send(Command::ShareSkill {
        repo: repo.clone(),
        name: "new".into(),
    })
    .await
    .unwrap();
    let mut tp = t.tree_paths().await;
    tp.sort();
    assert_eq!(
        tp,
        [
            "skills/new/SKILL.md",
            "skills/new/ref/a.md",
            "skills/s/SKILL.md"
        ]
    );
    let blobs = t.pushed_blobs().await;
    assert!(blobs.contains(&"---\nname: new\n---\n".to_string()) && blobs.contains(&"A\n".into()));

    // my CLAUDE.md is added under my login after the group's text
    std::fs::write(t.file("CLAUDE.md"), "my rule\n").unwrap();
    h.send(Command::ShareClaudeMd { repo }).await.unwrap();
    assert_eq!(
        t.pushed_blobs().await.last().unwrap(),
        "# Team\n\n## ann\n\nmy rule\n"
    );
}

// ---- jobs (Parity 41, 42, 64, 73, 74, 75) ----

#[cfg(unix)]
mod jobs {
    use super::*;
    use claude_sync_actions::jobs::{remove_jobs, JobsEnv};
    use claude_sync_fs::tasks::{Kind, Out, Runner};
    use std::sync::atomic::AtomicBool;

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
    }
    impl Runner for Fake {
        fn run(&self, p: &str, a: &[String], _: Option<&[u8]>) -> std::io::Result<Out> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{p} {}", a.join(" ")));
            Ok(Out {
                ok: true,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    struct Rig {
        t: T,
        fake: Arc<Fake>,
        online: Arc<AtomicBool>,
        spawned: Arc<Mutex<Vec<Vec<String>>>>,
        env: JobsEnv,
        agents: std::path::PathBuf,
        args_file: std::path::PathBuf,
    }

    fn job(prompt: &str) -> String {
        json!({"jobs": [{"id": "nightly", "title": "Night", "cron": "17 10 * * 1,4",
            "prompt": prompt, "cwd": "~", "permissionMode": "plan", "timeoutMinutes": 5,
            "args": ["--chrome"]}]})
        .to_string()
    }

    async fn rig() -> (Rig, Handle) {
        use std::os::unix::fs::PermissionsExt;
        let t = T::new().await;
        let bin = t.dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let args_file = t.dir.path().join("args.txt");
        let claude = bin.join("claude");
        std::fs::write(
            &claude,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho ran\n",
                args_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let agents = t.dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let fake = Arc::new(Fake::default());
        let online = Arc::new(AtomicBool::new(true));
        let spawned: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
        let (o, s) = (online.clone(), spawned.clone());
        let env = JobsEnv {
            exe: "/app/claude-sync".into(),
            kind: Kind::Launchd {
                agents_dir: agents.clone(),
                uid: "501".into(),
            },
            runner: fake.clone(),
            claude: Arc::new(move || Some(claude.clone())),
            login_path: Arc::new(|| Some("/login/bin".into())),
            online: Arc::new(move || o.load(Ordering::SeqCst)),
            spawn: Arc::new(move |a| {
                s.lock().unwrap().push(a.to_vec());
                Ok(())
            }),
        };
        *t.jobs.lock().unwrap() = Some(env.clone());
        let h = t
            .group_with("h1", &[("schedules.json", &job("hello world"))])
            .await;
        h.send(Command::SyncNow).await.unwrap();
        (
            Rig {
                t,
                fake,
                online,
                spawned,
                env,
                agents,
                args_file,
            },
            h,
        )
    }

    impl Rig {
        fn plist(&self) -> std::path::PathBuf {
            self.agents.join("dev.claude-sync.job.nightly.plist")
        }
        fn logs(&self) -> std::path::PathBuf {
            self.t.dir.path().join(".claude-sync/logs/jobs/nightly")
        }
        /// Run the job the way the hidden `run-job` subcommand does: a fresh core on the same state.
        async fn run(&self, manual: bool) -> Result<String> {
            let (env, jobs) = (self.t.env("cid"), self.env.clone());
            tokio::task::spawn_blocking(move || {
                let mut c = Core::open(env)?;
                c.set_jobs_env(jobs);
                c.run_job("nightly", manual)
            })
            .await
            .unwrap()
        }
    }

    fn acc(id: &str) -> Command {
        Command::JobAccept {
            repo: "zed/team".into(),
            id: id.into(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_job_is_registered_only_after_it_is_accepted_and_never_twice() {
        let (r, h) = rig().await;
        let j = &h.snapshot().jobs[0];
        assert_eq!(
            (j.id.as_str(), j.accepted, j.enabled),
            ("nightly", false, true)
        );
        assert!(!r.plist().exists(), "not accepted: nothing registered");
        assert!(r.fake.calls.lock().unwrap().is_empty());

        h.send(acc("nightly")).await.unwrap();
        let plist = std::fs::read_to_string(r.plist()).unwrap();
        assert!(
            plist.contains("/app/claude-sync")
                && plist.contains("run-job")
                && plist.contains("nightly")
        );
        assert!(h.snapshot().jobs[0].accepted);

        // the same job on the next pass and the same accept: the task is left alone
        let n = r.fake.calls.lock().unwrap().len();
        h.send(Command::SyncNow).await.unwrap();
        h.send(acc("nightly")).await.unwrap();
        assert_eq!(r.fake.calls.lock().unwrap().len(), n);

        // a moved exe re-registers once, and the old registration is replaced, not added to
        let mut moved = r.env.clone();
        moved.exe = "/elsewhere/claude-sync".into();
        let h2 = {
            let h = spawn_with(r.t.env("cid"), Duration::from_secs(3600), Some(moved))
                .await
                .unwrap();
            h.send(Command::SyncNow).await.unwrap();
            h
        };
        assert!(std::fs::read_to_string(r.plist())
            .unwrap()
            .contains("/elsewhere/claude-sync"));
        let n = r.fake.calls.lock().unwrap().len();
        h2.send(Command::SyncNow).await.unwrap();
        assert_eq!(r.fake.calls.lock().unwrap().len(), n);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn off_and_a_changed_job_unregister_and_auto_accept_takes_the_change() {
        let (r, h) = rig().await;
        h.send(acc("nightly")).await.unwrap();
        h.send(Command::JobEnable {
            repo: "zed/team".into(),
            id: "nightly".into(),
            on: false,
        })
        .await
        .unwrap();
        assert!(!r.plist().exists());
        assert!(!h.snapshot().jobs[0].enabled && h.snapshot().jobs[0].accepted);
        assert!(r
            .run(false)
            .await
            .unwrap_err()
            .to_string()
            .contains("turned off"));
        assert_eq!(
            r.run(true).await.unwrap(),
            "ok",
            "Run now runs a job that is off"
        );
        h.send(Command::JobEnable {
            repo: "zed/team".into(),
            id: "nightly".into(),
            on: true,
        })
        .await
        .unwrap();
        assert!(r.plist().exists());

        // the group changes the prompt: the acceptance no longer holds
        r.t.serve("h2", &[("schedules.json", &job("do more"))])
            .await;
        r.t.push_chain("zed/team").await;
        h.send(Command::SyncNow).await.unwrap();
        assert!(!h.snapshot().jobs[0].accepted);
        assert!(!r.plist().exists());
        assert!(r
            .run(false)
            .await
            .unwrap_err()
            .to_string()
            .contains("not accepted"));

        // with auto-accept the next change is taken and registered
        h.send(Command::JobsAutoAccept {
            repo: "zed/team".into(),
            on: true,
        })
        .await
        .unwrap();
        assert!(h.snapshot().jobs[0].accepted && r.plist().exists());
        r.t.serve("h3", &[("schedules.json", &job("third"))]).await;
        r.t.push_chain("zed/team").await;
        h.send(Command::SyncNow).await.unwrap();
        assert!(h.snapshot().jobs[0].accepted && r.plist().exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_run_uses_the_schedule_args_logs_and_locks() {
        let (r, h) = rig().await;
        h.send(acc("nightly")).await.unwrap();

        // offline: recorded, claude not run, no log
        r.online.store(false, Ordering::SeqCst);
        assert_eq!(r.run(false).await.unwrap(), "skipped: offline");
        assert!(!r.args_file.exists() && !r.logs().exists());
        r.online.store(true, Ordering::SeqCst);

        // the 20 newest logs stay
        std::fs::create_dir_all(r.logs()).unwrap();
        for i in 0..25 {
            std::fs::write(r.logs().join(format!("19700101T0000{i:02}Z.log")), "old").unwrap();
        }
        assert_eq!(r.run(false).await.unwrap(), "ok");
        assert_eq!(
            std::fs::read_to_string(&r.args_file).unwrap(),
            "-p\nhello world\n--chrome\n--permission-mode\nplan\n"
        );
        let mut logs: Vec<_> = std::fs::read_dir(r.logs())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        logs.sort();
        assert_eq!(logs.len(), 20);
        let newest = std::fs::read_to_string(r.logs().join(logs.last().unwrap())).unwrap();
        assert!(
            newest.contains("PATH: /login/bin") && newest.contains("ran"),
            "{newest}"
        );
        assert_eq!(h.snapshot().jobs.len(), 1);
        let Event::Path(p) = h
            .send(Command::JobFile {
                repo: "zed/team".into(),
                id: "nightly".into(),
                report: false,
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert!(p.unwrap().ends_with(logs.last().unwrap()));
        assert_eq!(h.snapshot().jobs[0].last_result, "ok");

        // a fresh lock means a run is going; a stale one is broken
        let lock = r.t.dir.path().join(".claude-sync/jobs/nightly/lock");
        let now = r.t.clock.load(Ordering::SeqCst);
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(&lock, format!("1 {}", now - 60)).unwrap();
        assert_eq!(r.run(false).await.unwrap(), "already running");
        std::fs::write(&lock, format!("1 {}", now - 5 * 60 - 300 - 1)).unwrap();
        assert_eq!(r.run(false).await.unwrap(), "ok");
        assert!(!lock.exists(), "the lock is released");

        // Run now starts its own process
        h.send(Command::JobRunNow {
            repo: "zed/team".into(),
            id: "nightly".into(),
        })
        .await
        .unwrap();
        assert_eq!(
            r.spawned.lock().unwrap()[0],
            ["run-job", "nightly", "--now"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_jobs_removes_the_tasks_keeps_the_acceptances_and_fails_loudly() {
        let (r, h) = rig().await;
        h.send(acc("nightly")).await.unwrap();
        assert!(r.plist().exists());
        drop(h);
        let home = r.t.dir.path().to_path_buf();
        // an OS task that cannot be removed is an error, so remove-jobs exits 1
        std::fs::remove_file(r.plist()).unwrap();
        std::fs::create_dir_all(r.plist().join("stuck")).unwrap();
        assert!(remove_jobs(&home, &r.env).is_err());
        std::fs::remove_dir(r.plist().join("stuck")).unwrap();
        std::fs::remove_dir(r.plist()).unwrap();
        std::fs::write(r.plist(), "x").unwrap();
        assert_eq!(remove_jobs(&home, &r.env).unwrap(), 1);
        assert!(!r.plist().exists());
        // accepted stays: back on the next pass without asking again
        let h = spawn_with(
            r.t.env("cid"),
            Duration::from_secs(3600),
            Some(r.env.clone()),
        )
        .await
        .unwrap();
        assert!(h.snapshot().jobs[0].accepted);
        h.send(Command::SyncNow).await.unwrap();
        assert!(r.plist().exists());
    }

    #[test]
    fn without_a_store_the_v1_jobs_file_names_the_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join(".claude-sync");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("jobs.json"),
            r#"{"jobs":{"nightly":{"group":"zed/team"}}}"#,
        )
        .unwrap();
        let fake = Arc::new(Fake::default());
        let env = JobsEnv {
            exe: "/app".into(),
            kind: Kind::Launchd {
                agents_dir: dir.path().join("a"),
                uid: "501".into(),
            },
            runner: fake.clone(),
            claude: Arc::new(|| None),
            login_path: Arc::new(|| None),
            online: Arc::new(|| true),
            spawn: Arc::new(|_| Ok(())),
        };
        assert_eq!(remove_jobs(dir.path(), &env).unwrap(), 1);
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("nightly")));
        assert!(
            !state.join("claude-sync.db").exists(),
            "no store is created"
        );
    }
}

mod personal {
    use super::*;
    use claude_sync_store::blob_sha;
    use std::collections::{BTreeMap, HashMap};
    use wiremock::{Request, Respond};

    const REPO: &str = "/repos/ann/claude-sync-personal";

    /// A stateful fake of the one personal repo: Git Data calls over a map of snapshots, so a
    /// push by one device is what the next device reads. A parent that is not the head is a
    /// race (422), as on GitHub.
    #[derive(Default)]
    pub struct Git {
        pub exists: bool,
        pub private: bool,
        head: String,
        blobs: HashMap<String, Vec<u8>>,
        snaps: HashMap<String, BTreeMap<String, String>>,
        n: u32,
    }

    pub type Shared = Arc<Mutex<Git>>;
    type Handler = Box<dyn Fn(&mut Git, &Request) -> ResponseTemplate + Send + Sync>;

    impl Git {
        pub fn files(&self) -> BTreeMap<String, String> {
            let snap = self.snaps.get(&self.head).cloned().unwrap_or_default();
            snap.into_iter()
                .map(|(p, s)| (p, String::from_utf8_lossy(&self.blobs[&s]).into_owned()))
                .collect()
        }
        pub fn put(&mut self, path: &str, text: &str) {
            let mut f = self.snaps.get(&self.head).cloned().unwrap_or_default();
            let sha = blob_sha(text.as_bytes());
            self.blobs.insert(sha.clone(), text.as_bytes().to_vec());
            f.insert(path.into(), sha);
            self.n += 1;
            self.head = format!("c{}", self.n);
            self.snaps.insert(self.head.clone(), f);
        }
    }

    fn repo_json(g: &Git) -> serde_json::Value {
        json!({"name": "claude-sync-personal", "full_name": "ann/claude-sync-personal", "private": g.private})
    }

    struct Fn1<F>(F);
    impl<F: Fn(&Request) -> ResponseTemplate + Send + Sync> Respond for Fn1<F> {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            (self.0)(r)
        }
    }

    pub async fn mount(t: &T, git: &Shared) {
        let on = |m: &'static str, p: String, f: Handler| {
            let g = git.clone();
            Mock::given(method(m))
                .and(path_regex(p))
                .respond_with(Fn1(move |r: &Request| f(&mut g.lock().unwrap(), r)))
        };
        let nf = || ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"}));
        let mocks = vec![
            on(
                "GET",
                format!("^{REPO}$"),
                Box::new(move |g, _| {
                    if g.exists {
                        ResponseTemplate::new(200).set_body_json(repo_json(g))
                    } else {
                        nf()
                    }
                }),
            ),
            on(
                "POST",
                "^/user/repos$".into(),
                Box::new(|g, _| {
                    g.exists = true;
                    g.private = true;
                    g.put("README.md", "# claude-sync personal memory\n");
                    ResponseTemplate::new(201).set_body_json(repo_json(g))
                }),
            ),
            on(
                "GET",
                format!("^{REPO}/commits/main$"),
                Box::new(move |g, _| {
                    if g.head.is_empty() {
                        ResponseTemplate::new(409)
                            .set_body_json(json!({"message": "Git Repository is empty."}))
                    } else {
                        ResponseTemplate::new(200).set_body_json(json!({"sha": g.head}))
                    }
                }),
            ),
            on(
                "GET",
                format!("^{REPO}/git/trees/.+$"),
                Box::new(move |g, r| {
                    let id = r
                        .url
                        .path()
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    let key = id
                        .strip_prefix("tn")
                        .map(|x| format!("n{x}"))
                        .unwrap_or(id.clone());
                    let Some(snap) = g.snaps.get(&key) else {
                        return nf();
                    };
                    let tree: Vec<_> = snap
                        .iter()
                        .map(
                            |(p, s)| json!({"path": p, "mode": "100644", "type": "blob", "sha": s}),
                        )
                        .collect();
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"sha": format!("t{key}"), "tree": tree}))
                }),
            ),
            on(
                "GET",
                format!("^{REPO}/git/blobs/.+$"),
                Box::new(move |g, r| {
                    let id = r.url.path().rsplit('/').next().unwrap_or_default();
                    match g.blobs.get(id) {
                    Some(b) => ResponseTemplate::new(200).set_body_json(json!({"content": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b), "encoding": "base64"})),
                    None => nf(),
                }
                }),
            ),
            on(
                "POST",
                format!("^{REPO}/git/blobs$"),
                Box::new(move |g, r| {
                    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                    let b = base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        v["content"].as_str().unwrap(),
                    )
                    .unwrap();
                    let sha = blob_sha(&b);
                    g.blobs.insert(sha.clone(), b);
                    ResponseTemplate::new(201).set_body_json(json!({"sha": sha}))
                }),
            ),
            on(
                "POST",
                format!("^{REPO}/git/trees$"),
                Box::new(move |g, r| {
                    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                    let base = v["base_tree"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches('t')
                        .to_string();
                    let mut snap = g.snaps.get(&base).cloned().unwrap_or_default();
                    for e in v["tree"].as_array().unwrap() {
                        let p = e["path"].as_str().unwrap().to_string();
                        match e["sha"].as_str() {
                            Some(s) => {
                                snap.insert(p, s.into());
                            }
                            None => {
                                snap.remove(&p);
                            }
                        }
                    }
                    g.n += 1;
                    g.snaps.insert(format!("n{}", g.n), snap);
                    ResponseTemplate::new(201).set_body_json(json!({"sha": format!("tn{}", g.n)}))
                }),
            ),
            on(
                "POST",
                format!("^{REPO}/git/commits$"),
                Box::new(move |g, r| {
                    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                    let tree = v["tree"]
                        .as_str()
                        .unwrap()
                        .strip_prefix("tn")
                        .unwrap()
                        .to_string();
                    if v["parents"][0].as_str() != Some(g.head.as_str()) {
                        return ResponseTemplate::new(422)
                            .set_body_json(json!({"message": "Update is not a fast forward"}));
                    }
                    let snap = g.snaps[&format!("n{tree}")].clone();
                    g.n += 1;
                    let id = format!("c{}", g.n);
                    g.snaps.insert(id.clone(), snap);
                    ResponseTemplate::new(201).set_body_json(json!({"sha": id}))
                }),
            ),
            on(
                "PATCH",
                format!("^{REPO}/git/refs/heads/main$"),
                Box::new(move |g, r| {
                    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                    g.head = v["sha"].as_str().unwrap().to_string();
                    ResponseTemplate::new(200).set_body_json(json!({}))
                }),
            ),
        ];
        t.mock("GET", "/user", 200, json!({"login": "ann", "id": 1}))
            .await;
        for m in mocks {
            m.mount(&t.server).await;
        }
    }

    pub fn new_git() -> Shared {
        Arc::default()
    }

    struct Dev {
        t: T,
        h: Handle,
    }

    async fn device(name: &'static str, git: &Shared) -> Dev {
        let t = T::named(name).await;
        mount(&t, git).await;
        let h = t.signed_in().await;
        Dev { t, h }
    }

    impl Dev {
        fn claude(&self, rel: &str) -> std::path::PathBuf {
            self.t.dir.path().join(".claude").join(rel)
        }
        fn write(&self, rel: &str, text: &str) {
            let p = self.claude(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        fn memory(&self) -> String {
            let enc = claude_sync_fs::disk::encode_folder(&self.t.dir.path().to_string_lossy());
            format!("projects/{enc}/memory/MEMORY.md")
        }
        async fn sync(&self) {
            self.t.clock.fetch_add(1, Ordering::SeqCst);
            self.h.send(Command::SyncNow).await.unwrap();
        }
        async fn personal(&self) -> PersonalView {
            self.h.snapshot().personal
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn turning_on_makes_the_private_repo_and_shares_the_files() {
        let git = new_git();
        let a = device("laptop", &git).await;
        a.write("CLAUDE.md", "# Mine\nalways test\n");
        a.write(&a.memory(), "- likes tabs\n");
        a.h.send(Command::PersonalOn).await.unwrap();
        let p = a.personal().await;
        assert!(p.on && p.error.is_none(), "{p:?}");
        assert_eq!(p.devices.len(), 1);
        assert!(p.devices[0].this && p.devices[0].name == "laptop");
        let files = git.lock().unwrap().files();
        assert_eq!(
            files["claude-sync-personal.json"].replace(char::is_whitespace, ""),
            r#"{"format":1,"newDevicesJoin":true}"#
        );
        assert_eq!(files["CLAUDE.md"], "# Mine\nalways test\n");
        let key = files.keys().find(|k| k.starts_with("memory/home")).unwrap();
        assert_eq!(files[key], "- likes tabs\n");
        assert!(files["devices.json"].contains("laptop"));
        assert!(files.contains_key("state/claude-sync/choices.json"));
        assert!(git.lock().unwrap().private);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn two_devices_round_trip_and_a_rename_is_pushed_on_the_next_pass() {
        let git = new_git();
        let a = device("laptop", &git).await;
        a.write(&a.memory(), "- one\n");
        a.h.send(Command::PersonalOn).await.unwrap();
        // a second device finds the repo, joins by itself and receives the memory
        let b = device("desktop", &git).await;
        b.sync().await;
        let pb = b.personal().await;
        assert!(pb.on, "{pb:?}");
        assert_eq!(
            std::fs::read_to_string(b.claude(&b.memory())).unwrap(),
            "- one\n"
        );
        // both add a line; the line merge keeps both, with LF bytes
        a.write(&a.memory(), "- one\n- from laptop\n");
        b.write(&b.memory(), "- one\n- from desktop\r\n");
        a.sync().await;
        b.sync().await;
        a.sync().await;
        let merged = std::fs::read_to_string(a.claude(&a.memory())).unwrap();
        assert!(
            merged.contains("from laptop") && merged.contains("from desktop"),
            "{merged}"
        );
        let files = git.lock().unwrap().files();
        let key = files.keys().find(|k| k.starts_with("memory/home")).unwrap();
        assert!(!files[key].contains('\r'));
        // the laptop renames the desktop; it is pushed on the next pass and the desktop sees it
        let id = a
            .personal()
            .await
            .devices
            .into_iter()
            .find(|d| d.name == "desktop")
            .unwrap()
            .id;
        a.h.send(Command::DeviceRename {
            id: id.clone(),
            name: "Office PC".into(),
        })
        .await
        .unwrap();
        a.sync().await;
        assert!(git.lock().unwrap().files()["devices.json"].contains("Office PC"));
        // a device cannot remove itself
        let me = a
            .personal()
            .await
            .devices
            .into_iter()
            .find(|d| d.this)
            .unwrap()
            .id;
        let e =
            a.h.send(Command::DeviceRemove { id: me })
                .await
                .unwrap_err();
        assert!(e.0.contains("can't remove itself"));
        a.h.send(Command::DeviceRemove { id }).await.unwrap();
        a.sync().await;
        assert!(!git.lock().unwrap().files()["devices.json"].contains("Office PC"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_repo_that_turns_public_stops_the_sync() {
        let git = new_git();
        let a = device("laptop", &git).await;
        a.write("CLAUDE.md", "# Mine\n");
        a.h.send(Command::PersonalOn).await.unwrap();
        let before = git.lock().unwrap().files();
        git.lock().unwrap().private = false;
        a.write("CLAUDE.md", "# Mine\nsecret plan\n");
        a.sync().await;
        let p = a.personal().await;
        assert!(p.error.as_deref().unwrap().contains("public"), "{p:?}");
        assert_eq!(git.lock().unwrap().files(), before, "nothing was pushed");
        // private again: it syncs again
        git.lock().unwrap().private = true;
        a.sync().await;
        assert_eq!(
            git.lock().unwrap().files()["CLAUDE.md"],
            "# Mine\nsecret plan\n"
        );
        assert!(a.personal().await.error.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn choices_from_another_device_are_applied_and_off_reaches_it() {
        let git = new_git();
        let a = device("laptop", &git).await;
        a.h.send(Command::PersonalOn).await.unwrap();
        let b = device("desktop", &git).await;
        b.sync().await;
        assert!(b.personal().await.on);
        // a choice made on the laptop (a kind off, a group paused, the interval) follows
        a.t.clock.fetch_add(100, Ordering::SeqCst);
        a.h.send(Command::PersonalKind {
            kind: "memory".into(),
            on: false,
        })
        .await
        .unwrap();
        a.h.send(Command::SetInterval { secs: 300 }).await.unwrap();
        a.sync().await;
        b.sync().await;
        let (pa, sb) = (a.personal().await, b.h.snapshot());
        assert_eq!(pa.kinds_off, vec!["memory"]);
        assert_eq!(sb.personal.kinds_off, vec!["memory"]);
        assert_eq!(sb.interval_secs, 300);
        // a kind that is off neither pushes nor pulls
        a.write(&a.memory(), "- private to the laptop\n");
        a.sync().await;
        assert!(!git
            .lock()
            .unwrap()
            .files()
            .values()
            .any(|v| v.contains("private to the laptop")));
        // turning it off on the laptop turns it off on the desktop at its next pass, and new
        // devices stop joining
        a.h.send(Command::PersonalOff).await.unwrap();
        assert!(!a.personal().await.on);
        assert!(git.lock().unwrap().files()["claude-sync-personal.json"].contains("false"));
        b.sync().await;
        assert!(!b.personal().await.on);
        // a third device is offered it, not joined
        let c = device("tablet", &git).await;
        c.sync().await;
        let pc = c.personal().await;
        assert!(!pc.on && pc.offer, "{pc:?}");
        c.h.send(Command::PersonalDismiss).await.unwrap();
        assert!(!c.personal().await.offer);
    }
}
