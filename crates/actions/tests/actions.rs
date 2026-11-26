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
}

impl T {
    async fn new() -> T {
        T {
            server: MockServer::start().await,
            dir: tempfile::tempdir().unwrap(),
            clock: Arc::new(AtomicI64::new(1_000_000)),
            secrets: Arc::default(),
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
            device: "laptop".into(),
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
        spawn(self.env("cid"), tick).await.unwrap()
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
        let gf =
            serde_json::to_string_pretty(&json!({"schema": 1, "name": "Team", "members": ["ann"]}))
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
