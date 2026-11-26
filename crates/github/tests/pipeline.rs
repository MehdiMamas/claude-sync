//! The shared request pipeline: ETag cache, 401, rate limits per resource, 422 text,
//! 5xx retry and backoff, request counts, token refresh.
mod common;
use claude_sync_github::{with_caller, Client, Error, Tokens};
use common::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn user_ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"login": "o", "id": 1}))
}

#[test]
fn a_304_returns_the_cached_body_and_is_counted() {
    let s = Srv::new();
    s.mount(
        Mock::given(path("/user"))
            .and(header("if-none-match", "\"v1\""))
            .respond_with(ResponseTemplate::new(304))
            .with_priority(1)
            .expect(1),
    );
    s.mount(
        Mock::given(path("/user"))
            .respond_with(user_ok().insert_header("etag", "\"v1\""))
            .expect(1),
    );
    let c = s.client();
    assert_eq!(c.user().unwrap().login, "o");
    assert_eq!(c.user().unwrap().login, "o");
    let r: Vec<_> = c
        .counts()
        .iter()
        .map(|x| (x.endpoint.clone(), x.result.clone()))
        .collect();
    assert!(
        r.contains(&("GET user".into(), "200".into()))
            && r.contains(&("GET user".into(), "304".into()))
    );
}

#[test]
fn only_gets_with_an_etag_are_cached() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(user_ok())
            .expect(2),
    );
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/blobs"))
            .respond_with(
                ResponseTemplate::new(201)
                    .insert_header("etag", "\"x\"")
                    .set_body_json(json!({"sha": "b"})),
            )
            .expect(2),
    );
    let c = s.client();
    c.user().unwrap();
    c.user().unwrap();
    c.create_blob("o", "r", b"a").unwrap();
    c.create_blob("o", "r", b"a").unwrap();
    assert!(s
        .requests()
        .iter()
        .all(|r| r.headers.get("if-none-match").is_none()));
}

#[test]
fn a_confirmed_401_signs_out_and_stops_all_calls() {
    let s = Srv::new();
    s.mount(
        Mock::given(path("/repos/o/r"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"message": "Bad credentials"})),
            )
            .expect(1),
    );
    s.mount(
        Mock::given(path("/user"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"message": "Bad credentials"})),
            )
            .expect(1),
    );
    let c = s.client();
    assert_eq!(c.repo("o", "r").unwrap_err(), Error::SignInExpired);
    assert!(c.is_expired());
    assert_eq!(c.user().unwrap_err(), Error::SignInExpired);
    assert_eq!(
        c.repo("o", "r").unwrap_err().to_string(),
        "Your GitHub sign-in expired. Sign in again."
    );
    assert_eq!(s.requests().len(), 2);
    // Signing in again clears it.
    c.set_tokens(Tokens::plain("tok2"));
    assert!(!c.is_expired());
}

#[test]
fn one_odd_401_on_one_repo_does_not_sign_out() {
    let s = Srv::new();
    s.mount(
        Mock::given(path("/repos/o/r"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({"message": "odd"}))),
    );
    s.mount(Mock::given(path("/user")).respond_with(user_ok()));
    let c = s.client();
    assert_eq!(
        c.repo("o", "r").unwrap_err(),
        Error::Github {
            status: 401,
            message: "odd".into()
        }
    );
    assert!(!c.is_expired());
    assert!(c.user().is_ok());
}

