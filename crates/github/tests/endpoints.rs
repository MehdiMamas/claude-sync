//! One test per row of v2-Endpoints (rowNN = the row's order on the frame), plus the error
//! answers the frame lists for it: 304, 401, 403 rate limit, 422, 5xx are in `pipeline.rs`.
mod common;
use claude_sync_github::{api, device, Error};
use common::*;
use serde_json::json;
use wiremock::matchers::{
    body_partial_json, body_string_contains, header, method, path, query_param,
};
use wiremock::{Mock, ResponseTemplate};

// ---- sign in -------------------------------------------------------------------------

#[test]
fn row01_device_code_start() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/login/device/code"))
            .and(header("accept", "application/json"))
            .and(body_string_contains("client_id=Iv1.test"))
            .and(body_string_contains("scope=repo+read%3Auser"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code": "dc", "user_code": "ABCD-1234",
                "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 5})))
            .expect(1),
    );
    let c = device::start(&s.client()).unwrap();
    assert_eq!(
        (c.user_code.as_str(), c.interval, c.device_code.as_str()),
        ("ABCD-1234", 5, "dc")
    );
}

fn poll_reply(body: serde_json::Value) -> Result<device::DevicePoll, Error> {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .and(body_string_contains("client_id=Iv1.test"))
            .and(body_string_contains("device_code=dc"))
            .and(body_string_contains(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(body)),
    );
    device::poll(&s.client(), "dc")
}

#[test]
fn row02_device_poll_states() {
    use device::DevicePoll::*;
    assert_eq!(
        poll_reply(json!({"error": "authorization_pending"})).unwrap(),
        Pending
    );
    assert_eq!(
        poll_reply(json!({"error": "slow_down", "interval": 10})).unwrap(),
        SlowDown(10)
    );
    assert_eq!(
        poll_reply(json!({"error": "slow_down"})).unwrap(),
        SlowDown(0)
    );
    assert_eq!(
        poll_reply(json!({"error": "expired_token"})).unwrap(),
        Expired
    );
    assert_eq!(
        poll_reply(json!({"error": "access_denied"})).unwrap(),
        Denied
    );
    match poll_reply(json!({"access_token": "gho_x", "token_type": "bearer"})).unwrap() {
        Token(t) => assert_eq!((t.access_token.as_str(), t.refresh_token), ("gho_x", None)),
        other => panic!("{other:?}"),
    }
    let e = poll_reply(json!({"error": "incorrect_client_credentials"})).unwrap_err();
    assert!(
        matches!(&e, Error::Github { status: 400, message } if message.contains("incorrect_client_credentials"))
    );
    let e = poll_reply(json!({})).unwrap_err();
    assert_eq!(e.to_string(), "GitHub sent no token and no error");
}

fn code(interval: u64, expires_in: u64) -> device::DeviceCode {
    serde_json::from_value(
        json!({"device_code": "dc", "user_code": "U", "verification_uri": "v",
        "expires_in": expires_in, "interval": interval}),
    )
    .unwrap()
}

