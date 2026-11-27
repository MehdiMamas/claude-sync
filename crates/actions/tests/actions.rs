//! Each command against a fake GitHub (wiremock) and a temp home; the loop with a fake clock.
use base64::Engine as _;
use claude_sync_actions::*;
use claude_sync_github::secrets::SecretStore;
use claude_sync_github::{Config, Result as GhResult};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
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
    no_auto: std::sync::atomic::AtomicBool,
    /// Served commits have no GitHub account behind the author (only a git name).
    anon: std::sync::atomic::AtomicBool,
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
            no_auto: Default::default(),
            anon: Default::default(),
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
                relay: uri.clone(),
                releases: uri.clone(),
                client_id: client_id.into(),
                retry_delay: Duration::from_millis(1),
            },
            app_client_id: String::new(),
            secrets: claude_sync_actions::Secrets(self.secrets.clone()),
            now: Arc::new(move || clock.load(Ordering::SeqCst)),
            device: format!("ann@{}", self.device),
            procs: Arc::new(claude_sync_actions::handover::NoProcs),
            no_auto_personal: self.no_auto.load(Ordering::SeqCst),
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
    t.serve_repo(
        "zed/a",
        "h1",
        &[],
        json!({"schema": 1, "name": "A", "members": ["ann", "bob"]}),
        false,
    )
    .await;
    let found = vec![DiscoverView {
        repo: "zed/a".into(),
        members: Some(2),
    }];
    assert_eq!(
        h.send(Command::Discover).await.unwrap(),
        Event::Discovered(found.clone())
    );
    assert_eq!(h.snapshot().discoverable, found);
}

