//! The GitHub App sign-in (issue #39): which repos the App is installed on, and which of the
//! user's group repos it does not cover. Sign-in itself is the same device flow with the
//! App's client ID; the App's user tokens expire and refresh like the OAuth ones.
use super::GitHub;
use crate::GroupId;
use serde::Deserialize;

/// Where the user installs the App on their account or an organisation.
pub fn install_url(slug: &str) -> String {
    format!("https://github.com/apps/{slug}/installations/new")
}

/// Where the user changes the repos of an existing installation.
pub fn manage_url(slug: &str) -> String {
    format!("https://github.com/apps/{slug}")
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Installation {
    pub id: u64,
    pub account: InstallationAccount,
    /// `all` or `selected`.
    #[serde(default)]
    pub repository_selection: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InstallationAccount {
    pub login: String,
}

#[derive(Deserialize)]
struct RepoPage {
    #[serde(default)]
    repositories: Vec<RepoName>,
}

#[derive(Deserialize)]
struct RepoName {
    full_name: String,
}

#[derive(Deserialize)]
struct InstallationPage {
    #[serde(default)]
    installations: Vec<Installation>,
}

/// What the App reaches, against the repos the user needs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// Repos the App is installed on (all installations the user can use).
    pub covered: Vec<GroupId>,
    /// Wanted repos the App cannot reach: the user must add them to an installation.
    pub missing: Vec<GroupId>,
    /// The App is installed on no account at all.
    pub no_installation: bool,
}

/// The wanted repos that are not in `installed` (compared case-insensitively).
pub fn uncovered(wanted: &[GroupId], installed: &[GroupId]) -> Vec<GroupId> {
    let same = |a: &GroupId, b: &GroupId| {
        a.owner.eq_ignore_ascii_case(&b.owner) && a.repo.eq_ignore_ascii_case(&b.repo)
    };
    wanted
        .iter()
        .filter(|w| !installed.iter().any(|i| same(w, i)))
        .cloned()
        .collect()
}

impl GitHub {
    /// Like `get_pages`, for endpoints that wrap each page's list in an object.
    async fn get_object_pages<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> crate::Result<Vec<T>> {
        let mut out = Vec::new();
        let mut next = Some(path.to_string());
        while let Some(p) = next {
            let resp = self.request(reqwest::Method::GET, &p, None, None).await?;
            out.push(resp.json::<T>()?);
            next = resp.next;
        }
        Ok(out)
    }

    /// `GET /user/installations`: installations of the App the signed-in user can use.
    pub async fn installations(&self) -> crate::Result<Vec<Installation>> {
        let pages: Vec<InstallationPage> = self
            .get_object_pages("/user/installations?per_page=100")
            .await?;
        Ok(pages.into_iter().flat_map(|p| p.installations).collect())
    }

    /// `GET /user/installations/{id}/repositories`: repos of one installation the user can reach.
    pub async fn installation_repos(&self, installation: u64) -> crate::Result<Vec<GroupId>> {
        let pages: Vec<RepoPage> = self
            .get_object_pages(&format!(
                "/user/installations/{installation}/repositories?per_page=100"
            ))
            .await?;
        Ok(pages
            .into_iter()
            .flat_map(|p| p.repositories)
            .filter_map(|r| GroupId::parse(&r.full_name))
            .collect())
    }

    /// Which of `wanted` the App reaches.
    pub async fn app_coverage(&self, wanted: &[GroupId]) -> crate::Result<Coverage> {
        let installations = self.installations().await?;
        let mut covered = Vec::new();
        for i in &installations {
            covered.extend(self.installation_repos(i.id).await?);
        }
        Ok(Coverage {
            missing: uncovered(wanted, &covered),
            covered,
            no_installation: installations.is_empty(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn id(s: &str) -> GroupId {
        GroupId::parse(s).unwrap()
    }

    #[test]
    fn urls() {
        assert_eq!(
            install_url("claude-sync"),
            "https://github.com/apps/claude-sync/installations/new"
        );
    }

    #[test]
    fn uncovered_ignores_case() {
        let missing = uncovered(&[id("Me/a"), id("me/b")], &[id("me/A")]);
        assert_eq!(missing, [id("me/b")]);
    }

    #[tokio::test]
    async fn coverage_lists_installations_and_their_repos() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/installations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "total_count": 1,
                "installations": [{"id": 7, "account": {"login": "me"}, "repository_selection": "selected"}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user/installations/7/repositories"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "total_count": 1,
                "repositories": [{"full_name": "me/claude-sync-data"}]
            })))
            .mount(&server)
            .await;
        let gh =
            GitHub::with_api("tok".into(), server.uri()).with_retry_delay(Duration::from_millis(1));
        let c = gh
            .app_coverage(&[id("me/claude-sync-data"), id("friend/group")])
            .await
            .unwrap();
        assert_eq!(c.covered, [id("me/claude-sync-data")]);
        assert_eq!(c.missing, [id("friend/group")]);
        assert!(!c.no_installation);
    }

    #[tokio::test]
    async fn no_installation_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/installations"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"total_count": 0, "installations": []})),
            )
            .mount(&server)
            .await;
        let gh = GitHub::with_api("tok".into(), server.uri());
        let c = gh.app_coverage(&[id("me/x")]).await.unwrap();
        assert!(c.no_installation);
        assert_eq!(c.missing, [id("me/x")]);
    }
}