fn replies(s: &Srv, list: &[serde_json::Value]) {
    for r in list {
        s.mount(
            Mock::given(method("POST"))
                .and(path("/login/oauth/access_token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(r.clone()))
                .up_to_n_times(1),
        );
    }
}

#[test]
fn row02_device_wait_honours_interval_and_slow_down() {
    let s = Srv::new();
    replies(
        &s,
        &[
            json!({"error": "authorization_pending"}),
            json!({"error": "slow_down"}),
            json!({"error": "slow_down", "interval": 20}),
            json!({"access_token": "gho_x"}),
        ],
    );
    let mut sleeps = vec![];
    let r = device::wait(&s.client(), &code(5, 900), |d| sleeps.push(d.as_secs())).unwrap();
    assert!(matches!(r, device::DevicePoll::Token(_)));
    // slow_down without a number adds 5 s; with one it takes GitHub's.
    assert_eq!(sleeps, [5, 5, 10, 20]);
}

#[test]
fn row02_device_wait_ends_on_expiry_and_denial() {
    let s = Srv::new();
    s.mount(Mock::given(path("/login/oauth/access_token")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!({"error": "authorization_pending"})),
    ));
    let mut sleeps = vec![];
    let r = device::wait(&s.client(), &code(5, 12), |d| sleeps.push(d.as_secs())).unwrap();
    assert_eq!((r, sleeps), (device::DevicePoll::Expired, vec![5, 5, 5]));

    let s = Srv::new();
    replies(&s, &[json!({"error": "access_denied"})]);
    let r = device::wait(&s.client(), &code(5, 900), |_| {}).unwrap();
    assert_eq!(r, device::DevicePoll::Denied);
    // A GitHub-side expired code ends the wait too.
    let s = Srv::new();
    replies(&s, &[json!({"error": "expired_token"})]);
    assert_eq!(
        device::wait(&s.client(), &code(5, 900), |_| {}).unwrap(),
        device::DevicePoll::Expired
    );
}

#[test]
fn row03_user() {
    let s = Srv::new();
    s.once("GET", "/user", 200, json!({"login": "octo", "id": 7}));
    let u = s.client().user().unwrap();
    assert_eq!((u.login.as_str(), u.id), ("octo", 7));
}

// ---- reading the repo ----------------------------------------------------------------

#[test]
fn row04_head_conditional() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/repos/o/r/commits/main"))
            .and(header("if-none-match", "\"e1\""))
            .respond_with(ResponseTemplate::new(304))
            .with_priority(1)
            .expect(1),
    );
    s.mount(
        Mock::given(method("GET"))
            .and(path("/repos/o/r/commits/main"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "\"e1\"")
                    .set_body_json(json!({"sha": "c1"})),
            )
            .expect(1),
    );
    let c = s.client();
    let h = c.head("o", "r", "main", None).unwrap().unwrap();
    assert_eq!((h.sha.as_str(), h.etag.as_deref()), ("c1", Some("\"e1\"")));
    assert_eq!(c.head("o", "r", "main", h.etag.as_deref()).unwrap(), None);
}

#[test]
fn row05_tree_recursive() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/repos/o/r/git/trees/c1"))
            .and(query_param("recursive", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"sha": "t1", "truncated": false,
                "tree": [{"path": "a.md", "mode": "100644", "type": "blob", "sha": "b1"}, {"path": "d", "mode": "040000", "type": "tree", "sha": "t2"}]})))
            .expect(1),
    );
    let t = s.client().tree("o", "r", "c1", true).unwrap();
    assert_eq!(
        (t.sha.as_str(), t.tree.len(), t.tree[0].kind.as_str()),
        ("t1", 2, "blob")
    );
}

#[test]
fn row06_blob_decodes_wrapped_base64() {
    let s = Srv::new();
    s.once(
        "GET",
        "/repos/o/r/git/blobs/b1",
        200,
        json!({"content": "aGVs\nbG8=\n", "encoding": "base64"}),
    );
    assert_eq!(s.client().blob("o", "r", "b1").unwrap(), b"hello");
}

// ---- repos and rulesets --------------------------------------------------------------

#[test]
fn row07_create_repo_private_with_first_commit() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/user/repos"))
            .and(body_partial_json(
                json!({"name": "claude-sync-data", "private": true, "auto_init": true}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(repo_json("claude-sync-data")))
            .expect(1),
    );
    let r = s.client().create_repo("claude-sync-data", "d").unwrap();
    assert_eq!(r.full_name, "o/claude-sync-data");
}

