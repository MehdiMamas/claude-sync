//! The engine's `Remote` on the client, against wiremock.
mod common;
use claude_sync_engine::traits::Remote;
use claude_sync_github::remote::GithubRemote;
use common::*;
use serde_json::json;
use std::collections::BTreeMap;
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

fn tree_mock(s: &Srv, sha: &str) {
    s.mount(
        Mock::given(method("GET"))
            .and(path(format!("/repos/o/r/git/trees/{sha}")))
            .and(query_param("recursive", "1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"sha": "t1", "tree": [
                {"path": "CLAUDE.md", "mode": "100644", "type": "blob", "sha": "b1"},
                {"path": "skills", "mode": "040000", "type": "tree", "sha": "t9"},
                {"path": "skills/a.md", "mode": "100644", "type": "blob", "sha": "b2"}]})),
            )
            .expect(1),
    );
}

#[test]
fn head_is_none_when_the_etag_matches() {
    let s = Srv::new();
    s.mount(
        Mock::given(path("/repos/o/r/commits/main"))
            .and(header("if-none-match", "\"e1\""))
            .respond_with(ResponseTemplate::new(304))
            .expect(1),
    );
    let r = GithubRemote::new(s.client(), "o", "r", "main");
    assert_eq!(r.head(Some("\"e1\"")).unwrap(), None);
}

#[test]
fn files_downloads_each_blob_once_and_reuses_unchanged_ones() {
    let s = Srv::new();
    tree_mock(&s, "c1");
    s.mount(
        Mock::given(path("/repos/o/r/git/blobs/b1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"content": "YQ==", "encoding": "base64"})),
            )
            .expect(1),
    );
    s.mount(
        Mock::given(path("/repos/o/r/git/blobs/b2"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"content": "Yg==", "encoding": "base64"})),
            )
            .expect(1),
    );
    // The next commit: b1 unchanged, b2 gone, b3 new.
    s.mount(
        Mock::given(path("/repos/o/r/git/trees/c2"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"sha": "t2", "tree": [
        {"path": "CLAUDE.md", "mode": "100644", "type": "blob", "sha": "b1"},
        {"path": "new.md", "mode": "100644", "type": "blob", "sha": "b3"}]})),
            )
            .expect(1),
    );
    s.mount(
        Mock::given(path("/repos/o/r/git/blobs/b3"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"content": "Yw==", "encoding": "base64"})),
            )
            .expect(1),
    );
    let r = GithubRemote::new(s.client(), "o", "r", "main");
    let f = r.files("c1", &BTreeMap::new()).unwrap();
    assert_eq!(f["CLAUDE.md"], b"a");
    assert_eq!(f["skills/a.md"], b"b");
    assert_eq!(f.len(), 2);
    let f = r.files("c2", &BTreeMap::new()).unwrap();
    assert_eq!(f.keys().collect::<Vec<_>>(), ["CLAUDE.md", "new.md"]);
}

#[test]
fn push_makes_blobs_a_tree_on_the_parents_tree_a_commit_and_moves_the_ref() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/repos/o/r/git/trees/c1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"sha": "t1", "tree": []})),
            )
            .expect(1),
    );
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/blobs"))
            .and(body_partial_json(json!({"content": "aGk="})))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "b9"})))
            .expect(1),
    );
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/trees"))
            .and(body_partial_json(json!({"base_tree": "t1", "tree": [
        {"path": "CLAUDE.md", "sha": "b9"}, {"path": "old.md", "sha": null}]})))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "t2"})))
            .expect(1),
    );
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/commits"))
            .and(body_partial_json(
                json!({"tree": "t2", "parents": ["c1"], "message": "Share"}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "c2"})))
            .expect(1),
    );
    s.mount(
        Mock::given(method("PATCH"))
            .and(path("/repos/o/r/git/refs/heads/main"))
            .and(body_partial_json(json!({"sha": "c2", "force": false})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1),
    );
    let r = GithubRemote::new(s.client(), "o", "r", "main");
    let pushes = BTreeMap::from([
        ("CLAUDE.md".to_string(), Some(b"hi".to_vec())),
        ("old.md".to_string(), None),
    ]);
    assert_eq!(r.push("c1", &pushes, "Share").unwrap(), "c2");
}

#[test]
fn push_that_lost_the_race_says_so() {
    let s = Srv::new();
    s.mount(
        Mock::given(method("GET")).respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"sha": "t1", "tree": []})),
        ),
    );
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/trees"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "t2"}))),
    );
    s.mount(
        Mock::given(method("POST"))
            .and(path("/repos/o/r/git/commits"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha": "c2"}))),
    );
    s.mount(
        Mock::given(method("PATCH")).respond_with(
            ResponseTemplate::new(422)
                .set_body_json(json!({"message": "Update is not a fast forward"})),
        ),
    );
    let r = GithubRemote::new(s.client(), "o", "r", "main");
    let e = r.push("c1", &BTreeMap::new(), "m").unwrap_err();
    assert_eq!(e.0, "someone else pushed first");
}

#[test]
fn push_to_an_empty_repo_uses_the_contents_api() {
    use std::collections::BTreeMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};
    let s = Srv::new();
    s.mount(
        Mock::given(method("PUT"))
            .and(path("/repos/o/r/contents/claude-sync.json"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"commit": {"sha": "c1"}})),
            )
            .expect(1),
    );
    let r = GithubRemote::new(s.client(), "o", "r", "main");
    let mut p = BTreeMap::new();
    p.insert("claude-sync.json".to_string(), Some(b"{}".to_vec()));
    assert_eq!(r.push("", &p, "create group").unwrap(), "c1");
}
