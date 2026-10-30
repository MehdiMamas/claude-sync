//! One GraphQL query for the state of many repos at once (issue #144): the current name and
//! the branch head of every group repo and the personal repo, in a single call.
use super::GitHub;
use crate::remote::Head;
use crate::{Error, GroupId};
use reqwest::Method;
use serde_json::{json, Value};
use std::collections::HashMap;

/// What the query says about one repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSnapshot {
    pub database_id: u64,
    /// `owner/repo` as GitHub names it now (differs from the asked name after a rename).
    pub name_with_owner: String,
    /// `Head::empty()` for a repo with no commits.
    pub head: Head,
}

/// The query for `ids`: one aliased `repository` per id (`r0`, `r1`, ...).
pub fn heads_query(ids: &[GroupId]) -> String {
    let mut out = String::from("query {");
    for (i, id) in ids.iter().enumerate() {
        out.push_str(&format!(
            " r{i}: repository(owner: {}, name: {}) {{ id databaseId nameWithOwner \
             defaultBranchRef {{ name target {{ ... on Commit {{ oid committedDate tree {{ oid }} }} }} }} }}",
            json!(id.owner),
            json!(id.repo)
        ));
    }
    out.push_str(" }");
    out
}

/// The answer for `ids` (as sent to `heads_query`). A repo the answer lacks, or GitHub
/// reports as `null` (no access, gone), is left out.
pub fn parse_heads(data: &Value, ids: &[GroupId]) -> HashMap<GroupId, RepoSnapshot> {
    let mut out = HashMap::new();
    for (i, id) in ids.iter().enumerate() {
        let Some(repo) = data.get(format!("r{i}")).filter(|r| r.is_object()) else {
            continue;
        };
        let (Some(database_id), Some(name)) =
            (repo["databaseId"].as_u64(), repo["nameWithOwner"].as_str())
        else {
            continue;
        };
        let head = match repo.get("defaultBranchRef").filter(|b| b.is_object()) {
            None => Head::empty(),
            Some(branch) => {
                let target = &branch["target"];
                let (Some(commit), Some(tree)) =
                    (target["oid"].as_str(), target["tree"]["oid"].as_str())
                else {
                    continue;
                };
                Head {
                    commit: commit.into(),
                    tree: tree.into(),
                    date: target["committedDate"].as_str().map(Into::into),
                    etag: None,
                }
            }
        };
        out.insert(
            id.clone(),
            RepoSnapshot {
                database_id,
                name_with_owner: name.into(),
                head,
            },
        );
    }
    out
}

impl GitHub {
    /// Name and head of each of `ids` in one GraphQL call. `Err` when the call fails or the
    /// GraphQL limit is used up; the caller falls back to the REST calls.
    pub async fn repo_snapshots(
        &self,
        ids: &[GroupId],
    ) -> crate::Result<HashMap<GroupId, RepoSnapshot>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let body = json!({ "query": heads_query(ids) });
        let reply: Value = self.send(Method::POST, "/graphql", &body).await?;
        match reply.get("data").filter(|d| d.is_object()) {
            Some(data) => Ok(parse_heads(data, ids)),
            None => Err(Error::Invalid(format!(
                "GraphQL answered without data: {}",
                reply["errors"]
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::CORE;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn id(o: &str, r: &str) -> GroupId {
        GroupId::new(o, r)
    }

    fn repo(name: &str, db: u64, branch: Value) -> Value {
        json!({ "id": "X", "databaseId": db, "nameWithOwner": name, "defaultBranchRef": branch })
    }

    fn branch(oid: &str, tree: &str) -> Value {
        json!({ "name": "main", "target": { "oid": oid, "committedDate": "2026-01-01T00:00:00Z", "tree": { "oid": tree } } })
    }

    #[test]
    fn the_query_has_one_alias_per_repo() {
        let q = heads_query(&[
            id("ana", "claude-sync-group"),
            id("ana", "claude-sync-personal"),
        ]);
        assert!(
            q.contains(r#"r0: repository(owner: "ana", name: "claude-sync-group")"#),
            "{q}"
        );
        assert!(
            q.contains(r#"r1: repository(owner: "ana", name: "claude-sync-personal")"#),
            "{q}"
        );
        assert!(!q.contains("r2:"));
        assert!(
            q.contains("nameWithOwner") && q.contains("databaseId") && q.contains("committedDate")
        );
        assert_eq!(q.matches('{').count(), q.matches('}').count());
    }

    #[test]
    fn the_answer_gives_heads_renames_and_gaps() {
        let ids = [
            id("ana", "a"),
            id("ana", "old"),
            id("ana", "gone"),
            id("ana", "empty"),
        ];
        let data = json!({
            "r0": repo("ana/a", 1, branch("c1", "t1")),
            "r1": repo("ana/new", 2, branch("c2", "t2")),
            "r2": null,
            "r3": repo("ana/empty", 4, Value::Null),
        });
        let got = parse_heads(&data, &ids);
        assert_eq!(got.len(), 3);
        let a = &got[&ids[0]];
        assert_eq!((a.database_id, a.name_with_owner.as_str()), (1, "ana/a"));
        assert_eq!((a.head.commit.as_str(), a.head.tree.as_str()), ("c1", "t1"));
        assert_eq!(a.head.date.as_deref(), Some("2026-01-01T00:00:00Z"));
        // Renamed: asked for `old`, GitHub says `new`.
        assert_eq!(got[&ids[1]].name_with_owner, "ana/new");
        // Missing repo is left out; an empty branch is an empty head.
        assert!(!got.contains_key(&ids[2]));
        assert!(got[&ids[3]].head.is_empty());
    }

    fn gh_for(server: &MockServer) -> GitHub {
        GitHub::with_api("tok".into(), server.uri())
    }

    #[tokio::test]
    async fn repo_snapshots_makes_one_post() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "r0": repo("ana/a", 1, branch("c1", "t1")) }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let got = gh_for(&server)
            .repo_snapshots(&[id("ana", "a")])
            .await
            .unwrap();
        assert_eq!(got[&id("ana", "a")].head.commit, "c1");
    }

    #[tokio::test]
    async fn an_error_answer_is_an_error_so_the_caller_falls_back() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "errors": [{ "message": "Something went wrong" }]
            })))
            .mount(&server)
            .await;
        assert!(gh_for(&server)
            .repo_snapshots(&[id("ana", "a")])
            .await
            .is_err());
    }

    #[tokio::test]
    async fn the_graphql_limit_does_not_block_rest_and_the_other_way_round() {
        let server = MockServer::start().await;
        let reset = crate::state::now_unix() + 600;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-resource", "graphql")
                    .insert_header("x-ratelimit-remaining", "0")
                    .insert_header("x-ratelimit-reset", reset.to_string().as_str())
                    .set_body_json(json!({"message": "rate limit"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"login": "me"})))
            .mount(&server)
            .await;
        let gh = gh_for(&server);
        let ids = [id("ana", "a")];
        assert!(matches!(
            gh.repo_snapshots(&ids).await,
            Err(Error::RateLimited { .. })
        ));
        assert_eq!(gh.graphql_limited_until(), Some(reset));
        assert_eq!(gh.limited_until(), None);
        assert_eq!(gh.user().await.unwrap().login, "me");
        // No second POST while limited (the mock expects exactly one).
        assert!(matches!(
            gh.repo_snapshots(&ids).await,
            Err(Error::RateLimited { .. })
        ));

        // And a REST limit leaves GraphQL alone.
        let gh = gh_for(&server);
        gh.limited_until.lock().unwrap().insert(CORE, reset);
        assert_eq!(gh.graphql_limited_until(), None);
    }
}