#[test]
fn row07_create_repo_refuses_public_and_reports_422() {
    let s = Srv::new();
    s.once(
        "POST",
        "/user/repos",
        201,
        json!({"name": "x", "full_name": "o/x", "private": false}),
    );
    let e = s.client().create_repo("x", "d").unwrap_err();
    assert_eq!(
        e.to_string(),
        "repo o/x is public; claude-sync only uses private repos"
    );
    let s = Srv::new();
    s.once("POST", "/user/repos", 422, json!({"message": "Repository creation failed.",
        "errors": [{"resource": "Repository", "code": "custom", "field": "name", "message": "name already exists on this account"}]}));
    let e = s.client().create_repo("x", "d").unwrap_err();
    assert_eq!(e.status(), Some(422));
    assert!(e
        .to_string()
        .contains("name already exists on this account"));
}

#[test]
fn row07_topic_marks_a_group() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("PUT"))
            .and(path("/repos/o/r/topics"))
            .and(body_partial_json(json!({"names": [api::TOPIC]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"names": [api::TOPIC]})))
            .expect(1),
    );
    s.client().set_topics("o", "r", &[api::TOPIC]).unwrap();
}

#[test]
fn row08_ruleset_only_admins_push() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/rulesets"))
            .and(body_partial_json(
                json!({"name": api::HOST_LOCK_NAME, "enforcement": "active",
                "rules": [{"type": "update"}, {"type": "deletion"}, {"type": "non_fast_forward"}]}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 1})))
            .expect(1),
    );
    s.client().create_ruleset("o", "r").unwrap();
    // A non-admin (or a plan without rulesets) is refused; that is an error, not a rate limit.
    let s = Srv::new();
    s.once(
        "POST",
        "/repos/o/r/rulesets",
        403,
        json!({"message": "Must have admin rights to Repository."}),
    );
    assert_eq!(
        s.client().create_ruleset("o", "r").unwrap_err().status(),
        Some(403)
    );
}

// ---- people --------------------------------------------------------------------------

#[test]
fn row09_invite_with_read_permission() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("PUT"))
            .and(path("/repos/o/r/collaborators/friend"))
            .and(body_partial_json(json!({"permission": "pull"})))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 5})))
            .expect(1),
    );
    s.client().invite("o", "r", "friend").unwrap();
}

#[test]
fn row09_invite_retries_once_without_permission_on_personal_repos() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("PUT"))
            .and(path("/repos/o/r/collaborators/friend"))
            .and(body_partial_json(json!({"permission": "pull"})))
            .respond_with(
                ResponseTemplate::new(422).set_body_json(json!({"message": "Validation Failed",
                "errors": ["Cannot assign Q permission of read"]})),
            )
            .expect(1),
    );
    s.mount(
        Mock::given(method("PUT"))
            .and(path("/repos/o/r/collaborators/friend"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 5})))
            .expect(1)
            .with_priority(5),
    );
    s.client().invite("o", "r", "friend").unwrap();
    let reqs = s.requests();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[1].body.is_empty());
    // Any other 422 is not retried.
    let s = Srv::new();
    s.once(
        "PUT",
        "/repos/o/r/collaborators/o",
        422,
        json!({"message": "Repository owner cannot be a collaborator"}),
    );
    let e = s.client().invite("o", "r", "o").unwrap_err();
    assert!(e
        .to_string()
        .contains("Repository owner cannot be a collaborator"));
}

#[test]
fn row10_user_invitations_all_pages() {
    let s = Srv::new();
    let next = format!(
        "<{}/user/repository_invitations?per_page=100&page=2>; rel=\"next\"",
        s.uri()
    );
    s.mount(
        Mock::given(method("GET"))
            .and(path("/user/repository_invitations"))
            .and(query_param("page", "2"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"id": 2, "repository": repo_json("b")}])),
            )
            .expect(1),
    );
    s.mount(
        Mock::given(method("GET"))
            .and(path("/user/repository_invitations"))
            .and(query_param("per_page", "100"))
            .respond_with(ResponseTemplate::new(200).insert_header("link", next.as_str())
                .set_body_json(json!([{"id": 1, "repository": repo_json("a"), "inviter": {"login": "host"}}])))
            .expect(1)
            .with_priority(5),
    );
    let v = s.client().invitations().unwrap();
    assert_eq!(v.iter().map(|i| i.id).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(v[0].inviter.as_ref().unwrap().login, "host");
}