#[test]
fn a_401_on_user_needs_no_second_call() {
    let s = Srv::new();
    s.mount(
        Mock::given(path("/user"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1),
    );
    assert_eq!(s.client().user().unwrap_err(), Error::SignInExpired);
}

#[test]
fn no_tokens_means_not_signed_in_and_no_request() {
    let s = Srv::new();
    let c = Client::new(s.config()).unwrap();
    assert_eq!(c.user().unwrap_err(), Error::NotSignedIn);
    assert!(s.requests().is_empty());
}

#[test]
fn a_primary_rate_limit_waits_for_the_reset_without_sending() {
    let s = Srv::new();
    let reset = now() + 3600;
    s.mount(
        Mock::given(path("/user"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-remaining", "0")
                    .insert_header("x-ratelimit-reset", reset.to_string().as_str())
                    .set_body_json(json!({"message": "API rate limit exceeded"})),
            )
            .expect(1),
    );
    let c = s.client();
    assert_eq!(
        c.user().unwrap_err(),
        Error::RateLimited { reset_unix: reset }
    );
    assert_eq!(c.limited_until(), Some(reset));
    // Neither a GET nor a POST goes out while limited.
    assert!(matches!(c.repo("o", "r"), Err(Error::RateLimited { .. })));
    assert!(matches!(
        c.create_blob("o", "r", b"x"),
        Err(Error::RateLimited { .. })
    ));
    assert!(c
        .user()
        .unwrap_err()
        .to_string()
        .starts_with("GitHub limit reached, resumes at "));
}

#[test]
fn secondary_limit_and_retry_after() {
    let s = Srv::new();
    s.mount(
        Mock::given(path("/user")).respond_with(ResponseTemplate::new(403).set_body_json(
            json!({"message": "You have exceeded a secondary rate limit. Please wait."}),
        )),
    );
    let c = s.client();
    c.user().unwrap_err();
    let t = c.limited_until().unwrap();
    assert!((now() + 58..=now() + 62).contains(&t));

    let s = Srv::new();
    s.mount(
        Mock::given(path("/user"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "120")),
    );
    let c = s.client();
    c.user().unwrap_err();
    assert!((now() + 118..=now() + 122).contains(&c.limited_until().unwrap()));
}

#[test]
fn a_plain_403_is_an_error_not_a_limit() {
    let s = Srv::new();
    s.mount(Mock::given(path("/user")).respond_with(
        ResponseTemplate::new(403).set_body_json(json!({"message": "Resource not accessible"})),
    ));
    let c = s.client();
    assert_eq!(
        c.user().unwrap_err(),
        Error::Github {
            status: 403,
            message: "Resource not accessible".into()
        }
    );
    assert_eq!(c.limited_until(), None);
}

#[test]
fn a_200_with_none_left_limits_the_next_call() {
    let s = Srv::new();
    let reset = now() + 600;
    s.mount(
        Mock::given(path("/user"))
            .respond_with(
                user_ok()
                    .insert_header("x-ratelimit-remaining", "0")
                    .insert_header("x-ratelimit-reset", reset.to_string().as_str()),
            )
            .expect(1),
    );
    let c = s.client();
    c.user().unwrap();
    assert_eq!(c.limited_until(), Some(reset));
    assert!(matches!(c.user(), Err(Error::RateLimited { .. })));
}

#[test]
fn graphql_and_core_limits_are_kept_apart() {
    let s = Srv::new();
    let reset = now() + 600;
    s.mount(
        Mock::given(path("/repos/o/r")).respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-resource", "graphql")
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset", reset.to_string().as_str()),
        ),
    );
    s.mount(Mock::given(path("/user")).respond_with(user_ok()));
    let c = s.client();
    assert!(matches!(c.repo("o", "r"), Err(Error::RateLimited { .. })));
    assert_eq!(
        (c.graphql_limited_until(), c.limited_until()),
        (Some(reset), None)
    );
    assert!(c.user().is_ok());
}

#[test]
fn a_422_keeps_its_details() {
    let s = Srv::new();
    s.once(
        "POST",
        "/repos/o/r/git/refs",
        422,
        json!({"message": "Validation Failed", "errors": ["Reference already exists"]}),
    );
    let e = s
        .client()
        .create_ref("o", "r", "refs/heads/a", "c")
        .unwrap_err();
    assert_eq!(
        e,
        Error::Github {
            status: 422,
            message: "Validation Failed: Reference already exists".into()
        }
    );
}

#[test]
fn a_get_is_retried_on_502_to_504_and_every_answer_is_counted() {
    let s = Srv::new();
    s.mount(
        Mock::given(path("/user"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2),
    );
    s.mount(Mock::given(path("/user")).respond_with(user_ok()));
    let c = s.client();
    assert!(c.user().is_ok());
    let by: Vec<_> = c
        .counts()
        .iter()
        .map(|x| (x.result.clone(), x.count))
        .collect();
    assert!(by.contains(&("other".into(), 2)) && by.contains(&("200".into(), 1)));

    let s = Srv::new();
    s.mount(
        Mock::given(path("/user"))
            .respond_with(ResponseTemplate::new(502))
            .expect(3),
    );
    assert_eq!(s.client().user().unwrap_err().status(), Some(502));
}

#[test]
fn a_post_is_never_retried_on_5xx() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1),
    );
    assert_eq!(
        s.client().create_blob("o", "r", b"x").unwrap_err().status(),
        Some(503)
    );
}

