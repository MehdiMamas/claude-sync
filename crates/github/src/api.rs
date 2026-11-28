//! One typed function per row of v2-Endpoints. Callers name themselves with `with_caller`.
use crate::client::PlainBody;
use crate::{Client, Error, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use claude_sync_engine::traits::Head;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Topic that marks a repo as a group.
pub const TOPIC: &str = "claude-sync-group";
/// Name of the ruleset that keeps everyone but admins from pushing to main.
pub const HOST_LOCK_NAME: &str = "claude-sync: only the host updates the group";

#[derive(Debug, Clone, Deserialize)]
pub struct Owner {
    pub login: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Perms {
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub push: bool,
    #[serde(default)]
    pub pull: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub login: String,
    pub id: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Repo {
    pub name: String,
    pub full_name: String,
    #[serde(default)]
    pub private: bool,
    pub owner: Option<Owner>,
    pub permissions: Option<Perms>,
    pub topics: Option<Vec<String>>,
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    #[serde(default)]
    pub mode: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub sha: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tree {
    pub sha: String,
    #[serde(default)]
    pub tree: Vec<TreeEntry>,
    #[serde(default)]
    pub truncated: bool,
}

/// A pending invitation: to a repo (`repository`, `inviter`) or on a repo (`invitee`).
#[derive(Debug, Clone, Deserialize)]
pub struct Invitation {
    pub id: u64,
    pub repository: Option<Repo>,
    pub inviter: Option<Owner>,
    pub invitee: Option<Owner>,
    /// RFC 3339.
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Collaborator {
    pub login: String,
    pub permissions: Option<Perms>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PullRef {
    pub sha: String,
    #[serde(rename = "ref")]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pull {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub state: String,
    pub head: Option<PullRef>,
    pub user: Option<Owner>,
    pub updated_at: Option<String>,
    pub created_at: Option<String>,
    #[serde(default)]
    pub html_url: String,
    /// Only the single-pull reply has it; `Some(false)`: behind main, needs an update.
    #[serde(default)]
    pub mergeable: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Review {
    pub user: Option<Owner>,
    pub state: String,
    /// The branch head the review was given on.
    #[serde(default)]
    pub commit_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Parent {
    pub sha: String,
}

/// One commit with the files it touched.
#[derive(Debug, Clone, Deserialize)]
pub struct CommitDetail {
    pub sha: String,
    #[serde(default)]
    pub parents: Vec<Parent>,
    #[serde(default)]
    pub files: Vec<ChangedFile>,
    #[serde(default)]
    pub commit: Option<CommitInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Merged {
    pub sha: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitAuthor {
    pub name: String,
    pub date: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitInfo {
    pub message: String,
    pub author: Option<CommitAuthor>,
    pub committer: Option<CommitAuthor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitItem {
    pub sha: String,
    pub commit: CommitInfo,
    pub author: Option<Owner>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangedFile {
    pub filename: String,
    pub status: String,
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Compare {
    #[serde(default)]
    pub files: Vec<ChangedFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Installation {
    pub id: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Platform {
    pub url: String,
    pub signature: String,
}

/// A version goes into a download URL: only `1.2.3` or `1.2.3-rc.1` is let through.
fn check_version(v: &str) -> Result<()> {
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let ok = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && !p.contains("..") && p.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(|p| {
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
        });
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("bad version in latest.json: {v:?}")))
    }
}

/// `latest.json` of the updater.
#[derive(Debug, Clone, Deserialize)]
pub struct Latest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub platforms: BTreeMap<String, Platform>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeedbackCreated {
    pub number: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Health {
    pub ok: bool,
    #[serde(default)]
    pub version: String,
}

#[derive(Deserialize)]
struct Sha {
    sha: String,
}

fn rp(o: &str, r: &str, rest: &str) -> String {
    format!("/repos/{o}/{r}{rest}")
}

fn is_race(e: &Error) -> bool {
    matches!(e, Error::Github { status: 422, message }
        if { let m = message.to_lowercase(); m.contains("fast forward") || m.contains("cannot be updated") })
}

impl Client {
    // ----- account ---------------------------------------------------------------------

    pub fn user(&self) -> Result<User> {
        self.get("/user")?.json()
    }

    /// Repos the user can read; every page, conditional.
    pub fn user_repos(&self) -> Result<Vec<Repo>> {
        self.get_pages("/user/repos?affiliation=owner,collaborator&per_page=100")
    }

    pub fn topics(&self, o: &str, r: &str) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct T {
            names: Vec<String>,
        }
        Ok(self.get(&rp(o, r, "/topics"))?.json::<T>()?.names)
    }

    pub fn set_topics(&self, o: &str, r: &str, names: &[&str]) -> Result<()> {
        self.send(
            Method::PUT,
            &rp(o, r, "/topics"),
            Some(&json!({ "names": names })),
        )
        .map(drop)
    }

    /// GitHub App mode.
    pub fn installations(&self) -> Result<Vec<Installation>> {
        #[derive(Deserialize)]
        struct R {
            installations: Vec<Installation>,
        }
        Ok(self
            .get("/user/installations?per_page=100")?
            .json::<R>()?
            .installations)
    }

    pub fn installation_repos(&self, id: u64) -> Result<Vec<Repo>> {
        #[derive(Deserialize)]
        struct R {
            repositories: Vec<Repo>,
        }
        Ok(self
            .get(&format!(
                "/user/installations/{id}/repositories?per_page=100"
            ))?
            .json::<R>()?
            .repositories)
    }

    // ----- repos -----------------------------------------------------------------------

    /// A renamed repo answers by redirect, which is followed.
    pub fn repo(&self, o: &str, r: &str) -> Result<Repo> {
        self.get(&rp(o, r, ""))?.json()
    }

    /// A new private repo with a first commit (the Git Data API refuses an empty one).
    pub fn create_repo(&self, name: &str, description: &str) -> Result<Repo> {
        let body =
            json!({"name": name, "private": true, "auto_init": true, "description": description});
        let repo: Repo = self
            .send(Method::POST, "/user/repos", Some(&body))?
            .json()?;
        if !repo.private {
            return Err(Error::Invalid(format!(
                "repo {} is public; claude-sync only uses private repos",
                repo.full_name
            )));
        }
        Ok(repo)
    }

    /// Only admins push to the default branch.
    pub fn create_ruleset(&self, o: &str, r: &str) -> Result<()> {
        let body = json!({
            "name": HOST_LOCK_NAME,
            "target": "branch",
            "enforcement": "active",
            "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
            "rules": [{"type": "update"}, {"type": "deletion"}, {"type": "non_fast_forward"}],
            // Repository role 5 is admin.
            "bypass_actors": [{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always"}],
        });
        self.send(Method::POST, &rp(o, r, "/rulesets"), Some(&body))
            .map(drop)
    }

    /// Names of the rulesets on a repo (to create ours once).
    pub fn rulesets(&self, o: &str, r: &str) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct R {
            name: String,
        }
        let v: Vec<R> = self.get(&rp(o, r, "/rulesets"))?.json()?;
        Ok(v.into_iter().map(|r| r.name).collect())
    }

    /// Create `path` through the contents API: the only write an empty repo accepts. Returns
    /// the new commit sha.
    pub fn put_file(
        &self,
        o: &str,
        r: &str,
        path: &str,
        bytes: &[u8],
        message: &str,
    ) -> Result<String> {
        #[derive(Deserialize)]
        struct C {
            sha: String,
        }
        #[derive(Deserialize)]
        struct R {
            commit: C,
        }
        let body = json!({"message": message, "content": STANDARD.encode(bytes)});
        let r: R = self
            .send(
                Method::PUT,
                &rp(o, r, &format!("/contents/{path}")),
                Some(&body),
            )?
            .json()?;
        Ok(r.commit.sha)
    }

    pub fn transfer(&self, o: &str, r: &str, new_owner: &str) -> Result<()> {
        let body = json!({ "new_owner": new_owner });
        self.send(Method::POST, &rp(o, r, "/transfer"), Some(&body))
            .map(drop)
    }

    // ----- people ----------------------------------------------------------------------

    pub fn collaborators(&self, o: &str, r: &str) -> Result<Vec<Collaborator>> {
        self.get_pages(&rp(o, r, "/collaborators?per_page=100"))
    }

    /// Read access. User-owned repos refuse a permission: one retry with no body.
    pub fn invite(&self, o: &str, r: &str, login: &str) -> Result<()> {
        let path = rp(o, r, &format!("/collaborators/{login}"));
        let body = json!({ "permission": "pull" });
        match self.send(Method::PUT, &path, Some(&body)) {
            Err(Error::Github {
                status: 422,
                message,
            }) if message.to_lowercase().contains("permission") => {
                self.send(Method::PUT, &path, None).map(drop)
            }
            r => r.map(drop),
        }
    }

    /// Pending invitations on a repo (host).
    pub fn repo_invitations(&self, o: &str, r: &str) -> Result<Vec<Invitation>> {
        self.get_pages(&rp(o, r, "/invitations?per_page=100"))
    }

    pub fn cancel_invitation(&self, o: &str, r: &str, id: u64) -> Result<()> {
        self.send(
            Method::DELETE,
            &rp(o, r, &format!("/invitations/{id}")),
            None,
        )
        .map(drop)
    }

    /// Remove a member, or leave (`login` is your own).
    pub fn remove_collaborator(&self, o: &str, r: &str, login: &str) -> Result<()> {
        self.send(
            Method::DELETE,
            &rp(o, r, &format!("/collaborators/{login}")),
            None,
        )
        .map(drop)
    }

    /// Invitations to repos for the signed-in user.
    pub fn invitations(&self) -> Result<Vec<Invitation>> {
        self.get_pages("/user/repository_invitations?per_page=100")
    }

    pub fn accept_invitation(&self, id: u64) -> Result<()> {
        self.send(
            Method::PATCH,
            &format!("/user/repository_invitations/{id}"),
            None,
        )
        .map(drop)
    }

    // ----- git data --------------------------------------------------------------------

    /// `None`: the head did not move (`etag` matched). The ETag comes back with the sha.
    pub fn head(&self, o: &str, r: &str, branch: &str, etag: Option<&str>) -> Result<Option<Head>> {
        let resp = self.get_etag(&rp(o, r, &format!("/commits/{branch}")), etag)?;
        if resp.status == 304 {
            return Ok(None);
        }
        Ok(Some(Head {
            sha: resp.json::<Sha>()?.sha,
            etag: resp.etag,
        }))
    }

    /// `sha` may be a commit or a tree.
    pub fn tree(&self, o: &str, r: &str, sha: &str, recursive: bool) -> Result<Tree> {
        let q = if recursive { "?recursive=1" } else { "" };
        self.get(&rp(o, r, &format!("/git/trees/{sha}{q}")))?.json()
    }

    pub fn blob(&self, o: &str, r: &str, sha: &str) -> Result<Vec<u8>> {
        #[derive(Deserialize)]
        struct B {
            content: String,
        }
        let b: B = self.get(&rp(o, r, &format!("/git/blobs/{sha}")))?.json()?;
        let clean: String = b.content.chars().filter(|c| !c.is_whitespace()).collect();
        STANDARD
            .decode(clean)
            .map_err(|e| Error::Invalid(format!("bad blob: {e}")))
    }

    pub fn create_blob(&self, o: &str, r: &str, bytes: &[u8]) -> Result<String> {
        let body = json!({"content": STANDARD.encode(bytes), "encoding": "base64"});
        Ok(self
            .send(Method::POST, &rp(o, r, "/git/blobs"), Some(&body))?
            .json::<Sha>()?
            .sha)
    }

    /// `entries`: `{path, mode, type, sha}` (`sha: null` deletes).
    pub fn create_tree(
        &self,
        o: &str,
        r: &str,
        base_tree: &str,
        entries: &[Value],
    ) -> Result<String> {
        let body = json!({"base_tree": base_tree, "tree": entries});
        Ok(self
            .send(Method::POST, &rp(o, r, "/git/trees"), Some(&body))?
            .json::<Sha>()?
            .sha)
    }

    pub fn create_commit(
        &self,
        o: &str,
        r: &str,
        message: &str,
        tree: &str,
        parent: &str,
    ) -> Result<String> {
        let body = json!({"message": message, "tree": tree, "parents": [parent]});
        Ok(self
            .send(Method::POST, &rp(o, r, "/git/commits"), Some(&body))?
            .json::<Sha>()?
            .sha)
    }

    /// Move the branch, never forced. Another push that won gives `Error::PushRace`.
    pub fn update_ref(&self, o: &str, r: &str, branch: &str, sha: &str) -> Result<()> {
        let body = json!({"sha": sha, "force": false});
        match self.send(
            Method::PATCH,
            &rp(o, r, &format!("/git/refs/heads/{branch}")),
            Some(&body),
        ) {
            Err(e) if is_race(&e) => Err(Error::PushRace),
            r => r.map(drop),
        }
    }

    /// `full_ref` is `refs/heads/<name>`.
    pub fn create_ref(&self, o: &str, r: &str, full_ref: &str, sha: &str) -> Result<()> {
        let body = json!({"ref": full_ref, "sha": sha});
        self.send(Method::POST, &rp(o, r, "/git/refs"), Some(&body))
            .map(drop)
    }

    pub fn commits(
        &self,
        o: &str,
        r: &str,
        branch: &str,
        per_page: u32,
    ) -> Result<Vec<CommitItem>> {
        self.get(&rp(
            o,
            r,
            &format!("/commits?sha={branch}&per_page={per_page}"),
        ))?
        .json()
    }

    pub fn compare(&self, o: &str, r: &str, base: &str, head: &str) -> Result<Compare> {
        self.get(&rp(o, r, &format!("/compare/{base}...{head}")))?
            .json()
    }

    // ----- proposals -------------------------------------------------------------------

    pub fn create_pull(
        &self,
        o: &str,
        r: &str,
        title: &str,
        head: &str,
        base: &str,
    ) -> Result<Pull> {
        let body = json!({
            "title": title, "head": head, "base": base,
            "body": "Proposed with claude-sync. Approve or decline it in the claude-sync app.",
        });
        self.send(Method::POST, &rp(o, r, "/pulls"), Some(&body))?
            .json()
    }

    /// Open pull requests; a `304` is answered from the cache, so it costs nothing.
    pub fn open_pulls(&self, o: &str, r: &str) -> Result<Vec<Pull>> {
        self.get_pages(&rp(o, r, "/pulls?state=open&per_page=100"))
    }

    /// The single pull: the list reply leaves `mergeable` out.
    pub fn pull(&self, o: &str, r: &str, n: u64) -> Result<Pull> {
        self.get(&rp(o, r, &format!("/pulls/{n}")))?.json()
    }

    pub fn pull_files(&self, o: &str, r: &str, n: u64) -> Result<Vec<ChangedFile>> {
        self.get_pages(&rp(o, r, &format!("/pulls/{n}/files?per_page=100")))
    }

    pub fn comment(&self, o: &str, r: &str, n: u64, body: &str) -> Result<()> {
        let body = json!({ "body": body });
        self.send(
            Method::POST,
            &rp(o, r, &format!("/issues/{n}/comments")),
            Some(&body),
        )
        .map(drop)
    }

    pub fn commit_detail(&self, o: &str, r: &str, sha: &str) -> Result<CommitDetail> {
        self.get(&rp(o, r, &format!("/commits/{sha}")))?.json()
    }

    pub fn reviews(&self, o: &str, r: &str, n: u64) -> Result<Vec<Review>> {
        self.get_pages(&rp(o, r, &format!("/pulls/{n}/reviews?per_page=100")))
    }

    pub fn approve(&self, o: &str, r: &str, n: u64) -> Result<()> {
        let body = json!({ "event": "APPROVE" });
        self.send(
            Method::POST,
            &rp(o, r, &format!("/pulls/{n}/reviews")),
            Some(&body),
        )
        .map(drop)
    }

    /// `head_sha` pins the merge to what was read; a changed branch answers 409.
    pub fn merge_pull(&self, o: &str, r: &str, n: u64, head_sha: Option<&str>) -> Result<Merged> {
        let body = head_sha.map(|s| json!({ "sha": s }));
        self.send(
            Method::PUT,
            &rp(o, r, &format!("/pulls/{n}/merge")),
            body.as_ref(),
        )?
        .json()
    }

    pub fn close_pull(&self, o: &str, r: &str, n: u64) -> Result<Pull> {
        let body = json!({ "state": "closed" });
        self.send(
            Method::PATCH,
            &rp(o, r, &format!("/pulls/{n}")),
            Some(&body),
        )?
        .json()
    }

    // ----- releases and relay ----------------------------------------------------------

    /// The page with installers; opened from a browser, never fetched.
    pub fn release_page_url(&self) -> String {
        format!("{}/latest", self.config().releases.trim_end_matches('/'))
    }

    pub fn latest(&self) -> Result<Latest> {
        let url = format!(
            "{}/latest/download/latest.json",
            self.config().releases.trim_end_matches('/')
        );
        let (status, body) = self.plain(Method::GET, &url, None, PlainBody::None)?;
        if status != 200 {
            return Err(Error::Github {
                status,
                message: "no update info".into(),
            });
        }
        let l: Latest = serde_json::from_slice(&body)
            .map_err(|e| Error::Invalid(format!("bad latest.json: {e}")))?;
        check_version(&l.version)?;
        Ok(l)
    }

    /// A file of release `v<version>` (the command line's archive and its `.sha256`).
    pub fn release_asset(&self, version: &str, name: &str) -> Result<Vec<u8>> {
        check_version(version)?;
        let url = format!(
            "{}/download/v{version}/{name}",
            self.config().releases.trim_end_matches('/')
        );
        let (status, body) = self.plain(Method::GET, &url, None, PlainBody::None)?;
        if status != 200 {
            return Err(Error::Github {
                status,
                message: format!("{name} is not in release v{version}"),
            });
        }
        Ok(body)
    }

    /// Send feedback; the relay checks the user token against GitHub.
    pub fn feedback(&self, payload: &Value) -> Result<FeedbackCreated> {
        let token = self.token()?;
        let url = format!("{}/api/feedback", self.config().relay.trim_end_matches('/'));
        let (status, body) = self.plain(
            Method::POST,
            &url,
            Some(&token),
            PlainBody::Json(payload.clone()),
        )?;
        if (200..300).contains(&status) {
            return serde_json::from_slice(&body)
                .map_err(|e| Error::Invalid(format!("unexpected relay reply: {e}")));
        }
        // The relay's `{"error": "..."}` as it is, so the person reads why.
        let msg = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(Value::as_str)
                    .map(|s| s.trim().chars().take(600).collect())
            })
            .filter(|s: &String| !s.is_empty())
            .unwrap_or_else(|| {
                format!(
                    "relay returned {status}: {}",
                    String::from_utf8_lossy(&body)
                        .chars()
                        .take(300)
                        .collect::<String>()
                )
            });
        Err(Error::Github {
            status,
            message: msg,
        })
    }

    pub fn health(&self) -> Result<Health> {
        let url = format!("{}/api/health", self.config().relay.trim_end_matches('/'));
        let (status, body) = self.plain(Method::GET, &url, None, PlainBody::None)?;
        if status != 200 {
            return Err(Error::Github {
                status,
                message: "relay not healthy".into(),
            });
        }
        serde_json::from_slice(&body).map_err(|e| Error::Invalid(format!("bad reply: {e}")))
    }
}

#[cfg(test)]
mod latest_tests {
    use super::Latest;

    #[test]
    fn latest_json_of_the_updater_parses() {
        let l: Latest = serde_json::from_str(
            r#"{"version":"2.0.0","notes":"n","pub_date":"2026-01-01T00:00:00Z","platforms":{
                "windows-x86_64":{"url":"https://x/a-setup.exe","signature":"sig"},
                "darwin-aarch64":{"url":"https://x/a.app.tar.gz","signature":"sig"}}}"#,
        )
        .unwrap();
        assert_eq!(l.version, "2.0.0");
        assert_eq!(l.platforms["windows-x86_64"].signature, "sig");
        assert!(!l.platforms.contains_key("linux-aarch64"));
        // only the version is needed (the aarch64 Linux command line reads just that)
        let l: Latest = serde_json::from_str(r#"{"version":"2.0.1"}"#).unwrap();
        assert!(l.platforms.is_empty());
    }
}