#[test]
fn row11_accept_invitation() {
    let s = Srv::new();
    s.once_empty("PATCH", "/user/repository_invitations/4");
    s.client().accept_invitation(4).unwrap();
}

// ---- pushing -------------------------------------------------------------------------

#[test]
fn row12_create_commit() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/commits"))
            .and(body_partial_json(
                json!({"tree": "t2", "parents": ["c1"], "message": "Share"}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "c2"})))
            .expect(1),
    );
    assert_eq!(
        s.client()
            .create_commit("o", "r", "Share", "t2", "c1")
            .unwrap(),
        "c2"
    );
}

#[test]
fn row13_update_ref_never_forces_and_maps_a_lost_race() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("PATCH"))
            .and(path("/repos/o/r/git/refs/heads/main"))
            .and(body_partial_json(json!({"sha": "c2", "force": false})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ref": "refs/heads/main"})),
            )
            .expect(1),
    );
    s.client().update_ref("o", "r", "main", "c2").unwrap();
    let s = Srv::new();
    s.once(
        "PATCH",
        "/repos/o/r/git/refs/heads/main",
        422,
        json!({"message": "Update is not a fast forward"}),
    );
    assert_eq!(
        s.client().update_ref("o", "r", "main", "c2").unwrap_err(),
        Error::PushRace
    );
    // Another 422 is a plain error.
    let s = Srv::new();
    s.once(
        "PATCH",
        "/repos/o/r/git/refs/heads/main",
        422,
        json!({"message": "Object does not exist"}),
    );
    assert_eq!(
        s.client()
            .update_ref("o", "r", "main", "c2")
            .unwrap_err()
            .status(),
        Some(422)
    );
}

#[test]
fn row14_create_pull() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/pulls"))
            .and(body_partial_json(
                json!({"head": "proposal/me-20260101000000", "base": "main", "title": "Add x"}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 7})))
            .expect(1),
    );
    assert_eq!(
        s.client()
            .create_pull("o", "r", "Add x", "proposal/me-20260101000000", "main")
            .unwrap()
            .number,
        7
    );
}

#[test]
fn row15_merge_pull_pinned_to_the_head_read() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("PUT"))
            .and(path("/repos/o/r/pulls/7/merge"))
            .and(body_partial_json(json!({"sha": "h1"})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"sha": "m1", "merged": true})),
            )
            .expect(1),
    );
    assert_eq!(
        s.client().merge_pull("o", "r", 7, Some("h1")).unwrap().sha,
        "m1"
    );
    let s = Srv::new();
    s.once(
        "PUT",
        "/repos/o/r/pulls/7/merge",
        409,
        json!({"message": "Head branch was modified."}),
    );
    assert_eq!(
        s.client()
            .merge_pull("o", "r", 7, Some("h1"))
            .unwrap_err()
            .status(),
        Some(409)
    );
}

#[test]
fn row16_get_repo_follows_a_rename() {
    let s = Srv::new();
    s.once("GET", "/repos/o/r", 200, repo_json("r"));
    let r = s.client().repo("o", "r").unwrap();
    assert_eq!((r.private, r.permissions.unwrap().admin), (true, true));
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/repos/o/old"))
            .respond_with(
                ResponseTemplate::new(301)
                    .insert_header("location", format!("{}/repos/o/new", s.uri())),
            )
            .expect(1),
    );
    s.once("GET", "/repos/o/new", 200, repo_json("new"));
    assert_eq!(s.client().repo("o", "old").unwrap().full_name, "o/new");
}

// ---- releases and relay --------------------------------------------------------------