#[tokio::test(flavor = "multi_thread")]
async fn invitations_and_pause_and_a_401_stops_the_loop() {
    let t = T::new().await;
    let h = t.signed_in().await;
    t.mock("GET", "/user/repository_invitations", 200, json!([{"id": 7, "repository": {"name": "t", "full_name": "zed/t", "private": true}, "inviter": {"login": "zed"}, "created_at": "2024-01-02T03:04:05Z"}])).await;
    h.send(Command::WindowOpened).await.unwrap();
    assert_eq!(
        h.snapshot().invitations,
        vec![InvitationView {
            id: 7,
            repo: "zed/t".into(),
            inviter: "zed".into(),
            created_at: Some(1704164645)
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
        let gf =
            json!({"schema": 1, "name": "Team", "members": ["ann"], "autoAcceptClaudeMd": false});
        self.serve_repo("zed/team", head, files, gf, true).await;
    }
    /// `serve` for any repo and group file; `reset` clears every mock first (and brings
    /// back `/user`), so a second repo is served with `reset: false`.
    async fn serve_repo(
        &self,
        repo: &str,
        head: &str,
        files: &[(&str, &str)],
        gf: serde_json::Value,
        reset: bool,
    ) {
        if reset {
            self.server.reset().await;
            self.mock("GET", "/user", 200, json!({"login": "ann", "id": 1}))
                .await;
        }
        let (o, n) = repo.split_once('/').unwrap();
        self.mock(
            "GET",
            &format!("/repos/{repo}"),
            200,
            json!({"name": n, "full_name": repo, "private": true, "owner": {"login": o}}),
        )
        .await;
        let r = format!("/repos/{repo}");
        self.mock(
            "GET",
            &format!("{r}/commits/main"),
            200,
            json!({"sha": head}),
        )
        .await;
        let who = json!({"name": "Zed", "date": "2026-01-02T03:04:05Z"});
        let account = if self.anon.load(Ordering::SeqCst) {
            json!(null)
        } else {
            json!({"login": "zed"})
        };
        self.mock("GET", &format!("{r}/commits"), 200, json!([{"sha": head, "commit": {"message": "edit\n\nmore", "author": who, "committer": who}, "author": account}])).await;
        let gf = serde_json::to_string_pretty(&gf).unwrap();
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
async fn a_group_whose_head_is_refused_leaves_this_machine_with_one_notice() {
    let t = T::new().await;
    let h = t
        .group_with("h1", &[("skills/demo/SKILL.md", "---\nname: demo\n---\n")])
        .await;
    h.send(Command::SyncNow).await.unwrap();
    assert!(t.file("skills/demo/SKILL.md").exists());
    // v1: a 404 (or 403) on the head read means access is gone
    t.server.reset().await;
    t.mock("GET", "/user", 200, json!({"login": "ann", "id": 1}))
        .await;
    t.mock(
        "GET",
        "/repos/zed/team/commits/main",
        404,
        json!({"message": "Not Found"}),
    )
    .await;
    h.send(Command::SyncNow).await.unwrap();
    let snap = h.snapshot();
    assert!(snap.groups.is_empty());
    assert!(!t.file("skills/demo/SKILL.md").exists());
    let n: Vec<_> = snap
        .notifications
        .iter()
        .filter(|n| n.text.contains("is gone"))
        .map(|n| n.text.clone())
        .collect();
    assert_eq!(
        n,
        ["access to zed/team is gone; its files were removed from this machine"]
    );
    // nothing is left to fail again
    h.send(Command::SyncNow).await.unwrap();
    assert_eq!(h.snapshot().notifications.len(), snap.notifications.len());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pass_names_its_groups_and_a_failure_has_its_own_time() {
    let t = T::new().await;
    let h = t
        .group_with("h1", &[("skills/demo/SKILL.md", "---\nname: demo\n---\n")])
        .await;
    for _ in 0..40 {
        if t.file("skills/demo/SKILL.md").exists() {
            break; // the first pass (the loop's own) delivered it
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    t.serve(
        "h2",
        &[("skills/demo/SKILL.md", "---\nname: demo\n---\nv2\n")],
    )
    .await;
    let mut rx = h.watch();
    let seen = Arc::new(AtomicBool::new(false));
    let flag = seen.clone();
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            if rx.borrow().groups.iter().any(|g| g.syncing) {
                flag.store(true, Ordering::SeqCst);
            }
        }
    });
    let e = h.send(Command::SyncNow).await.unwrap();
    assert_eq!(
        e,
        Event::Synced {
            files: 1,
            failed: 0,
            repos: vec!["zed/team".into()]
        }
    );
    assert!(seen.load(Ordering::SeqCst), "the group showed as syncing");
    let g = h.snapshot().groups.remove(0);
    assert!(!g.syncing && g.failed_at.is_none());
    // the next pass fails: the failure time is set, the last good sync stays
    t.server.reset().await;
    // a 5xx, not a 404: a refused head means access is gone and removes the group
    t.mock("GET", "/repos/zed/team/commits/main", 500, json!({}))
        .await;
    t.clock.fetch_add(100, Ordering::SeqCst);
    let e = h.send(Command::SyncNow).await.unwrap();
    assert!(matches!(e, Event::Synced { failed: 1, .. }), "{e:?}");
    let f = h.snapshot().groups.remove(0);
    assert_eq!(f.last_sync, g.last_sync);
    assert!(f.failed_at.is_some_and(|x| x > g.last_sync.unwrap_or(0)));
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

/// Serve the group with auto-accept on, so what the group changes is applied.
async fn serve_auto(t: &T, head: &str, text: &str) {
    let gf = json!({"schema": 1, "name": "Team", "members": ["ann"], "autoAcceptClaudeMd": true});
    t.serve_repo("zed/team", head, &[("CLAUDE.md", text)], gf, true)
        .await;
}

fn copy_file(t: &T) -> std::path::PathBuf {
    let d = std::fs::read_dir(t.dir.path().join(".claude-sync/groups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    d.join("CLAUDE.md")
}

fn backed_up(t: &T, needle: &str) -> bool {
    fn walk(d: &std::path::Path, needle: &str) -> bool {
        std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, needle)
                } else {
                    std::fs::read_to_string(&p).is_ok_and(|s| s.contains(needle))
                }
            })
    }
    walk(&t.dir.path().join(".claude-sync/backups"), needle)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_edit_of_the_claude_md_copy_survives_a_remote_change_and_is_backed_up() {
    let t = T::new().await;
    let h = t.group_with("h1", &[]).await;
    serve_auto(&t, "h1", &md(&["- A", "- B"])).await;
    h.send(Command::SyncNow).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(copy_file(&t)).unwrap(),
        md(&["- A", "- B"])
    );
    // the person appends a line to the delivered copy; a pass with nothing new keeps it
    let mut mine = std::fs::read_to_string(copy_file(&t)).unwrap();
    mine.push_str("- mine\n");
    std::fs::write(copy_file(&t), &mine).unwrap();
    h.send(Command::SyncNow).await.unwrap();
    assert_eq!(std::fs::read_to_string(copy_file(&t)).unwrap(), mine);
    let Event::Edits(e) = h
        .send(Command::EditsList {
            repo: "zed/team".into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        e,
        vec![EditView {
            path: "CLAUDE.md".into(),
            kept: false
        }]
    );
    // the group changes; the edit stays and the change lands beside it
    serve_auto(&t, "h2", &md(&["- A", "- B", "- C"])).await;
    h.send(Command::SyncNow).await.unwrap();
    let now = std::fs::read_to_string(copy_file(&t)).unwrap();
    assert!(now.contains("- mine") && now.contains("- C"), "{now}");
    assert!(backed_up(&t, "- mine"), "the edit was not backed up");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edit_of_a_line_the_group_also_changed_is_not_lost() {
    let t = T::new().await;
    let h = t.group_with("h1", &[]).await;
    serve_auto(&t, "h1", &md(&["- A", "- B"])).await;
    h.send(Command::SyncNow).await.unwrap();
    std::fs::write(copy_file(&t), md(&["- A", "- B mine"])).unwrap();
    serve_auto(&t, "h2", &md(&["- A", "- B group"])).await;
    h.send(Command::SyncNow).await.unwrap();
    let now = std::fs::read_to_string(copy_file(&t)).unwrap();
    assert!(now.contains("- B mine"), "{now}");
    assert!(backed_up(&t, "- B mine"));
    // undo brings the group's text back and keeps the edit in backups
    h.send(Command::UndoEdit {
        repo: "zed/team".into(),
        path: "CLAUDE.md".into(),
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(copy_file(&t)).unwrap(),
        md(&["- A", "- B group"])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_review_item_by_a_commit_with_no_github_login_names_the_git_author() {
    let t = T::new().await;
    t.anon.store(true, Ordering::SeqCst);
    let h = t
        .group_with("h1", &[("CLAUDE.md", &md(&["- Be kind"]))])
        .await;
    h.send(Command::SyncNow).await.unwrap();
    t.serve("h2", &[("CLAUDE.md", &md(&["- Be kind", "- Use tabs"]))])
        .await;
    h.send(Command::SyncNow).await.unwrap();
    let Event::Review(l) = h
        .send(Command::ReviewList {
            repo: "zed/team".into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(l[0].author, "Zed");
    assert!(h
        .snapshot()
        .notifications
        .iter()
        .any(|n| n.text.contains("Zed changed zed/team's CLAUDE.md")));
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
async fn two_groups_setting_one_value_differently_are_listed_as_a_setting_clash() {
    let t = T::new().await;
    let h = t.signed_in().await;
    let gf = json!({"schema": 1, "name": "Team", "members": ["ann"], "sync": {"settings": true}});
    t.join_served(
        &h,
        "zed/one",
        "h1",
        &[("settings.shared.json", r#"{"model":"opus"}"#)],
        gf.clone(),
        true,
    )
    .await;
    t.join_served(
        &h,
        "zed/two",
        "h2",
        &[("settings.shared.json", r#"{"model":"sonnet"}"#)],
        gf,
        false,
    )
    .await;
    h.send(Command::SyncNow).await.unwrap();
    let Event::Clashes(c) = h.send(Command::Clashes).await.unwrap() else {
        panic!()
    };
    let s: Vec<_> = c.iter().filter(|x| x.kind == "setting").collect();
    assert_eq!(s.len(), 1, "{c:?}");
    assert_eq!(s[0].target.as_deref(), Some("/model"));
    assert_eq!(s[0].a.source.as_deref(), Some("zed/one"));
    assert_eq!(s[0].b.source.as_deref(), Some("zed/two"));
}

#[tokio::test(flavor = "multi_thread")]
async fn edit_line_of_the_own_text_rewrites_it_with_a_backup() {
    let t = T::new().await;
    std::fs::create_dir_all(t.dir.path().join(".claude")).unwrap();
    std::fs::write(
        t.file("CLAUDE.md"),
        md(&["# Me", "- Always use tabs", "- B"]),
    )
    .unwrap();
    let h = t.signed_in().await;
    let cmd = |text: &str| Command::EditLine {
        source: None,
        line: "- Always use tabs".into(),
        text: text.into(),
    };
    h.send(cmd("- Use tabs in Go only")).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(t.file("CLAUDE.md")).unwrap(),
        md(&["# Me", "- Use tabs in Go only", "- B"])
    );
    assert_eq!(
        std::fs::read_dir(t.dir.path().join(".claude-sync/backups"))
            .unwrap()
            .count(),
        1
    );
    // the line is gone now, so a second edit is refused and writes nothing
    let e = h.send(cmd("x")).await.unwrap_err();
    assert!(e
        .to_string()
        .contains("your CLAUDE.md no longer has that line"));
    // empty text removes the line
    h.send(Command::EditLine {
        source: None,
        line: "- B".into(),
        text: String::new(),
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(t.file("CLAUDE.md")).unwrap(),
        md(&["# Me", "- Use tabs in Go only"])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn edit_line_of_a_group_commits_it_redacted_or_refuses() {
    let t = T::new().await;
    let h = t
        .group_with(
            "h1",
            &[("CLAUDE.md", &md(&["# Team", "- Never use tabs", "- B"]))],
        )
        .await;
    h.send(Command::SyncNow).await.unwrap();
    t.push_chain("zed/team").await;
    let edit = |line: &str, text: &str| Command::EditLine {
        source: Some("zed/team".into()),
        line: line.into(),
        text: text.into(),
    };
    h.send(edit("- Never use tabs", "- Use tabs in Go only"))
        .await
        .unwrap();
    assert_eq!(
        t.pushed_blobs().await,
        [md(&["# Team", "- Use tabs in Go only", "- B"])]
    );
    let commit = &t.sent("POST", "/repos/zed/team/git/commits").await[0];
    assert!(commit["message"]
        .as_str()
        .unwrap()
        .ends_with(": edit CLAUDE.md"));
    // a secret in the text never leaves the machine
    let key = format!("ghp_{}", "a".repeat(36));
    h.send(edit("- B", &format!("- key {key}"))).await.unwrap();
    let blobs = t.pushed_blobs().await;
    assert_eq!(blobs.len(), 2);
    assert!(
        !blobs[1].contains(&key) && blobs[1].contains("[redacted]"),
        "{}",
        blobs[1]
    );
    // a line the group no longer has
    let e = h.send(edit("- gone", "x")).await.unwrap_err();
    assert!(e
        .to_string()
        .contains("zed/team's CLAUDE.md no longer has that line"));
    assert_eq!(t.pushed_blobs().await.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn edit_line_is_refused_when_the_group_does_not_sync_claude_md() {
    let t = T::new().await;
    let h = t.signed_in().await;
    let gf = json!({"schema": 1, "name": "Team", "members": ["ann"], "sync": {"claudeMd": false}});
    t.serve_repo("zed/team", "h1", &[("CLAUDE.md", "- A\n")], gf, true)
        .await;
    h.send(Command::Join {
        repo: "zed/team".into(),
    })
    .await
    .unwrap();
    let e = h
        .send(Command::EditLine {
            source: Some("zed/team".into()),
            line: "- A".into(),
            text: "- B".into(),
        })
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("zed/team does not sync CLAUDE.md"),
        "{e}"
    );
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
async fn a_changed_group_setting_is_a_local_edit_of_settings_shared_json_as_in_v1() {
    let t = T::new().await;
    let h = t.signed_in().await;
    let gf = json!({"schema": 1, "name": "Team", "members": ["ann"], "sync": {"settings": true}});
    t.join_served(
        &h,
        "zed/team",
        "h1",
        &[("settings.shared.json", "{\"model\":\"opus\"}\n")],
        gf,
        true,
    )
    .await;
    h.send(Command::SyncNow).await.unwrap();
    let list = || async {
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
    assert!(std::fs::read_to_string(t.file("settings.json"))
        .unwrap()
        .contains("opus"));
    assert!(list().await.is_empty());
    std::fs::write(t.file("settings.json"), "{\"model\":\"haiku\"}").unwrap();
    let e = list().await;
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].path, "settings.shared.json");
}

#[tokio::test(flavor = "multi_thread")]
async fn flags_turned_off_in_the_group_file_apply_in_the_same_pass() {
    let t = T::new().await;
    let h = t.signed_in().await;
    let on = json!({"schema": 1, "name": "Team", "members": ["ann"], "sync": {"settings": true}});
    t.join_served(
        &h,
        "zed/team",
        "h1",
        &[("settings.shared.json", "{\"model\":\"opus\"}\n")],
        on,
        true,
    )
    .await;
    h.send(Command::SyncNow).await.unwrap();
    assert!(std::fs::read_to_string(t.file("settings.json"))
        .unwrap()
        .contains("opus"));
    let off = json!({"schema": 1, "name": "Team", "members": ["ann"], "sync": {"settings": false}});
    t.serve_repo(
        "zed/team",
        "h2",
        &[("settings.shared.json", "{\"model\":\"opus\"}\n")],
        off,
        true,
    )
    .await;
    h.send(Command::SyncNow).await.unwrap();
    assert!(!std::fs::read_to_string(t.file("settings.json"))
        .unwrap_or_default()
        .contains("opus"));
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
    let Event::SharePreview { added, secret } = h
        .send(Command::SharePreview {
            repo: repo.clone(),
            path: "skills/s/SKILL.md".into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!((added, secret), (vec!["mine".to_string()], None));
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
    let Event::SharePreview { secret, .. } = h
        .send(Command::SharePreview {
            repo: repo.clone(),
            path: "skills/s/SKILL.md".into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(secret.as_deref(), Some(e.0.as_str()));
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
async fn an_allow_marker_on_one_line_does_not_unblock_a_secret_on_another() {
    let t = T::new().await;
    let h = t
        .group_with("h1", &[("skills/s/SKILL.md", "---\nname: s\n---\nhello\n")])
        .await;
    h.send(Command::SyncNow).await.unwrap();
    let token = format!("ghp_{}", "a".repeat(36));
    std::fs::write(
        t.file("skills/s/SKILL.md"),
        format!("ok {token} # claude-sync: allow-secret\nreal {token}\n"),
    )
    .unwrap();
    let e = h
        .send(Command::ShareEdit {
            repo: "zed/team".into(),
            path: "skills/s/SKILL.md".into(),
        })
        .await
        .unwrap_err();
    assert!(
        e.0.starts_with("secret found in skills/s/SKILL.md:2 ("),
        "{}",
        e.0
    );
    let Event::SharePreview { secret, .. } = h
        .send(Command::SharePreview {
            repo: "zed/team".into(),
            path: "skills/s/SKILL.md".into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(secret.as_deref(), Some(e.0.as_str()));
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
    // Open copy: the saved copy of mine lies where the pass kept it (Parity 39)
    let (repo, p, at) = (c[0].repo.clone(), c[0].path.clone(), c[0].saved_at);
    let Event::Path(Some(f)) = h
        .send(Command::ConflictOpen {
            repo: repo.clone(),
            path: p.clone(),
            saved_at: at,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "mine\n");
    // a path that climbs out of the folder is refused
    assert!(h
        .send(Command::ConflictOpen {
            repo: repo.clone(),
            path: "../x".into(),
            saved_at: at,
        })
        .await
        .is_err());
    // Delete copy: the file and the list entry go
    h.send(Command::ConflictDelete {
        repo,
        path: p,
        saved_at: at,
    })
    .await
    .unwrap();
    assert!(!std::path::Path::new(&f).exists());
    let Event::Conflicts(c) = h.send(Command::Conflicts).await.unwrap() else {
        panic!()
    };
    assert!(c.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_test_notification_is_one_new_notice_for_the_shell_to_show() {
    let t = T::new().await;
    let h = t.signed_in().await;
    h.send(Command::TestNotification).await.unwrap();
    let s = h.snapshot();
    assert_eq!(s.unread, 1);
    assert_eq!(
        (s.notifications[0].kind.as_str(), s.notifications[0].read),
        ("test", false)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn app_coverage_lists_joined_groups_the_app_cannot_reach() {
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
    h.send(Command::Join {
        repo: "ann/team".into(),
    })
    .await
    .unwrap();
    t.mock(
        "GET",
        "/user/installations",
        200,
        json!({"total_count": 1, "installations": [{"id": 7}]}),
    )
    .await;
    let repos =
        |full: &str| json!({"total_count": 1, "repositories": [{"name": "x", "full_name": full}]});
    t.mock(
        "GET",
        "/user/installations/7/repositories",
        200,
        repos("ann/other"),
    )
    .await;
    h.send(Command::AppCoverage).await.unwrap();
    assert_eq!(h.snapshot().app_uncovered, vec!["ann/team".to_string()]);
    t.server.reset().await;
    t.mock(
        "GET",
        "/user/installations",
        200,
        json!({"installations": [{"id": 7}]}),
    )
    .await;
    t.mock(
        "GET",
        "/user/installations/7/repositories",
        200,
        repos("Ann/Team"),
    )
    .await;
    h.send(Command::AppCoverage).await.unwrap();
    assert!(h.snapshot().app_uncovered.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_install_note_lists_what_the_v1_import_skipped_or_did_not_know() {
    use claude_sync_store::{import_v1, DaemonLockHeld, Db};
    let t = T::new().await;
    let state = t.dir.path().join(".claude-sync");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("app.json"), r#"{"paused":false}"#).unwrap();
    std::fs::write(state.join("jobs.json"), "{ torn").unwrap();
    std::fs::write(state.join("surprise.json"), "{}").unwrap();
    let db = Db::open(&state.join("claude-sync.db")).unwrap();
    import_v1(
        &db,
        &state,
        &t.dir.path().join(".claude"),
        &DaemonLockHeld::claim(),
    )
    .unwrap();
    drop(db);
    let h = t.signed_in().await;
    let Some(InstallNote::Skipped { files, total }) = h.snapshot().install else {
        panic!("no note")
    };
    assert_eq!(files, ["jobs.json", "surprise.json"]);
    assert!(total >= 3, "{total}");
}

#[tokio::test(flavor = "multi_thread")]
async fn recent_logs_are_the_newest_file_redacted_and_cut() {
    let t = T::new().await;
    let h = t.signed_in().await;
    assert_eq!(
        h.send(Command::RecentLogs).await.unwrap(),
        Event::Text(String::new())
    );
    let dir = t.dir.path().join(".claude-sync/logs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("claude-sync.2026-01-01.log"), "old\n").unwrap();
    let secret = format!("ghp_{}", "a".repeat(36));
    let body = format!("{}\nwith token {secret}\n", "x".repeat(300 * 1024));
    std::fs::write(dir.join("claude-sync.2026-01-02.log"), body).unwrap();
    let Event::Text(out) = h.send(Command::RecentLogs).await.unwrap() else {
        panic!()
    };
    assert!(out.len() <= 200 * 1024 + 64, "{}", out.len());
    assert!(out.ends_with("[redacted]") && !out.contains(&secret) && !out.contains("old"));
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
            minute: Duration::from_secs(60),
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
                c.run_job(None, "nightly", manual)
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
        assert!(h.snapshot().jobs[0].last_run > 0);

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
            ["run-job", "zed/team", "nightly", "--now"]
        );
    }

    /// Parity 79: a job that belongs to a pack registers only while the pack is on here.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_packs_job_registers_only_while_the_pack_is_on() {
        let (r, h) = rig().await;
        let manifest = r#"{"schema":1,"id":"ext","title":"Ext","summary":"A pack","setup":"setup.md","check":{"run":["true"]},"jobs":["nightly"]}"#;
        r.t.serve(
            "h2",
            &[
                ("schedules.json", &job("hello world")),
                ("packs/ext/pack.json", manifest),
                ("packs/ext/setup.md", "Install.\n"),
            ],
        )
        .await;
        h.send(Command::SyncNow).await.unwrap();
        assert!(h.send(acc("nightly")).await.is_err(), "no job while off");
        assert!(!r.plist().exists(), "the pack is off: no task");
        assert!(h.snapshot().jobs.is_empty(), "and no job card yet");
        let pk = |c: fn(String, String) -> Command| c("zed/team".into(), "ext".into());
        h.send(pk(|repo, id| Command::PackCheck { repo, id }))
            .await
            .unwrap();
        h.send(pk(|repo, id| Command::PackOn { repo, id }))
            .await
            .unwrap();
        h.send(acc("nightly")).await.unwrap();
        assert!(r.plist().exists(), "on: registered");
        h.send(pk(|repo, id| Command::PackOff { repo, id }))
            .await
            .unwrap();
        assert!(!r.plist().exists(), "off again: the task is removed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn leaving_a_group_unregisters_its_jobs() {
        let (r, h) = rig().await;
        h.send(acc("nightly")).await.unwrap();
        assert!(r.plist().exists());
        h.send(Command::Leave {
            repo: "zed/team".into(),
        })
        .await
        .unwrap();
        assert!(!r.plist().exists(), "the OS task went with the group");
        assert!(h.snapshot().jobs.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_job_that_hits_its_timeout_is_killed() {
        use std::os::unix::fs::PermissionsExt;
        let (mut r, h) = rig().await;
        h.send(acc("nightly")).await.unwrap();
        let claude = r.t.dir.path().join("bin/claude");
        std::fs::write(&claude, "#!/bin/sh\nsleep 60\n").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        r.env.minute = Duration::from_millis(100); // the 5 minute limit is half a second
        let start = std::time::Instant::now();
        assert_eq!(r.run(false).await.unwrap(), "timed out after 5 min");
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the sleep was killed"
        );
        let log = std::fs::read_dir(r.logs())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(std::fs::read_to_string(log)
            .unwrap()
            .contains("timed out after 5 min"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_helper_asks_the_local_claude_and_skips_empty_or_secret_text() {
        use std::os::unix::fs::PermissionsExt;
        let (r, h) = rig().await;
        let claude = r.t.dir.path().join("bin/claude");
        std::fs::write(
            &claude,
            "#!/bin/sh\ncat > /dev/null\necho '{\"result\": \"- adds a rule\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ask = |text: &str| Command::Explain {
            task: ExplainTask::ClaudeMdReview,
            text: text.into(),
        };
        assert_eq!(
            h.send(ask("+ use tabs")).await.unwrap(),
            Event::Text("- adds a rule".into())
        );
        let e = h.send(ask("  ")).await.unwrap_err();
        assert_eq!(e.0, "There is nothing to explain yet.");
        let e = h
            .send(ask("key ghp_abcdefghijklmnopqrstuvwxyz0123456789"))
            .await
            .unwrap_err();
        assert!(e.0.contains("looks like it contains a secret"), "{}", e.0);
        // switched off, the helper is not offered; back on, it answers again
        assert!(h.snapshot().helper);
        h.send(Command::Helper { on: false }).await.unwrap();
        assert!(!h.snapshot().helper);
        let e = h.send(ask("+ use tabs")).await.unwrap_err();
        assert!(e.0.contains("helper is off"), "{}", e.0);
        h.send(Command::Helper { on: true }).await.unwrap();
        assert!(h.send(ask("+ use tabs")).await.is_ok());
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
            minute: Duration::from_secs(60),
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
        /// commit -> its parent, so a ref move can be refused when it is not a fast forward
        parents: HashMap<String, String>,
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
                    g.parents.insert(id.clone(), g.head.clone());
                    ResponseTemplate::new(201).set_body_json(json!({"sha": id}))
                }),
            ),
            on(
                "PATCH",
                format!("^{REPO}/git/refs/heads/main$"),
                Box::new(move |g, r| {
                    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                    let sha = v["sha"].as_str().unwrap().to_string();
                    // as on GitHub, an unforced ref move to a commit that is not on top of the
                    // head loses a race: without this, two passes at once silently lost a push
                    if g.parents.get(&sha) != Some(&g.head) {
                        return ResponseTemplate::new(422)
                            .set_body_json(json!({"message": "Update is not a fast forward"}));
                    }
                    g.head = sha;
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
    async fn a_test_home_never_joins_personal_memory_by_itself() {
        let git = new_git();
        let a = device("laptop", &git).await;
        a.h.send(Command::PersonalOn).await.unwrap();
        // a plain new device joins by itself (v1 default)
        let b = device("desktop", &git).await;
        b.sync().await;
        assert!(b.personal().await.on);
        // a dev-home device with the flag does not, and is not offered either
        let t = T::named("test").await;
        t.no_auto.store(true, Ordering::SeqCst);
        mount(&t, &git).await;
        let c = Dev {
            h: t.signed_in().await,
            t,
        };
        c.sync().await;
        let pc = c.personal().await;
        assert!(!pc.on && !pc.offer, "{pc:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn new_devices_join_switch_is_written_to_the_repo_and_the_offer_follows() {
        let git = new_git();
        let a = device("laptop", &git).await;
        a.h.send(Command::PersonalOn).await.unwrap();
        assert!(a.personal().await.new_devices_join);
        a.h.send(Command::PersonalJoinAuto { on: false })
            .await
            .unwrap();
        assert!(!a.personal().await.new_devices_join);
        assert!(git.lock().unwrap().files()["claude-sync-personal.json"].contains("false"));
        // a new device is offered the repo, not joined
        let b = device("desktop", &git).await;
        b.sync().await;
        let pb = b.personal().await;
        assert!(!pb.on && pb.offer, "{pb:?}");
        a.h.send(Command::PersonalJoinAuto { on: true })
            .await
            .unwrap();
        assert!(a.personal().await.new_devices_join);
        assert!(git.lock().unwrap().files()["claude-sync-personal.json"].contains("true"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn files_that_stay_on_this_device_are_listed_with_v1_wording() {
        let git = new_git();
        let a = device("laptop", &git).await;
        a.h.send(Command::PersonalOn).await.unwrap();
        let big = a.memory().replace("MEMORY.md", "big.md");
        a.write(&big, &"x".repeat(300 * 1024));
        a.write(
            "CLAUDE.md",
            "# Mine\ntoken ghp_abcdefghijklmnopqrstuvwxyz0123456789\n",
        );
        a.sync().await;
        let stays = a.personal().await.stays;
        assert!(
            stays
                .iter()
                .any(|s| s.ends_with("/big.md: not synced, it is over 256 KiB")),
            "{stays:?}"
        );
        assert!(
            stays
                .iter()
                .any(|s| s.starts_with("CLAUDE.md: not synced, line 2 looks like a secret (")),
            "{stays:?}"
        );
        assert!(!git
            .lock()
            .unwrap()
            .files()
            .values()
            .any(|v| v.contains("ghp_abc")));
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

    /// v2-Budgets: an idle hour (60 ticks, 3 groups + personal) costs at most 424 GitHub
    /// requests, counted by the client (`take_counts`, flushed into the `requests` table), and
    /// every one is conditional. The clock is tokio's paused one; the fake `now` follows it.
    #[tokio::test(start_paused = true)]
    async fn an_idle_hour_costs_at_most_424_requests() {
        const GROUPS: [&str; 3] = ["zed/g1", "zed/g2", "zed/g3"];
        let git = new_git();
        let t = T::new().await;
        mount(&t, &git).await;
        let h = t.signed_in_tick(Duration::from_secs(60)).await;
        // setup runs paused, so the loop's idle ticks do not mix in
        h.send(Command::Pause).await.unwrap();
        h.send(Command::SetInterval { secs: 60 }).await.unwrap();
        for g in GROUPS {
            t.mock(
                "GET",
                &format!("/repos/{g}"),
                200,
                json!({"name": "x", "full_name": g, "private": true}),
            )
            .await;
            t.push_chain(g).await;
            h.send(Command::Join { repo: g.into() }).await.unwrap();
        }
        for (i, g) in GROUPS.iter().enumerate() {
            let gf = json!({"schema": 1, "name": "Team", "members": ["ann"]});
            t.serve_repo(
                g,
                "h1",
                &[("skills/a/SKILL.md", "---\nname: a\n---\n")],
                gf,
                i == 0,
            )
            .await;
        }
        mount(&t, &git).await;
        h.send(Command::PersonalOn).await.unwrap();
        assert!(h.snapshot().personal.on, "{:?}", h.snapshot().personal);
        for _ in 0..3 {
            t.clock.fetch_add(60, Ordering::SeqCst);
            h.send(Command::SyncNow).await.unwrap();
        }
        // From here the repos are static: answer 304 to the ETag, 200 with it otherwise.
        // The personal head moves when this device pushes, so its ETag is the head itself.
        let g = git.clone();
        Mock::given(method("GET"))
            .and(path_regex(format!("^{REPO}/commits/main$")))
            .respond_with(Fn1(move |r: &Request| {
                let head = g.lock().unwrap().head.clone();
                let tag = format!("\"{head}\"");
                if r.headers
                    .get("if-none-match")
                    .is_some_and(|v| v == tag.as_str())
                {
                    ResponseTemplate::new(304)
                } else {
                    ResponseTemplate::new(200)
                        .insert_header("etag", tag.as_str())
                        .set_body_json(json!({"sha": head}))
                }
            }))
            .with_priority(1)
            .mount(&t.server)
            .await;
        let mut repos: Vec<(String, String)> = GROUPS
            .iter()
            .map(|g| (g.to_string(), "h1".into()))
            .collect();
        repos.push(("ann/claude-sync-personal".into(), String::new()));
        for (r, head) in &repos {
            let (o, n) = r.split_once('/').unwrap();
            let group = !head.is_empty();
            for (re, body) in [
                (format!("^/repos/{r}/commits/main$"), json!({"sha": head})),
                (format!("^/repos/{r}/pulls$"), json!([])),
                (
                    format!("^/repos/{r}$"),
                    json!({"name": n, "full_name": r, "private": true, "owner": {"login": o}}),
                ),
            ] {
                if !group && re.ends_with("commits/main$") {
                    continue;
                }
                Mock::given(method("GET"))
                    .and(path_regex(re.clone()))
                    .and(wiremock::matchers::header("if-none-match", "\"e1\""))
                    .respond_with(ResponseTemplate::new(304))
                    .with_priority(1)
                    .mount(&t.server)
                    .await;
                Mock::given(method("GET"))
                    .and(path_regex(re))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .insert_header("etag", "\"e1\"")
                            .set_body_json(body),
                    )
                    .with_priority(2)
                    .mount(&t.server)
                    .await;
            }
        }
        // one warm pass learns the ETags, as a running app has
        t.clock.fetch_add(60, Ordering::SeqCst);
        h.send(Command::SyncNow).await.unwrap();
        let db_path = t.dir.path().join(".claude-sync/claude-sync.db");
        let counted = || -> u64 {
            let db = claude_sync_store::Db::open(&db_path).unwrap();
            db.requests().unwrap().iter().map(|r| r.n).sum()
        };
        h.send(Command::Resume).await.unwrap();
        let (base_db, base_srv) = (counted(), t.server.received_requests().await.unwrap().len());
        for _ in 0..60 {
            t.clock.fetch_add(60, Ordering::SeqCst);
            tokio::time::advance(Duration::from_secs(60)).await;
            // the actor takes the timer branch first; then a no-op command answers only after
            // the tick ended (the actor runs one thing at a time)
            tokio::task::yield_now().await;
            h.send(Command::SignInCancel).await.unwrap();
        }
        let (db, srv) = (
            counted() - base_db,
            t.server.received_requests().await.unwrap().len() - base_srv,
        );
        println!("idle hour: {db} counted requests, {srv} seen by the fake");
        assert!(db <= 424, "{db} requests in an idle hour");
        assert!(srv <= 424, "{srv} requests seen by the fake");
    }
}

// ---- proposals, copy, revert, history (Parity 33-36, 83-85) ----

mod proposals {
    use super::*;

    pub struct P {
        pub n: u64,
        pub by: &'static str,
        pub head: &'static str,
        pub up: &'static str,
        pub mergeable: bool,
        /// (reviewer, state, commit)
        pub reviews: Vec<(&'static str, &'static str, &'static str)>,
    }

    pub fn p(n: u64, by: &'static str) -> P {
        P {
            n,
            by,
            head: "p1",
            up: "u1",
            mergeable: true,
            reviews: vec![],
        }
    }

    impl P {
        fn json(&self, mergeable: bool) -> serde_json::Value {
            let mut v = json!({"number": self.n, "title": format!("{}: share x", self.by),
                "state": "open", "head": {"sha": self.head, "ref": format!("proposal/{}-1", self.by)},
                "user": {"login": self.by}, "updated_at": self.up, "created_at": "2026-01-02T03:04:05Z",
                "html_url": format!("https://example.test/pull/{}", self.n)});
            if mergeable {
                v["mergeable"] = json!(self.mergeable);
            }
            v
        }
    }

    impl T {
        pub async fn push_tail(&self, repo: &str) {
            let r = format!("/repos/{repo}");
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
            self.mock("POST", &format!("{r}/git/refs"), 201, json!({}))
                .await;
            self.mock(
                "POST",
                &format!("{r}/pulls"),
                201,
                json!({"number": 7, "state": "open"}),
            )
            .await;
        }
        /// Serve, mock the push calls, join.
        pub async fn join_served(
            &self,
            h: &Handle,
            repo: &str,
            head: &str,
            files: &[(&str, &str)],
            gf: serde_json::Value,
            first: bool,
        ) {
            self.serve_repo(repo, head, files, gf, first).await;
            self.push_tail(repo).await;
            h.send(Command::Join { repo: repo.into() }).await.unwrap();
        }
        pub async fn pulls(&self, repo: &str, ps: &[P]) {
            let r = format!("/repos/{repo}");
            let list: Vec<_> = ps.iter().map(|x| x.json(false)).collect();
            self.mock("GET", &format!("{r}/pulls"), 200, json!(list))
                .await;
            for x in ps {
                let n = x.n;
                self.mock("GET", &format!("{r}/pulls/{n}"), 200, x.json(true))
                    .await;
                self.mock(
                    "GET",
                    &format!("{r}/pulls/{n}/files"),
                    200,
                    json!([{"filename": "CLAUDE.md", "status": "modified", "patch": "@@ +x"}]),
                )
                .await;
                let rev: Vec<_> = x
                    .reviews
                    .iter()
                    .map(|(u, s, c)| json!({"user": {"login": u}, "state": s, "commit_id": c}))
                    .collect();
                self.mock("GET", &format!("{r}/pulls/{n}/reviews"), 200, json!(rev))
                    .await;
            }
            self.mock("POST", &format!("{r}/pulls/1/reviews"), 200, json!({}))
                .await;
        }
        pub async fn sent(&self, verb: &str, p: &str) -> Vec<serde_json::Value> {
            self.server
                .received_requests()
                .await
                .unwrap()
                .into_iter()
                .filter(|r| r.method.as_str() == verb && r.url.path() == p)
                .map(|r| serde_json::from_slice(&r.body).unwrap_or(json!(null)))
                .collect()
        }
    }

    pub fn gf(members: &[&str], proposal: bool) -> serde_json::Value {
        json!({"schema": 1, "name": "Team", "members": members, "autoAcceptClaudeMd": false, "proposalMode": proposal})
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn in_proposal_mode_a_share_goes_to_a_pull_request_and_stays_a_local_edit() {
        let t = T::new().await;
        let h = t.signed_in().await;
        t.join_served(
            &h,
            "zed/team",
            "h1",
            &[("skills/s/SKILL.md", "one\n")],
            gf(&["ann"], true),
            true,
        )
        .await;
        h.send(Command::SyncNow).await.unwrap();
        std::fs::write(t.file("skills/s/SKILL.md"), "mine\n").unwrap();
        let e = h
            .send(Command::ShareEdit {
                repo: "zed/team".into(),
                path: "skills/s/SKILL.md".into(),
            })
            .await
            .unwrap();
        assert_eq!(e, Event::Proposed(7));
        assert!(t
            .sent("PATCH", "/repos/zed/team/git/refs/heads/main")
            .await
            .is_empty());
        let r = t.sent("POST", "/repos/zed/team/git/refs").await;
        assert_eq!(
            r[0]["ref"], "refs/heads/proposal/ann-19700112134640",
            "proposal/<login>-<stamp>"
        );
        let pr = t.sent("POST", "/repos/zed/team/pulls").await;
        assert_eq!(pr[0]["head"], "proposal/ann-19700112134640");
        assert_eq!(pr[0]["base"], "main");
        assert_eq!(t.pushed_blobs().await, ["mine\n"]);
        let Event::Edits(ed) = h
            .send(Command::EditsList {
                repo: "zed/team".into(),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(ed.len(), 1, "nothing is shared until it is merged");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn in_proposal_mode_a_group_line_edit_becomes_a_pull_request() {
        let t = T::new().await;
        let h = t.signed_in().await;
        t.join_served(
            &h,
            "zed/team",
            "h1",
            &[("CLAUDE.md", "- A\n")],
            gf(&["ann"], true),
            true,
        )
        .await;
        h.send(Command::SyncNow).await.unwrap();
        let e = h
            .send(Command::EditLine {
                source: Some("zed/team".into()),
                line: "- A".into(),
                text: "- B".into(),
            })
            .await
            .unwrap();
        assert_eq!(e, Event::Proposed(7));
        assert!(t
            .sent("PATCH", "/repos/zed/team/git/refs/heads/main")
            .await
            .is_empty());
        assert_eq!(t.pushed_blobs().await, ["- B\n"]);
    }

    async fn list(h: &Handle) -> Vec<ProposalView> {
        let Event::Proposals(v) = h
            .send(Command::Proposals {
                repo: "zed/team".into(),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        v
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn approvals_never_count_the_author_or_an_old_head_and_never_your_own() {
        let t = T::new().await;
        let h = t.signed_in().await;
        t.join_served(
            &h,
            "zed/team",
            "h1",
            &[],
            gf(&["ann", "zed", "bo"], true),
            true,
        )
        .await;
        let mut seven = p(7, "zed");
        seven.reviews = vec![
            ("zed", "APPROVED", "p1"),
            ("bo", "APPROVED", "p1"),
            ("cy", "APPROVED", "old"),
            ("dee", "APPROVED", "p1"),
            ("dee", "CHANGES_REQUESTED", "p1"),
        ];
        let mut ten = p(10, "zed");
        ten.mergeable = false;
        t.pulls("zed/team", &[seven, p(8, "ann"), ten]).await;
        t.mock("POST", "/repos/zed/team/pulls/7/reviews", 200, json!({}))
            .await;
        let v = list(&h).await;
        assert_eq!(v[0].created.as_deref(), Some("2026-01-02T03:04:05Z"));
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].approvals, ["bo"]);
        assert!(v[1].mine && !v[0].mine);
        assert!(v[2].needs_update && !v[0].needs_update);
        assert_eq!(v[0].files[0].path, "CLAUDE.md");

        let e = h
            .send(Command::ProposalApprove {
                repo: "zed/team".into(),
                number: 8,
            })
            .await
            .unwrap_err();
        assert_eq!(
            e.0,
            "you cannot approve your own proposal; another member has to"
        );
        let e = h
            .send(Command::ProposalApprove {
                repo: "zed/team".into(),
                number: 10,
            })
            .await
            .unwrap_err();
        assert_eq!(e.0, "proposal #10 needs an update before it can be merged");
        h.send(Command::ProposalApprove {
            repo: "zed/team".into(),
            number: 7,
        })
        .await
        .unwrap();
        assert_eq!(
            t.sent("POST", "/repos/zed/team/pulls/7/reviews").await[0]["event"],
            "APPROVE"
        );
        assert!(
            t.sent("PUT", "/repos/zed/team/pulls/7/merge")
                .await
                .is_empty(),
            "a member only approves"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn decline_comments_then_closes_and_accept_all_and_decline_all_skip_what_they_must() {
        let t = T::new().await;
        let h = t.signed_in().await;
        t.join_served(&h, "zed/team", "h1", &[], gf(&["ann", "zed"], true), true)
            .await;
        let mut ten = p(10, "zed");
        ten.mergeable = false;
        let mut nine = p(9, "zed");
        nine.reviews = vec![("ann", "APPROVED", "p1")];
        t.pulls("zed/team", &[p(7, "zed"), p(8, "ann"), nine, ten])
            .await;
        for n in [7, 8, 9, 10] {
            t.mock(
                "POST",
                &format!("/repos/zed/team/pulls/{n}/reviews"),
                200,
                json!({}),
            )
            .await;
            t.mock(
                "POST",
                &format!("/repos/zed/team/issues/{n}/comments"),
                201,
                json!({}),
            )
            .await;
            t.mock(
                "PATCH",
                &format!("/repos/zed/team/pulls/{n}"),
                200,
                json!({"number": n}),
            )
            .await;
        }
        t.mock(
            "GET",
            "/repos/zed/team/pulls/99",
            404,
            json!({"message": "Not Found"}),
        )
        .await;
        let e = h
            .send(Command::ProposalDecline {
                repo: "zed/team".into(),
                number: 99,
            })
            .await
            .unwrap_err();
        assert_eq!(e.0, "proposal #99 is not open");

        h.send(Command::ProposalsAcceptAll {
            repo: "zed/team".into(),
        })
        .await
        .unwrap();
        // only #7: #8 is mine, #9 is approved by me already, #10 is behind main
        for (n, k) in [(7, 1), (8, 0), (9, 0), (10, 0)] {
            assert_eq!(
                t.sent("POST", &format!("/repos/zed/team/pulls/{n}/reviews"))
                    .await
                    .len(),
                k,
                "#{n}"
            );
        }

        h.send(Command::ProposalDecline {
            repo: "zed/team".into(),
            number: 7,
        })
        .await
        .unwrap();
        assert_eq!(
            t.sent("POST", "/repos/zed/team/issues/7/comments").await[0]["body"],
            "Declined by ann in claude-sync."
        );
        assert_eq!(
            t.sent("PATCH", "/repos/zed/team/pulls/7").await[0]["state"],
            "closed"
        );

        h.send(Command::ProposalsDeclineAll {
            repo: "zed/team".into(),
        })
        .await
        .unwrap();
        for n in [8, 9, 10] {
            assert_eq!(
                t.sent("PATCH", &format!("/repos/zed/team/pulls/{n}"))
                    .await
                    .len(),
                1,
                "#{n}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_pull_request_is_read_again_only_when_its_head_or_updated_at_moved() {
        let t = T::new().await;
        let h = t.signed_in().await;
        t.join_served(&h, "zed/team", "h1", &[], gf(&["ann", "zed"], true), true)
            .await;
        let r = "/repos/zed/team";
        let list = |up: &str| json!([p(7, "zed").json_with(up)]);
        Mock::given(method("GET"))
            .and(path(format!("{r}/pulls")))
            .respond_with(ResponseTemplate::new(200).set_body_json(list("u1")))
            .up_to_n_times(2)
            .mount(&t.server)
            .await;
        t.mock("GET", &format!("{r}/pulls"), 200, list("u2")).await;
        t.mock("GET", &format!("{r}/pulls/7"), 200, p(7, "zed").json(true))
            .await;
        t.mock("GET", &format!("{r}/pulls/7/files"), 200, json!([]))
            .await;
        t.mock("GET", &format!("{r}/pulls/7/reviews"), 200, json!([]))
            .await;
        list_it(&h).await;
        list_it(&h).await;
        assert_eq!(files_gets(&t).await, 1, "the second list changed nothing");
        list_it(&h).await;
        assert_eq!(files_gets(&t).await, 2, "updated_at moved");
    }

    async fn files_gets(t: &T) -> usize {
        t.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|q| q.url.path() == "/repos/zed/team/pulls/7/files")
            .count()
    }

    async fn list_it(h: &Handle) {
        list(h).await;
    }

    impl P {
        fn json_with(&self, up: &str) -> serde_json::Value {
            let mut v = self.json(false);
            v["updated_at"] = json!(up);
            v
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_host_merges_by_approving_and_on_sync_only_after_a_current_member_approved() {
        let t = T::new().await;
        let h = t.signed_in().await;
        let g = gf(&["ann", "bo"], true);
        t.join_served(&h, "ann/team", "h1", &[], g, true).await;
        let mut gone = p(8, "bo");
        gone.reviews = vec![("zed", "APPROVED", "p1")]; // not a member any more
        let mut behind = p(9, "bo");
        behind.mergeable = false;
        behind.reviews = vec![("bo2", "APPROVED", "p1")];
        let mut ok = p(11, "bo");
        ok.reviews = vec![("bo", "APPROVED", "p1")];
        // a member other than the author approved #12; the author of #11 approved himself
        let mut twelve = p(12, "cy");
        twelve.reviews = vec![("bo", "APPROVED", "p1")];
        t.pulls("ann/team", &[ok, twelve, gone, behind]).await;
        for n in [8, 9, 11, 12] {
            t.mock(
                "PUT",
                &format!("/repos/ann/team/pulls/{n}/merge"),
                200,
                json!({"sha": "m"}),
            )
            .await;
        }
        h.send(Command::SyncNow).await.unwrap(); // caches proposalMode, then merges
        h.send(Command::SyncNow).await.unwrap();
        let merged = |n: u64| {
            let t = &t;
            async move {
                t.sent("PUT", &format!("/repos/ann/team/pulls/{n}/merge"))
                    .await
            }
        };
        assert!(
            merged(11).await.is_empty(),
            "the author's own approval is not one"
        );
        assert!(merged(8).await.is_empty(), "approved by someone who left");
        assert!(merged(9).await.is_empty(), "behind main");
        let m = merged(12).await;
        assert!(!m.is_empty());
        assert_eq!(m[0]["sha"], "p1", "pinned to the head that was read");

        // the host's own approval merges at once
        h.send(Command::ProposalApprove {
            repo: "ann/team".into(),
            number: 11,
        })
        .await
        .unwrap();
        assert_eq!(merged(11).await.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn proposal_mode_is_a_host_setting_in_the_group_file() {
        let t = T::new().await;
        let h = t.signed_in().await;
        t.join_served(&h, "ann/team", "h1", &[], gf(&["ann"], false), true)
            .await;
        h.send(Command::SetProposalMode {
            repo: "ann/team".into(),
            on: true,
        })
        .await
        .unwrap();
        let blob = t.sent("POST", "/repos/ann/team/git/blobs").await;
        let text = String::from_utf8(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                blob[0]["content"].as_str().unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(text.contains("\"proposalMode\": true"), "{text}");
        let t2 = T::new().await;
        let h2 = t2.group_with("h1", &[]).await;
        let e = h2
            .send(Command::SetProposalMode {
                repo: "zed/team".into(),
                on: true,
            })
            .await
            .unwrap_err();
        assert!(e.0.starts_with("Only zed can change zed/team"), "{}", e.0);
    }
}

mod copy_revert_history {
    use super::proposals::gf;
    use super::*;

    const KEY: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

    async fn two_groups(push: bool) -> (T, Handle) {
        let t = T::new().await;
        let h = t.signed_in().await;
        let src = format!("# Team\n- use tabs\n- token {KEY}\n");
        t.join_served(
            &h,
            "zed/team",
            "h1",
            &[
                ("CLAUDE.md", &src),
                ("skills/lint/SKILL.md", "lint\n"),
                ("skills/same/SKILL.md", "same\n"),
                ("agents/reviewer.md", "review\n"),
            ],
            gf(&["ann"], false),
            true,
        )
        .await;
        // the target: ann hosts it (the repo reply says whether she can push)
        let perms = json!({"admin": push, "push": push, "pull": true});
        t.mock(
            "GET",
            "/repos/ann/crew",
            200,
            json!({"name": "crew", "full_name": "ann/crew", "private": true, "permissions": perms}),
        )
        .await;
        t.join_served(
            &h,
            "ann/crew",
            "k1",
            &[
                ("CLAUDE.md", "# Crew\n"),
                ("skills/same/SKILL.md", "same\n"),
            ],
            gf(&["ann"], false),
            false,
        )
        .await;
        h.send(Command::SyncNow).await.unwrap();
        (t, h)
    }

    fn req() -> CopyRequest {
        CopyRequest {
            claude_md: format!("- use tabs\n- token {KEY}"),
            skills: vec!["lint".into(), "same".into()],
            agents: vec!["reviewer.md".into()],
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn copy_previews_then_puts_claude_md_skills_and_agents_into_the_other_group() {
        let (t, h) = two_groups(true).await;
        let Event::CopyPreview(pv) = h
            .send(Command::CopyPreview {
                from: "zed/team".into(),
                to: "ann/crew".into(),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert!(pv.can_push && pv.syncs_skills);
        assert!(
            pv.claude_md.contains("use tabs") && !pv.claude_md.contains("ghp_"),
            "{}",
            pv.claude_md
        );
        let s: Vec<_> = pv
            .skills
            .iter()
            .map(|i| (i.name.as_str(), i.in_target, i.same))
            .collect();
        assert_eq!(s, [("lint", false, false), ("same", true, true)]);
        assert_eq!(pv.agents[0].name, "reviewer.md");
        assert!(
            t.sent("POST", "/repos/ann/crew/git/blobs").await.is_empty(),
            "a preview writes nothing"
        );

        let e = h
            .send(Command::Copy {
                from: "zed/team".into(),
                to: "ann/crew".into(),
                req: req(),
            })
            .await
            .unwrap();
        assert_eq!(
            e,
            Event::Copied {
                files: 3,
                claude_md: true,
                proposed: None
            }
        );
        let blobs: Vec<String> = t
            .sent("POST", "/repos/ann/crew/git/blobs")
            .await
            .iter()
            .map(|b| {
                String::from_utf8(
                    base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        b["content"].as_str().unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap()
            })
            .collect();
        assert!(blobs.contains(&"lint\n".to_string()) && blobs.contains(&"review\n".to_string()));
        let md = blobs.iter().find(|b| b.starts_with("# Crew")).unwrap();
        assert!(md.starts_with("# Crew\n\n## Team\n\n- use tabs"), "{md}");
        assert!(!md.contains("ghp_"), "secrets are redacted");
        let tree = t.sent("POST", "/repos/ann/crew/git/trees").await;
        let mut paths: Vec<&str> = tree[0]["tree"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"].as_str().unwrap())
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            ["CLAUDE.md", "agents/reviewer.md", "skills/lint/SKILL.md"]
        );
        let m = t.sent("POST", "/repos/ann/crew/git/commits").await;
        assert!(
            m[0]["message"]
                .as_str()
                .unwrap()
                .contains("copy from zed/team"),
            "{m:?}"
        );
        // the target's base is untouched: the next sync delivers the files, no local edits
        let Event::Edits(ed) = h
            .send(Command::EditsList {
                repo: "ann/crew".into(),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert!(ed.is_empty());

        // errors in v1's words
        for (to, r, want) in [
            ("zed/team", req(), "pick another group to copy to"),
            (
                "ann/crew",
                CopyRequest {
                    skills: vec!["nope".into()],
                    ..Default::default()
                },
                "zed/team has no skill named nope",
            ),
        ] {
            let e = h
                .send(Command::Copy {
                    from: "zed/team".into(),
                    to: to.into(),
                    req: r,
                })
                .await
                .unwrap_err();
            assert_eq!(e.0, want);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn copy_needs_push_access_to_the_target() {
        let (t, h) = two_groups(false).await;
        let e = h
            .send(Command::Copy {
                from: "zed/team".into(),
                to: "ann/crew".into(),
                req: req(),
            })
            .await
            .unwrap_err();
        assert!(
            e.0.starts_with("You can't push to Team: your GitHub account can read it"),
            "{}",
            e.0
        );
        assert!(t.sent("POST", "/repos/ann/crew/git/blobs").await.is_empty());
    }

    /// ann hosts `ann/team`; main is at `cc`, the commit `bb` (parent `aa`) touched the skill
    /// and added an agent.
    async fn revert_rig(head_skill: &str) -> (T, Handle) {
        let t = T::new().await;
        let h = t.signed_in().await;
        let gfv = gf(&["ann"], false);
        let readme = ("README.md", "x\n");
        t.join_served(
            &h,
            "ann/team",
            "cc",
            &[
                ("skills/s/SKILL.md", head_skill),
                ("agents/new.md", "new\n"),
                readme,
            ],
            gfv.clone(),
            true,
        )
        .await;
        t.serve_repo(
            "ann/team",
            "bb",
            &[
                ("skills/s/SKILL.md", "two\n"),
                ("agents/new.md", "new\n"),
                readme,
            ],
            gfv.clone(),
            false,
        )
        .await;
        t.serve_repo(
            "ann/team",
            "aa",
            &[("skills/s/SKILL.md", "one\n"), readme],
            gfv,
            false,
        )
        .await;
        let who = json!({"name": "Ann", "date": "2026-01-02T03:04:05Z"});
        let detail = |files: serde_json::Value, parents: serde_json::Value, msg: &str| json!({"sha": "bb", "parents": parents, "files": files, "commit": {"message": msg, "author": who, "committer": who}});
        t.mock(
            "GET",
            "/repos/ann/team/commits/bb",
            200,
            detail(
                json!([
                    {"filename": "skills/s/SKILL.md", "status": "modified"},
                    {"filename": "agents/new.md", "status": "added"},
                    {"filename": "README.md", "status": "modified"}
                ]),
                json!([{"sha": "aa"}]),
                "edit skill #12",
            ),
        )
        .await;
        t.mock(
            "GET",
            "/repos/ann/team/commits/first",
            200,
            detail(json!([]), json!([]), "first"),
        )
        .await;
        t.mock(
            "GET",
            "/repos/ann/team/commits/docs",
            200,
            detail(
                json!([{"filename": "README.md", "status": "modified"}]),
                json!([{"sha": "aa"}]),
                "docs",
            ),
        )
        .await;
        h.send(Command::SyncNow).await.unwrap();
        (t, h)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn revert_previews_then_puts_each_file_back_in_a_new_commit() {
        let (t, h) = revert_rig("two\n").await;
        let c = |sha: &str, preview| {
            if preview {
                Command::RevertPreview {
                    repo: "ann/team".into(),
                    sha: sha.into(),
                }
            } else {
                Command::Revert {
                    repo: "ann/team".into(),
                    sha: sha.into(),
                }
            }
        };
        let Event::RevertPlan(plan) = h.send(c("bb", true)).await.unwrap() else {
            panic!()
        };
        let f: Vec<_> = plan
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.action.as_str(), f.blocked.is_none()))
            .collect();
        assert_eq!(
            f,
            [
                ("agents/new.md", "delete", true),
                ("skills/s/SKILL.md", "restore", true)
            ],
            "README.md is not a synced path"
        );
        assert_eq!(plan.summary, "edit skill #12");
        assert!(
            t.sent("POST", "/repos/ann/team/git/trees").await.is_empty(),
            "a preview writes nothing"
        );

        h.send(c("bb", false)).await.unwrap();
        let blobs = t.sent("POST", "/repos/ann/team/git/blobs").await;
        assert_eq!(blobs.len(), 1, "only the restored file is a blob");
        let tree = &t.sent("POST", "/repos/ann/team/git/trees").await[0]["tree"];
        assert_eq!(tree[0]["path"], "agents/new.md");
        assert!(tree[0]["sha"].is_null(), "an added file is deleted");
        let m = t.sent("POST", "/repos/ann/team/git/commits").await;
        assert!(
            m[0]["message"]
                .as_str()
                .unwrap()
                .ends_with("revert bb (edit skill #12)"),
            "{m:?}"
        );
        assert_eq!(
            t.sent("PATCH", "/repos/ann/team/git/refs/heads/main")
                .await
                .len(),
            1
        );

        for (sha, want) in [
            ("first", "first is the first commit and cannot be reverted"),
            (
                "docs",
                "Nothing to revert: the files of docs already match what they were before it",
            ),
        ] {
            assert_eq!(h.send(c(sha, true)).await.unwrap_err().0, want);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_file_that_changed_again_refuses_the_whole_revert_and_members_cannot_revert() {
        let (t, h) = revert_rig("three\n").await;
        let Event::RevertPlan(plan) = h
            .send(Command::RevertPreview {
                repo: "ann/team".into(),
                sha: "bb".into(),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        let b = plan.files.iter().find_map(|f| f.blocked.clone()).unwrap();
        assert_eq!(
            b,
            "skills/s/SKILL.md changed again after this commit; revert that first."
        );
        let e = h
            .send(Command::Revert {
                repo: "ann/team".into(),
                sha: "bb".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(e.0, b);
        assert!(t
            .sent("POST", "/repos/ann/team/git/commits")
            .await
            .is_empty());

        let t2 = T::new().await;
        let h2 = t2.group_with("h1", &[]).await;
        let e = h2
            .send(Command::RevertPreview {
                repo: "zed/team".into(),
                sha: "bb".into(),
            })
            .await
            .unwrap_err();
        assert!(e.0.starts_with("Only zed can change zed/team"), "{}", e.0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn history_has_notes_from_the_text_or_the_notes_file_and_issue_chips() {
        let t = T::new().await;
        let h = t.signed_in().await;
        let who = json!({"name": "Zed", "date": "2026-01-02T03:04:05Z"});
        let c = |sha: &str, m: &str| json!({"sha": sha, "commit": {"message": m, "author": who, "committer": who}, "author": {"login": "zed"}});
        let notes = json!({"s2": {"before": "x", "now": "y"}}).to_string();
        t.server.reset().await;
        t.mock("GET", "/user", 200, json!({"login": "ann", "id": 1}))
            .await;
        t.mock(
            "GET",
            "/repos/zed/team/commits",
            200,
            json!([
                c("s1", "edit #12 and #3\n\nBefore: tabs\nNow: spaces"),
                c("s2", "update"),
                c("s3", "plain")
            ]),
        )
        .await;
        t.join_served(
            &h,
            "zed/team",
            "s1",
            &[
                ("changes/before-now.json", &notes),
                ("skills/s/SKILL.md", "s\n"),
                ("agents/a.md", "a\n"),
                ("CLAUDE.md", "# T\n"),
            ],
            gf(&["ann"], false),
            false,
        )
        .await;
        let Event::History(l) = h
            .send(Command::History {
                repo: "zed/team".into(),
                limit: 50,
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        let got: Vec<_> = l
            .iter()
            .map(|i| {
                (
                    i.sha.as_str(),
                    i.issues.clone(),
                    i.before.as_deref(),
                    i.now.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("s1", vec![12, 3], Some("tabs"), Some("spaces")),
                ("s2", vec![], Some("x"), Some("y")),
                ("s3", vec![], None, None)
            ]
        );
        assert_eq!(l[0].summary, "edit #12 and #3");
        let reqs = t.server.received_requests().await.unwrap();
        let q = reqs
            .iter()
            .find(|r| r.url.path() == "/repos/zed/team/commits")
            .unwrap();
        assert!(
            q.url.query().unwrap().contains("per_page=20"),
            "at most 20 commits"
        );

        h.send(Command::SyncNow).await.unwrap();
        let Event::Contents(c) = h
            .send(Command::Contents {
                repo: "zed/team".into(),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        let got: Vec<_> = c
            .iter()
            .map(|i| (i.kind.as_str(), i.name.as_str()))
            .collect();
        assert_eq!(
            got,
            [("claudeMd", "CLAUDE.md"), ("agent", "a.md"), ("skill", "s")]
        );
    }
}

mod feedback {
    use super::*;

    fn input(title: &str) -> FeedbackInput {
        FeedbackInput {
            kind: "bug".into(),
            title: title.into(),
            description: "it broke".into(),
            app_version: "2.0.0".into(),
            os: "TestOS".into(),
            logs: "line one\nAuthorization: Bearer abc123secret\nlast".into(),
            screenshot: None,
        }
    }
    async fn relay(t: &T, status: u16, body: serde_json::Value) {
        t.mock("POST", "/api/feedback", status, body).await;
    }
    async fn posts(t: &T) -> Vec<serde_json::Value> {
        t.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/feedback")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_report_is_checked_redacted_and_filed() {
        let t = T::new().await;
        let h = t.signed_in().await;
        relay(
            &t,
            201,
            json!({"number": 7, "url": "https://example.test/i/7"}),
        )
        .await;
        for (f, text) in [
            (input("  "), "a title is needed"),
            (
                FeedbackInput {
                    kind: "x".into(),
                    ..input("t")
                },
                "kind must be bug or idea",
            ),
            (
                FeedbackInput {
                    screenshot: Some("aGVsbG8=".into()),
                    ..input("t")
                },
                "only PNG and JPEG images can be attached",
            ),
            (
                FeedbackInput {
                    screenshot: Some(base64::engine::general_purpose::STANDARD.encode(vec![
                        0x89u8;
                        3 * 1024 * 1024
                            + 1
                    ])),
                    ..input("t")
                },
                "screenshot is larger than 3 MB",
            ),
        ] {
            assert_eq!(h.send(Command::SendFeedback(f)).await.unwrap_err().0, text);
        }
        assert!(posts(&t).await.is_empty());
        let e = h
            .send(Command::SendFeedback(input(" Crash ")))
            .await
            .unwrap();
        assert_eq!(
            e,
            Event::Feedback {
                queued: false,
                limited: false,
                number: Some(7),
                url: Some("https://example.test/i/7".into())
            }
        );
        let p = &posts(&t).await[0];
        assert_eq!(
            (p["title"].as_str(), p["kind"].as_str()),
            (Some("Crash"), Some("bug"))
        );
        let logs = p["logs"].as_str().unwrap();
        assert!(
            logs.contains("Authorization:***")
                && !logs.contains("abc123secret")
                && logs.contains("last")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refused_and_too_large_show_the_relay_text_and_are_not_kept() {
        let t = T::new().await;
        let h = t.signed_in().await;
        relay(
            &t,
            403,
            json!({"error": "GitHub did not let your account open an issue"}),
        )
        .await;
        let e = h.send(Command::SendFeedback(input("a"))).await.unwrap_err();
        assert_eq!(e.0, "GitHub did not let your account open an issue");
        t.server.reset().await;
        t.mock("GET", "/user", 200, json!({"login": "ann", "id": 1}))
            .await;
        relay(&t, 413, json!({"error": "request body is too large"})).await;
        let e = h.send(Command::SendFeedback(input("b"))).await.unwrap_err();
        assert_eq!(e.0, "request body is too large");
        t.mock("GET", "/api/health", 200, json!({"ok": true})).await;
        h.send(Command::RetryFeedback).await.unwrap();
        assert_eq!(
            posts(&t).await.len(),
            1,
            "nothing was queued: no retry post"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offline_or_rate_limited_reports_wait_and_go_when_the_relay_answers() {
        let t = T::new().await;
        let h = t.signed_in().await;
        relay(
            &t,
            429,
            json!({"error": "too many feedback posts, try again later"}),
        )
        .await;
        let e = h.send(Command::SendFeedback(input("Slow"))).await.unwrap();
        assert!(matches!(
            e,
            Event::Feedback {
                queued: true,
                limited: true,
                number: None,
                ..
            }
        ));
        // still unhealthy: it waits
        h.send(Command::RetryFeedback).await.unwrap();
        assert_eq!(posts(&t).await.len(), 1);
        t.server.reset().await;
        t.mock("GET", "/user", 200, json!({"login": "ann", "id": 1}))
            .await;
        t.mock("GET", "/api/health", 200, json!({"ok": true})).await;
        relay(
            &t,
            201,
            json!({"number": 9, "url": "https://example.test/i/9"}),
        )
        .await;
        h.send(Command::RetryFeedback).await.unwrap();
        assert_eq!(posts(&t).await.len(), 1, "sent once, the queue is empty");
        let s = h.snapshot();
        assert_eq!(s.unread, 1);
        assert_eq!(s.notifications[0].text, "\"Slow\" was filed as #9.");
        h.send(Command::RetryFeedback).await.unwrap();
        assert_eq!(posts(&t).await.len(), 1);

        let id = s.notifications[0].id;
        h.send(Command::MarkRead { id }).await.unwrap();
        assert_eq!(h.snapshot().unread, 0);
        assert!(h.snapshot().notifications[0].read);
        h.send(Command::MarkAllRead).await.unwrap();
        h.send(Command::ClearNotifications).await.unwrap();
        assert!(h.snapshot().notifications.is_empty());
    }
}

mod extras {
    use super::*;

    fn pack(id: &str, check: &str) -> Vec<(String, String)> {
        vec![
            (
                format!("packs/{id}/pack.json"),
                format!(
                    r#"{{"schema":1,"id":"{id}","title":"Demo {id}","summary":"A pack","setup":"setup.md","check":{{"run":{check}}}}}"#
                ),
            ),
            (
                format!("packs/{id}/setup.md"),
                "Install the thing.\n".into(),
            ),
        ]
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_pack_missing_a_field_is_named_with_the_reason_in_the_snapshot() {
        let t = T::new().await;
        let h = t.signed_in().await;
        // `setup` is required
        let bad = r#"{"schema":1,"id":"half","title":"Half","summary":"A pack","check":{"run":["true"]}}"#;
        t.serve("h1", &[("packs/half/pack.json", bad)]).await;
        t.push_tail("zed/team").await;
        h.send(Command::Join {
            repo: "zed/team".into(),
        })
        .await
        .unwrap();
        h.send(Command::SyncNow).await.unwrap();
        let p = h.snapshot().groups[0].plugin_problems.clone();
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(
            p[0].starts_with("pack half: pack.json is not valid") && p[0].contains("setup"),
            "{p:?}"
        );
        // asking for the list keeps it
        let Event::Packs(l) = h.send(Command::Packs { repo: None }).await.unwrap() else {
            panic!()
        };
        assert!(l.is_empty());
        assert_eq!(h.snapshot().groups[0].plugin_problems, p);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_pack_goes_on_only_after_its_check_passed_here() {
        let t = T::new().await;
        let h = t.signed_in().await;
        let mut files = pack("demo", r#"["true"]"#);
        files.extend(pack("bad", r#"["sh","-c","echo nope >&2; exit 3"]"#));
        files.push(("packs/wrong/pack.json".into(), "{}".into()));
        let f: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        t.serve("h1", &f).await;
        t.push_tail("zed/team").await;
        h.send(Command::Join {
            repo: "zed/team".into(),
        })
        .await
        .unwrap();
        h.send(Command::SyncNow).await.unwrap();
        let go = |c: Command| h.send(c);
        let id = |i: &str| (String::from("zed/team"), i.to_string());
        let list = || async {
            match h.send(Command::Packs { repo: None }).await.unwrap() {
                Event::Packs(p) => p
                    .into_iter()
                    .map(|p| (p.id, p.state, p.tail))
                    .collect::<Vec<_>>(),
                e => panic!("{e:?}"),
            }
        };
        assert_eq!(
            list().await,
            [
                ("bad".into(), "off".into(), "".into()),
                ("demo".into(), "off".into(), "".into())
            ]
        );
        let (repo, d) = id("demo");
        let e = go(Command::PackOn {
            repo: repo.clone(),
            id: d.clone(),
        })
        .await
        .unwrap_err();
        assert!(e.0.contains("has not passed"), "{}", e.0);
        assert_eq!(
            go(Command::PackSetup {
                repo: repo.clone(),
                id: d.clone()
            })
            .await
            .unwrap(),
            Event::Text("Install the thing.\n".into())
        );
        go(Command::PackCheck {
            repo: repo.clone(),
            id: d.clone(),
        })
        .await
        .unwrap();
        go(Command::PackOn {
            repo: repo.clone(),
            id: d.clone(),
        })
        .await
        .unwrap();
        go(Command::PackCheck {
            repo: repo.clone(),
            id: "bad".into(),
        })
        .await
        .unwrap();
        assert_eq!(
            list().await,
            [
                ("bad".into(), "failed".into(), "nope".into()),
                ("demo".into(), "on".into(), "".into())
            ]
        );
        assert!(go(Command::PackOn {
            repo: repo.clone(),
            id: "bad".into()
        })
        .await
        .is_err());
        go(Command::PackOff {
            repo: repo.clone(),
            id: d.clone(),
        })
        .await
        .unwrap();
        assert!(
            go(Command::PackOff { repo, id: d }).await.is_err(),
            "not on"
        );
    }

    /// Parity 79: a pack's files, plugin pin and CLAUDE.md follow whether the pack is on here.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pack_places_its_files_pins_its_plugin_and_imports_only_while_on() {
        let t = T::new().await;
        let h = t.signed_in().await;
        let manifest = r#"{"schema":1,"id":"ext","title":"Ext","summary":"A pack","setup":"setup.md","check":{"run":["true"]},"plugins":["p@m"],"files":["skills/x"],"claudeMd":"CLAUDE.md"}"#;
        t.serve(
            "h1",
            &[
                ("packs/ext/pack.json", manifest),
                ("packs/ext/setup.md", "Install.\n"),
                ("packs/ext/CLAUDE.md", "- pack rule\n"),
                ("skills/x/SKILL.md", "x"),
                ("skills/y/SKILL.md", "y"),
                (
                    "settings.shared.json",
                    r#"{"enabledPlugins":{"p@m":false}}"#,
                ),
            ],
        )
        .await;
        t.push_tail("zed/team").await;
        let claude = t.dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let settings = || std::fs::read_to_string(claude.join("settings.json")).unwrap();
        h.send(Command::Join {
            repo: "zed/team".into(),
        })
        .await
        .unwrap();
        h.send(Command::SyncNow).await.unwrap();
        assert!(claude.join("skills/y/SKILL.md").is_file());
        assert!(!claude.join("skills/x").exists(), "the pack is off");
        let (repo, id) = ("zed/team".to_string(), "ext".to_string());
        let go = |c: fn(String, String) -> Command| h.send(c(repo.clone(), id.clone()));
        go(|repo, id| Command::PackCheck { repo, id })
            .await
            .unwrap();
        go(|repo, id| Command::PackOn { repo, id }).await.unwrap();
        assert!(claude.join("skills/x/SKILL.md").is_file(), "placed when on");
        let md = std::fs::read_to_string(claude.join("CLAUDE.md")).unwrap();
        assert!(md.contains("packs/ext/CLAUDE.md"), "{md}");
        // the machine's own true stays while the pack is on, and is not an edit
        std::fs::write(
            claude.join("settings.json"),
            r#"{"enabledPlugins":{"p@m":true}}"#,
        )
        .unwrap();
        h.send(Command::SyncNow).await.unwrap();
        assert!(settings().contains("true"), "{}", settings());
        go(|repo, id| Command::PackOff { repo, id }).await.unwrap();
        assert!(!claude.join("skills/x").exists(), "taken away when off");
        let md = std::fs::read_to_string(claude.join("CLAUDE.md")).unwrap_or_default();
        assert!(!md.contains("packs/ext"), "{md}");
        assert!(settings().contains("false"), "{}", settings());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ssh_alias_and_activity_are_local_choices() {
        let t = T::new().await;
        let h = t.signed_in().await;
        assert!(h
            .send(Command::SshAlias {
                alias: "bad name".into()
            })
            .await
            .is_err());
        h.send(Command::SshAlias {
            alias: "laptop-1".into(),
        })
        .await
        .unwrap();
        let _ = h.send(Command::SshSet { on: true }).await; // no tailscale here: may fail, the choice stays
        assert!(h.snapshot().ssh_on);
        h.send(Command::SshSet { on: false }).await.unwrap();
        assert!(!h.snapshot().ssh_on);

        let file = claude_sync_fs::Home::under(t.dir.path())
            .state
            .join("activity.json");
        let read = || -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap()
        };
        h.send(Command::ActivityTick { idle_secs: Some(1) })
            .await
            .unwrap();
        assert!(!file.exists(), "off by default: nothing is written");
        h.send(Command::ActivitySet { on: true }).await.unwrap();
        assert!(h.snapshot().activity_on);
        h.send(Command::ActivityTick { idle_secs: Some(1) })
            .await
            .unwrap();
        assert_eq!(read()["state"], "active");
        t.clock.fetch_add(10, Ordering::SeqCst);
        h.send(Command::ActivityTick { idle_secs: Some(2) })
            .await
            .unwrap();
        assert_eq!(read()["ts"], 1_000_000, "same state, no new write");
        h.send(Command::ActivityTick {
            idle_secs: Some(500),
        })
        .await
        .unwrap();
        assert_eq!(read()["state"], "idle");
        h.send(Command::ActivityTick { idle_secs: None })
            .await
            .unwrap();
        assert_eq!(read()["state"], "unknown");
        h.send(Command::ActivitySet { on: false }).await.unwrap();
        assert!(!file.exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_update_state_comes_from_the_latest_release_only() {
        let t = T::new().await;
        let h = t.signed_in().await;
        assert_eq!(h.snapshot().update, "");
        t.mock(
            "GET",
            "/latest/download/latest.json",
            200,
            json!({"version": "999.0.0"}),
        )
        .await;
        h.send(Command::CheckUpdate).await.unwrap();
        assert_eq!(h.snapshot().update, "available:999.0.0");
        t.server.reset().await;
        t.mock("GET", "/user", 200, json!({"login": "ann", "id": 1}))
            .await;
        t.mock(
            "GET",
            "/latest/download/latest.json",
            200,
            json!({"version": "0.0.1"}),
        )
        .await;
        h.send(Command::CheckUpdate).await.unwrap();
        assert_eq!(h.snapshot().update, "");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_shell_reports_the_update_states() {
        let t = T::new().await;
        let h = t.signed_in().await;
        let set = |s: &str| h.send(Command::SetUpdate { state: s.into() });
        set("available:999.0.0").await.unwrap();
        assert_eq!(h.snapshot().update, "available:999.0.0");
        assert!(h.snapshot().update_checked.is_some());
        set("ready:999.0.0").await.unwrap();
        t.mock(
            "GET",
            "/latest/download/latest.json",
            200,
            json!({"version": "999.0.0"}),
        )
        .await;
        // the release list does not undo a verified download
        h.send(Command::CheckUpdate).await.unwrap();
        assert_eq!(h.snapshot().update, "ready:999.0.0");
        set("ready:999.0.0").await.unwrap();
        // v1: one "update ready" notice, not one per report
        let ready: Vec<_> = h
            .snapshot()
            .notifications
            .iter()
            .filter(|n| n.kind == "update")
            .map(|n| n.text.clone())
            .collect();
        assert_eq!(
            ready,
            ["claude-sync 999.0.0 is ready. It installs at the next quiet moment."]
        );
        set("failed:999.0.0").await.unwrap();
        assert_eq!(h.snapshot().update, "failed:999.0.0");
        // a ready version that is now the running one reads as updated, and an up-to-date
        // check does not clear it
        set("ready:0.0.1").await.unwrap();
        assert_eq!(h.snapshot().update, "updated:0.0.1");
        set("").await.unwrap();
        assert_eq!(h.snapshot().update, "updated:0.0.1");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_created_group_shows_whether_the_host_lock_took() {
        let t = T::new().await;
        let h = t.signed_in().await;
        t.mock("POST", "/user/repos", 201, json!({"name": "mine", "full_name": "ann/mine", "private": true, "owner": {"login": "ann"}, "default_branch": "main"})).await;
        t.mock("PUT", "/repos/ann/mine/topics", 200, json!({}))
            .await;
        t.mock("GET", "/repos/ann/mine/rulesets", 200, json!([]))
            .await;
        t.mock(
            "POST",
            "/repos/ann/mine/rulesets",
            403,
            json!({"message": "upgrade"}),
        )
        .await;
        t.push_chain("ann/mine").await;
        let e = h
            .send(Command::CreateGroup {
                name: "Mine".into(),
                repo: "ann/mine".into(),
            })
            .await;
        if let Ok(Event::Created { lock, .. }) = e {
            assert_eq!(
                h.snapshot().groups[0].host_lock.as_deref(),
                Some(lock.as_str())
            );
        }
    }
}

mod recovery_and_requests {
    use super::*;
    use wiremock::matchers::header;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_gh_token_signs_in_the_same_account_only() {
        let t = T::new().await;
        for (tok, login) in [("ghtok", "ann"), ("bobtok", "bob")] {
            Mock::given(method("GET"))
                .and(path("/user"))
                .and(header("authorization", format!("Bearer {tok}").as_str()))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"login": login, "id": 1})),
                )
                .mount(&t.server)
                .await;
        }
        let env = t.env("cid");
        let secrets = t.secrets.clone();
        tokio::task::spawn_blocking(move || {
            let mut c = Core::open(env).unwrap();
            // no sign-in: any account the token belongs to
            assert!(c.needs_sign_in());
            assert!(!c.recover_sign_in("unknown"));
            assert!(c.recover_sign_in("ghtok\n"));
            assert!(!c.needs_sign_in());
            assert_eq!(c.snapshot().login.as_deref(), Some("ann"));
            assert_eq!(
                secrets.get("ann").unwrap().unwrap(),
                claude_sync_github::Tokens::plain("ghtok").to_secret()
            );
            // signed in as ann: a token of bob is refused, nothing changes
            assert!(!c.recover_sign_in("bobtok"));
            assert_eq!(c.snapshot().login.as_deref(), Some("ann"));
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn request_counts_reach_the_db_on_exit() {
        let t = T::new().await;
        t.mock("GET", "/user/repository_invitations", 200, json!([]))
            .await;
        t.secrets
            .set("ann", &claude_sync_github::Tokens::plain("tok").to_secret())
            .unwrap();
        t.secrets.set("@current", "ann").unwrap();
        let env = t.env("cid");
        let db = t.dir.path().join(".claude-sync/claude-sync.db");
        tokio::task::spawn_blocking(move || {
            let mut c = Core::open(env).unwrap();
            c.load_invitations();
            drop(c);
            let rows = claude_sync_store::Db::open(&db)
                .unwrap()
                .requests()
                .unwrap();
            assert!(
                rows.iter().any(|r| r.caller == "invitations" && r.n >= 1),
                "{rows:?}"
            );
        })
        .await
        .unwrap();
    }
}

// ---- group plugins (Parity 26, 27, 28) ----

#[cfg(unix)]
mod plugins {
    use super::*;
    use claude_sync_actions::jobs::JobsEnv;
    use claude_sync_fs::tasks::{Kind, Out, Runner};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct NoRunner;
    impl Runner for NoRunner {
        fn run(&self, _: &str, _: &[String], _: Option<&[u8]>) -> std::io::Result<Out> {
            Ok(Out {
                ok: true,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn jobs_env(t: &T, claude: Option<PathBuf>) -> JobsEnv {
        JobsEnv {
            exe: "/app/claude-sync".into(),
            kind: Kind::Launchd {
                agents_dir: t.dir.path().join("agents"),
                uid: "501".into(),
            },
            runner: Arc::new(NoRunner),
            claude: Arc::new(move || claude.clone()),
            login_path: Arc::new(|| None),
            online: Arc::new(|| true),
            spawn: Arc::new(|_| Ok(())),
            minute: Duration::from_secs(60),
        }
    }

    /// A `claude` that logs its arguments, knows no plugin yet, and fails installs while the
    /// file `fail` exists.
    fn fake_claude(t: &T) -> (PathBuf, PathBuf) {
        let bin = t.dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = t.dir.path().join("calls.log");
        let fail = t.dir.path().join("fail");
        let claude = bin.join("claude");
        std::fs::write(
            &claude,
            format!(
                "#!/bin/sh\necho \"$@\" >> '{}'\n\
                 case \"$*\" in\n\
                 *' list --json') echo '[]';;\n\
                 *install*) if [ -e '{}' ]; then echo 'registry unreachable' >&2; exit 1; fi;;\n\
                 esac\n",
                log.display(),
                fail.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        (claude, log)
    }

    fn files(big: &str) -> Vec<(&'static str, String)> {
        vec![
            (
                "plugins/mods/.claude-plugin/marketplace.json",
                r#"{"name":"mods","plugins":[{"name":"router","source":"./router"}]}"#.into(),
            ),
            ("plugins/mods/router/a.txt", "x".into()),
            ("plugins/mods/big.bin", big.into()),
            (
                "settings.shared.json",
                r#"{"enabledPlugins":{"router@mods":true,"not a valid id":true}}"#.into(),
            ),
        ]
    }

    async fn synced(t: &T) -> Handle {
        let big = "0".repeat((1 << 20) + 1);
        let f = files(&big);
        let f: Vec<(&str, &str)> = f.iter().map(|(p, c)| (*p, c.as_str())).collect();
        let h = t.group_with("h1", &f).await;
        h.send(Command::SyncNow).await.unwrap();
        h
    }

    fn calls(log: &PathBuf) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn registers_installs_retries_and_reports_the_size_cap() {
        let t = T::new().await;
        let (claude, log) = fake_claude(&t);
        std::fs::write(t.dir.path().join("fail"), "").unwrap();
        *t.jobs.lock().unwrap() = Some(jobs_env(&t, Some(claude)));
        let h = synced(&t).await;
        until(|| h.snapshot().groups[0].plugin_problems.len() == 2).await;

        // the marketplace is registered with Claude Code before the install is tried
        let c = calls(&log);
        let add = c
            .iter()
            .position(|l| {
                l.starts_with("plugin marketplace add ")
                    && l.ends_with(".claude-sync/marketplaces/mods")
            })
            .expect("marketplace add");
        let install = c
            .iter()
            .position(|l| l == "plugin install router@mods --scope user")
            .expect("install");
        assert!(add < install, "{c:?}");
        assert!(!c.iter().any(|l| l.contains("not a valid id")), "{c:?}");

        // the failed install and the file over the cap are the group's plugin problems
        let problems = h.snapshot().groups[0].plugin_problems.clone();
        assert_eq!(
            problems,
            vec![
                "couldn't install router@mods: registry unreachable".to_string(),
                "plugins/mods/big.bin is over the 1 MiB limit and was not placed".to_string(),
            ]
        );
        let placed = t.dir.path().join(".claude-sync/marketplaces/mods");
        assert!(placed.join("router/a.txt").exists());
        assert!(
            !placed.join("big.bin").exists(),
            "the cap keeps the file out"
        );

        // a second sync inside the back-off does not run the CLI again
        let before = calls(&log).len();
        h.send(Command::SyncNow).await.unwrap();
        assert_eq!(calls(&log).len(), before);
        // v1: the placed marketplace raised one "Plugins changed" notice, not one per pass
        let n = h
            .snapshot()
            .notifications
            .iter()
            .filter(|n| n.kind == "plugins")
            .map(|n| (n.text.clone(), n.repo.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            n,
            [(
                "Plugins changed. Run /reload-plugins in open Claude Code sessions, or restart them."
                    .to_string(),
                Some("zed/team".to_string())
            )]
        );

        // Retry runs it now; the install works this time and only the cap problem is left
        std::fs::remove_file(t.dir.path().join("fail")).unwrap();
        h.send(Command::PluginsRetry {
            repo: "zed/team".into(),
        })
        .await
        .unwrap();
        until(|| h.snapshot().groups[0].plugin_problems.len() == 1).await;
        assert!(calls(&log).len() > before);
    }

    async fn until(f: impl Fn() -> bool) {
        for _ in 0..200 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("did not happen in 5 s");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_slow_claude_does_not_block_the_actor() {
        let t = T::new().await;
        let claude = t.dir.path().join("claude");
        let done = t.dir.path().join("done");
        std::fs::write(
            &claude,
            format!(
                "#!/bin/sh\nsleep 1\ntouch '{}'\ncase \"$*\" in *' list --json') echo '[]';; esac\n",
                done.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        *t.jobs.lock().unwrap() = Some(jobs_env(&t, Some(claude)));
        let h = synced(&t).await;
        // the sync answered while claude still sleeps, and the actor takes the next command
        h.send(Command::Pause).await.unwrap();
        assert!(h.snapshot().paused);
        assert!(!done.exists());
        // the task ends on its own and the actor records it
        until(|| done.exists()).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_claude_is_a_problem_and_a_group_without_plugins_has_none() {
        let t = T::new().await;
        *t.jobs.lock().unwrap() = Some(jobs_env(&t, None));
        let h = synced(&t).await;
        let problems = h.snapshot().groups[0].plugin_problems.clone();
        assert_eq!(problems[0], claude_sync_actions::msg::CLAUDE_NOT_FOUND);

        let t = T::new().await;
        let (claude, log) = fake_claude(&t);
        *t.jobs.lock().unwrap() = Some(jobs_env(&t, Some(claude)));
        let h = t
            .group_with("h1", &[("skills/s/SKILL.md", "---\nname: s\n---\n")])
            .await;
        h.send(Command::SyncNow).await.unwrap();
        assert!(h.snapshot().groups[0].plugin_problems.is_empty());
        assert!(calls(&log).is_empty(), "no work, no claude call");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_open_imports_the_v1_state_once() {
    let t = T::new().await;
    let state = t.dir.path().join(".claude-sync");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("app.json"), r#"{"paused":true}"#).unwrap();
    let env = t.env("");
    let first = tokio::task::spawn_blocking({
        let e = env.clone();
        move || Core::open(e).map(|_| ())
    });
    first.await.unwrap().unwrap();
    let db = claude_sync_store::Db::open(&state.join("claude-sync.db")).unwrap();
    let report = db.import_report().unwrap().unwrap();
    assert_eq!(report.imported, ["app.json"]);
    // a v1 file that shows up later is not read: the import ran once
    std::fs::write(state.join("jobs.json"), "{}").unwrap();
    drop(db);
    tokio::task::spawn_blocking(move || Core::open(env).map(|_| ()))
        .await
        .unwrap()
        .unwrap();
    let db = claude_sync_store::Db::open(&state.join("claude-sync.db")).unwrap();
    assert_eq!(db.import_report().unwrap().unwrap().imported, ["app.json"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_v1_daemon_that_cannot_be_found_or_stopped_skips_the_import_and_says_so() {
    let t = T::new().await;
    let state = t.dir.path().join(".claude-sync");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("app.json"), "{}").unwrap();
    let lock = std::fs::File::create(state.join("daemon.lock")).unwrap();
    lock.lock().unwrap();
    let h = t.signed_in().await;
    assert_eq!(
        h.snapshot().install,
        Some(InstallNote::Daemon {
            pid: 0,
            version: "1".into()
        })
    );
    // nothing imported, nothing written
    assert!(!state.join("daemon.owner").exists());
    let db = claude_sync_store::Db::open(&state.join("claude-sync.db")).unwrap();
    assert!(!db.has_import().unwrap());
    drop(lock);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hand_edit_shows_in_the_snapshot_and_notifies_once_without_a_sync() {
    let t = T::new().await;
    // the folder is there at start, so the watcher takes it
    std::fs::create_dir_all(t.file("skills/demo")).unwrap();
    let h = t
        .group_with("h1", &[("skills/demo/SKILL.md", "---\nname: demo\n---\n")])
        .await;
    h.send(Command::SyncNow).await.unwrap();
    assert!(h.snapshot().groups[0].edits.is_empty());
    std::fs::write(t.file("skills/demo/SKILL.md"), "mine\n").unwrap();
    let mut seen = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        seen = !h.snapshot().groups[0].edits.is_empty();
        if seen {
            break;
        }
    }
    assert!(seen, "the watcher never reported the edit");
    let s = h.snapshot();
    assert_eq!(s.groups[0].edits[0].path, "skills/demo/SKILL.md");
    let n: Vec<_> = s
        .notifications
        .iter()
        .filter(|n| n.text.starts_with("Local change to a shared file"))
        .collect();
    assert_eq!(n.len(), 1, "{:?}", s.notifications);
    assert!(n[0]
        .text
        .contains("skills/demo/SKILL.md was changed on this machine. Share it with zed/team?"));
    assert_eq!(n[0].repo.as_deref(), Some("zed/team"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_invitation_raises_one_notification_even_when_polled_again() {
    let t = T::new().await;
    let h = t.signed_in().await;
    t.mock("GET", "/user/repository_invitations", 200, json!([{"id": 7, "repository": {"name": "t", "full_name": "zed/t", "private": true}, "inviter": {"login": "zed"}}])).await;
    h.send(Command::WindowOpened).await.unwrap();
    h.send(Command::WindowOpened).await.unwrap();
    let n: Vec<_> = h
        .snapshot()
        .notifications
        .into_iter()
        .filter(|n| n.text.starts_with("Invitation to a claude-sync group"))
        .collect();
    assert_eq!(n.len(), 1);
    assert_eq!(
        n[0].text,
        "Invitation to a claude-sync group: zed invited you to zed/t. Open claude-sync to accept."
    );
    assert_eq!(n[0].repo.as_deref(), Some("zed/t"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_name_follows_the_v1_rule() {
    let t = T::new().await;
    let h = t.signed_in().await;
    let id = "d1".to_string();
    for bad in ["", "  ", "a/b", "a\\b", "a:b", "me@pc", &"x".repeat(65)] {
        let r = h
            .send(Command::DeviceRename {
                id: id.clone(),
                name: bad.into(),
            })
            .await;
        assert_eq!(
            r.unwrap_err().to_string(),
            "device names can't contain / \\ : @ and are at most 64 characters",
            "{bad:?}"
        );
    }
    h.send(Command::DeviceRename {
        id,
        name: "x".repeat(64),
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_snapshot_carries_the_github_app_slug_for_the_install_page() {
    let t = T::new().await;
    let h = t.signed_in().await;
    assert_eq!(h.snapshot().app_slug, claude_sync_github::APP_SLUG);
    assert!(!h.snapshot().app_slug.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_leftover_claude_md_copy_is_backed_up_and_replaced_at_first_sight() {
    let t = T::new().await;
    let h = t.group_with("h1", &[("CLAUDE.md", "- A\n")]).await;
    let dir = t.dir.path().join(".claude-sync/groups/zed__team");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("CLAUDE.md"), "- stale\n").unwrap();
    h.send(Command::SyncNow).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
        "- A\n"
    );
    assert!(backed_up(&t, "- stale"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_review_state_is_saved_only_after_the_copy_is_written() {
    let t = T::new().await;
    let h = t.group_with("h1", &[("CLAUDE.md", "- A\n")]).await;
    // a directory where the copy goes makes the write fail
    let copy = t.dir.path().join(".claude-sync/groups/zed__team/CLAUDE.md");
    std::fs::create_dir_all(&copy).unwrap();
    let _ = h.send(Command::SyncNow).await;
    let db =
        claude_sync_store::Db::open(&t.dir.path().join(".claude-sync/claude-sync.db")).unwrap();
    assert!(
        claude_sync_engine::traits::Store::read(&db, "review/zed/team")
            .unwrap()
            .is_none_or(|b| b.is_empty())
    );
}
