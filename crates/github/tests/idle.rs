//! v2-Budgets: an idle hour at a 1-minute tick, 3 groups and the personal repo, costs 424
//! requests, every one conditional and answered 304 (0 primary points).
mod common;
use claude_sync_github::{with_caller, Client};
use common::*;
use serde_json::json;
use wiremock::matchers::{header, method, path_regex};
use wiremock::{Mock, ResponseTemplate};

const REPOS: [&str; 4] = ["g1", "g2", "g3", "me-personal"];
const GROUPS: [&str; 3] = ["g1", "g2", "g3"];

fn tick(c: &Client, etags: &[Option<String>]) {
    with_caller("tick", || {
        for (r, e) in REPOS.iter().zip(etags) {
            c.head("o", r, "main", e.as_deref()).unwrap();
        }
        for g in GROUPS {
            c.open_pulls("o", g).unwrap();
        }
    });
}

#[test]
fn an_idle_hour_is_424_conditional_requests() {
    let s = Srv::new();
    // Every endpoint answers 304 to its ETag and 200 (with that ETag) otherwise.
    for (re, body) in [
        (r"^/repos/o/[^/]+/commits/main$", json!({"sha": "c1"})),
        (r"^/repos/o/[^/]+/pulls$", json!([])),
        (
            r"^/repos/o/[^/]+$",
            json!({"name": "r", "full_name": "o/r", "private": true}),
        ),
    ] {
        s.mount(
            Mock::given(method("GET"))
                .and(path_regex(re))
                .and(header("if-none-match", "\"e1\""))
                .respond_with(ResponseTemplate::new(304))
                .with_priority(1),
        );
        s.mount(
            Mock::given(method("GET")).and(path_regex(re)).respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "\"e1\"")
                    .set_body_json(body),
            ),
        );
    }
    let c = s.client();
    // Warm start: the ETags of the last run are saved (heads) or cached (pulls, repo reads).
    let etags: Vec<Option<String>> = REPOS.iter().map(|_| Some("\"e1\"".to_string())).collect();
    for g in GROUPS {
        c.open_pulls("o", g).unwrap();
    }
    for r in REPOS {
        c.repo("o", r).unwrap();
    }
    let before = s.requests().len();
    let counted_before: u64 = c.counts().iter().map(|x| x.count).sum();

    for _ in 0..60 {
        tick(&c, &etags);
    }
    // Once a day per repo.
    for r in REPOS {
        c.repo("o", r).unwrap();
    }

    let hour = &s.requests()[before..];
    assert_eq!(hour.len(), 424);
    assert!(hour.len() <= 424);
    assert!(
        hour.iter()
            .all(|r| r.headers.get("if-none-match").is_some()),
        "every idle request is conditional"
    );
    let s304: u64 = c
        .counts()
        .iter()
        .filter(|x| x.result == "304")
        .map(|x| x.count)
        .sum();
    let total: u64 = c.counts().iter().map(|x| x.count).sum();
    assert_eq!(total - counted_before, 424);
    assert!(s304 >= 424, "all 424 answered 304");
    assert!(c.limited_until().is_none());
    // The per-minute flush takes the counts and starts again from zero.
    assert_eq!(c.take_counts().iter().map(|x| x.count).sum::<u64>(), total);
    assert!(c.take_counts().is_empty());
}