#[test]
fn row17_latest_json() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/releases/latest/download/latest.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"version": "2.0.0", "notes": "n",
                "platforms": {"darwin-aarch64": {"url": "https://x/a.tar.gz", "signature": "sig"}}})))
            .expect(1),
    );
    // No token is needed or sent.
    let c = claude_sync_github::Client::new(s.config()).unwrap();
    let l = c.latest().unwrap();
    assert_eq!(
        (
            l.version.as_str(),
            l.platforms["darwin-aarch64"].signature.as_str()
        ),
        ("2.0.0", "sig")
    );
    assert!(s.requests()[0].headers.get("authorization").is_none());
}

#[test]
fn a_version_that_is_not_digits_and_dots_never_reaches_a_url() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/releases/latest/download/latest.json"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"version": "2.0.0/../../x"})),
            ),
    );
    let c = s.client();
    assert!(c.latest().is_err());
    assert!(c.release_asset("1.2.3/../x", "a.tar.gz").is_err());
    assert!(c.release_asset("1.2", "a.tar.gz").is_err());
    assert!(s.requests().len() == 1, "only latest.json was asked for");
}

#[test]
fn row18_release_page_is_a_link_for_the_browser() {
    let s = Srv::new();
    assert_eq!(
        s.client().release_page_url(),
        format!("{}/releases/latest", s.uri())
    );
    assert!(claude_sync_github::Config::github()
        .releases
        .ends_with("/MehdiMamas/claude-sync/releases"));
    assert!(s.requests().is_empty());
}

#[test]
fn row19_feedback_with_the_user_token() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/api/feedback"))
            .and(header("authorization", "Bearer tok"))
            .and(body_partial_json(json!({"title": "t"})))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({"number": 12, "url": "https://x/12"})),
            )
            .expect(1),
    );
    assert_eq!(
        s.client()
            .feedback(&json!({"title": "t", "body": "b"}))
            .unwrap()
            .number,
        12
    );
    // The relay's own words are kept (a refusal is shown as it is).
    let s = Srv::new();
    s.once(
        "POST",
        "/api/feedback",
        403,
        json!({"error": "GitHub refused your account"}),
    );
    let e = s.client().feedback(&json!({})).unwrap_err();
    assert_eq!(
        e,
        Error::Github {
            status: 403,
            message: "GitHub refused your account".into()
        }
    );
    let s = Srv::new();
    s.once("POST", "/api/feedback", 502, json!({"x": 1}));
    assert!(s
        .client()
        .feedback(&json!({}))
        .unwrap_err()
        .to_string()
        .starts_with("GitHub returned 502: relay returned 502"));
}

#[test]
fn row20_health() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/api/health"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok": true, "version": "2.0.0"})),
            )
            .expect(1),
    );
    let h = claude_sync_github::Client::new(s.config())
        .unwrap()
        .health()
        .unwrap();
    assert!(h.ok);
}

#[test]
fn row21_refresh_token() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=r1"))
            .and(body_string_contains("client_id=Iv1.test"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"access_token": "a2", "refresh_token": "r2",
                "expires_in": 28800, "refresh_token_expires_in": 15811200})),
            )
            .expect(1),
    );
    let t = device::refresh(&s.client(), "r1").unwrap();
    assert_eq!(
        (t.access_token.as_str(), t.refresh_token.as_deref()),
        ("a2", Some("r2"))
    );
    assert!(t.expires_at.unwrap() > t.refresh_token_expires_at.unwrap() - 15811200);
}

#[test]
fn row22_installations() {
    let s = Srv::new();
    s.once(
        "GET",
        "/user/installations",
        200,
        json!({"total_count": 1, "installations": [{"id": 9}]}),
    );
    assert_eq!(s.client().installations().unwrap()[0].id, 9);
}

#[test]
fn row23_installation_repositories() {
    let s = Srv::new();
    s.once(
        "GET",
        "/user/installations/9/repositories",
        200,
        json!({"total_count": 1, "repositories": [repo_json("a")]}),
    );
    assert_eq!(s.client().installation_repos(9).unwrap()[0].name, "a");
}