#[test]
fn a_dead_host_is_a_network_error() {
    let s = Srv::new();
    let mut cfg = s.config();
    cfg.api = "http://127.0.0.1:1".into();
    let c = Client::new(cfg).unwrap();
    c.set_tokens(Tokens::plain("tok"));
    let e = c.user().unwrap_err();
    assert!(
        matches!(&e, Error::Network(m) if !m.contains("127.0.0.1:1/")),
        "{e}"
    );
    assert!(c.counts().is_empty());
}

#[test]
fn the_token_never_goes_to_another_host() {
    let s = Srv::new();
    s.mount(
        Mock::given(path("/user/repos"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("link", "<https://evil.example/x>; rel=\"next\"")
                    .set_body_json(json!([])),
            )
            .expect(1),
    );
    let e = s.client().user_repos().unwrap_err();
    assert_eq!(
        e.to_string(),
        "refusing to send the token to https://evil.example/x"
    );
    assert_eq!(s.requests().len(), 1);
}

#[test]
fn counts_name_the_caller_and_the_endpoint_shape() {
    let s = Srv::new();
    s.mount(
        Mock::given(path("/repos/o/r/pulls/12/reviews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([]))),
    );
    s.mount(
        Mock::given(path("/repos/o/r")).respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"})),
        ),
    );
    let c = s.client();
    with_caller("tick", || {
        c.reviews("o", "r", 12).unwrap();
        c.repo("o", "r").unwrap_err();
    });
    c.reviews("o", "r", 12).unwrap();
    let got: Vec<_> = c
        .counts()
        .into_iter()
        .map(|x| (x.caller, x.endpoint, x.result, x.count))
        .collect();
    assert!(got.contains(&(
        "tick".into(),
        "GET repos/:repo/pulls/:id/reviews".into(),
        "200".into(),
        1
    )));
    assert!(got.contains(&("tick".into(), "GET repos/:repo".into(), "other".into(), 1)));
    assert!(got.contains(&(
        "other".into(),
        "GET repos/:repo/pulls/:id/reviews".into(),
        "200".into(),
        1
    )));
}

fn refreshing(s: &Srv, access: &str, expires_at: i64) -> (Client, Arc<Mutex<Vec<String>>>) {
    let c = Client::new(s.config()).unwrap();
    c.set_tokens(Tokens {
        access_token: access.into(),
        refresh_token: Some("r1".into()),
        expires_at: Some(expires_at),
        refresh_token_expires_at: None,
    });
    let saved = Arc::new(Mutex::new(vec![]));
    let s2 = saved.clone();
    c.on_tokens(move |t| s2.lock().unwrap().push(t.access_token.clone()));
    (c, saved)
}

fn refresh_mock(s: &Srv) {
    s.mount(
        Mock::given(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token": "tok", "refresh_token": "r2", "expires_in": 28800}),
            ))
            .expect(1),
    );
}

#[test]
fn a_token_near_expiry_is_refreshed_first_and_handed_to_the_store() {
    let s = Srv::new();
    refresh_mock(&s);
    s.once("GET", "/user", 200, json!({"login": "o", "id": 1}));
    let (c, saved) = refreshing(&s, "old", now() + 60);
    c.user().unwrap();
    assert_eq!(*saved.lock().unwrap(), ["tok"]);
}

#[test]
fn a_401_renews_once_and_retries() {
    let s = Srv::new();
    refresh_mock(&s);
    s.mount(
        Mock::given(path("/user"))
            .and(header("authorization", "Bearer old"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1),
    );
    s.once("GET", "/user", 200, json!({"login": "o", "id": 1}));
    let (c, _) = refreshing(&s, "old", now() + 3600);
    assert_eq!(c.user().unwrap().login, "o");
}

#[test]
fn a_refused_refresh_token_signs_out() {
    let s = Srv::new();
    s.mount(Mock::given(path("/login/oauth/access_token")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!({"error": "bad_refresh_token"})),
    ));
    let (c, _) = refreshing(&s, "old", now() + 60);
    assert_eq!(c.user().unwrap_err(), Error::SignInExpired);
    assert!(c.is_expired());
}