#[test]
fn row24_user_repos_is_conditional() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .and(query_param("affiliation", "owner,collaborator"))
            .and(query_param("per_page", "100"))
            .and(header("if-none-match", "\"u1\""))
            .respond_with(ResponseTemplate::new(304))
            .with_priority(1)
            .expect(1),
    );
    s.mount(
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "\"u1\"")
                    .set_body_json(json!([repo_json("a")])),
            )
            .expect(1),
    );
    let c = s.client();
    assert_eq!(c.user_repos().unwrap().len(), 1);
    assert_eq!(c.user_repos().unwrap().len(), 1); // 304, served from the cache
}

#[test]
fn row25_topics() {
    let s = Srv::new();
    s.once(
        "GET",
        "/repos/o/r/topics",
        200,
        json!({"names": ["claude-sync-group"]}),
    );
    assert_eq!(s.client().topics("o", "r").unwrap(), [api::TOPIC]);
}

// ---- proposals -----------------------------------------------------------------------

#[test]
fn row26_open_pulls_304_is_free() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls"))
            .and(query_param("state", "open"))
            .and(header("if-none-match", "\"p1\""))
            .respond_with(ResponseTemplate::new(304))
            .with_priority(1)
            .expect(1),
    );
    s.mount(
        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls"))
            .respond_with(ResponseTemplate::new(200).insert_header("etag", "\"p1\"")
                .set_body_json(json!([{"number": 3, "title": "t", "state": "open", "head": {"sha": "h", "ref": "proposal/a-1"}, "user": {"login": "a"}}])))
            .expect(1),
    );
    let c = s.client();
    assert_eq!(
        c.open_pulls("o", "r").unwrap()[0]
            .head
            .as_ref()
            .unwrap()
            .name,
        "proposal/a-1"
    );
    assert_eq!(c.open_pulls("o", "r").unwrap()[0].number, 3);
}

#[test]
fn row27_reviews() {
    let s = Srv::new();
    s.once(
        "GET",
        "/repos/o/r/pulls/3/reviews",
        200,
        json!([{"user": {"login": "b"}, "state": "APPROVED"}]),
    );
    assert_eq!(
        s.client().reviews("o", "r", 3).unwrap()[0].state,
        "APPROVED"
    );
}

#[test]
fn row28_approve() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/pulls/3/reviews"))
            .and(body_partial_json(json!({"event": "APPROVE"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 1})))
            .expect(1),
    );
    s.client().approve("o", "r", 3).unwrap();
}

#[test]
fn row29_close_pull() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("PATCH"))
            .and(path("/repos/o/r/pulls/3"))
            .and(body_partial_json(json!({"state": "closed"})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"number": 3, "state": "closed"})),
            )
            .expect(1),
    );
    assert_eq!(s.client().close_pull("o", "r", 3).unwrap().state, "closed");
}

#[test]
fn row30_create_ref() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/refs"))
            .and(body_partial_json(
                json!({"ref": "refs/heads/proposal/me-20260101000000", "sha": "c1"}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"ref": "x"})))
            .expect(1),
    );
    s.client()
        .create_ref("o", "r", "refs/heads/proposal/me-20260101000000", "c1")
        .unwrap();
    // An existing branch name fails with 422, not a race.
    let s = Srv::new();
    s.once(
        "POST",
        "/repos/o/r/git/refs",
        422,
        json!({"message": "Reference already exists"}),
    );
    assert_eq!(
        s.client()
            .create_ref("o", "r", "refs/heads/a", "c1")
            .unwrap_err()
            .status(),
        Some(422)
    );
}

#[test]
fn row31_create_blob_base64() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/blobs"))
            .and(body_partial_json(
                json!({"content": "aGk=", "encoding": "base64"}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "b9"})))
            .expect(1),
    );
    assert_eq!(s.client().create_blob("o", "r", b"hi").unwrap(), "b9");
}

#[test]
fn row32_create_tree_on_a_base() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/trees"))
            .and(body_partial_json(json!({"base_tree": "t1", "tree": [{"path": "CLAUDE.md", "sha": "b9"}, {"path": "old.md", "sha": null}]})))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "t2"})))
            .expect(1),
    );
    let e = [
        json!({"path": "CLAUDE.md", "mode": "100644", "type": "blob", "sha": "b9"}),
        json!({"path": "old.md", "mode": "100644", "type": "blob", "sha": null}),
    ];
    assert_eq!(s.client().create_tree("o", "r", "t1", &e).unwrap(), "t2");
}

#[test]
fn row33_commits_of_main() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/repos/o/r/commits"))
            .and(query_param("sha", "main"))
            .and(query_param("per_page", "30"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"sha": "c1", "commit": {"message": "m", "author": {"name": "A", "date": "2026-01-01T00:00:00Z"}}, "author": {"login": "a"}}])))
            .expect(1),
    );
    let v = s.client().commits("o", "r", "main", 30).unwrap();
    assert_eq!(
        (
            v[0].commit.message.as_str(),
            v[0].author.as_ref().unwrap().login.as_str()
        ),
        ("m", "a")
    );
}

#[test]
fn row34_compare() {
    let s = Srv::new();
    s.once(
        "GET",
        "/repos/o/r/compare/a1...b2",
        200,
        json!({"files": [{"filename": "x.md", "status": "modified", "patch": "@@"}]}),
    );
    assert_eq!(
        s.client().compare("o", "r", "a1", "b2").unwrap().files[0].filename,
        "x.md"
    );
}

#[test]
fn row35_collaborators() {
    let s = Srv::new();
    s.once("GET", "/repos/o/r/collaborators", 200, json!([{"login": "a", "permissions": {"admin": true, "push": true, "pull": true}}, {"login": "b"}]));
    let v = s.client().collaborators("o", "r").unwrap();
    assert_eq!(
        (v.len(), v[0].permissions.as_ref().unwrap().admin),
        (2, true)
    );
}

#[test]
fn row36_repo_invitations() {
    let s = Srv::new();
    s.once(
        "GET",
        "/repos/o/r/invitations",
        200,
        json!([{"id": 4, "invitee": {"login": "friend"}}]),
    );
    assert_eq!(
        s.client().repo_invitations("o", "r").unwrap()[0]
            .invitee
            .as_ref()
            .unwrap()
            .login,
        "friend"
    );
}

#[test]
fn row37_cancel_invitation() {
    let s = Srv::new();
    s.once_empty("DELETE", "/repos/o/r/invitations/4");
    s.client().cancel_invitation("o", "r", 4).unwrap();
}

#[test]
fn row38_remove_collaborator() {
    let s = Srv::new();
    s.once_empty("DELETE", "/repos/o/r/collaborators/friend");
    s.client().remove_collaborator("o", "r", "friend").unwrap();
}

#[test]
fn row39_leave_removes_your_own_login() {
    let s = Srv::new();
    s.once_empty("DELETE", "/repos/o/r/collaborators/me");
    s.client().remove_collaborator("o", "r", "me").unwrap();
}

#[test]
fn row40_transfer() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/transfer"))
            .and(body_partial_json(json!({"new_owner": "new"})))
            .respond_with(
                ResponseTemplate::new(202).set_body_json(json!({"name": "r", "full_name": "o/r"})),
            )
            .expect(1),
    );
    s.client().transfer("o", "r", "new").unwrap();
}

#[test]
fn device_wait_stops_when_cancelled() {
    let s = Srv::new();
    let code = device::DeviceCode {
        device_code: "dc".into(),
        user_code: "X".into(),
        verification_uri: "u".into(),
        expires_in: 900,
        interval: 5,
    };
    let r = device::wait_with(&s.client(), &code, |_| {}, || true).unwrap();
    assert_eq!(r, device::DevicePoll::Cancelled);
    assert!(s.requests().is_empty());
}
