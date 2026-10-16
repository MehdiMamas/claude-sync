//! Reverting one commit of a group from the activity feed (docs/architecture.md §6).
//!
//! A revert is a NEW commit on the group repo's default branch that puts back, for each
//! allowlisted file the target commit touched, the file as it was at the commit's parent:
//! a file the commit added is deleted, one it deleted is restored. It goes through the same
//! secret scan and push path as every other change (fetch and retry on a push race, never
//! force), and the host only (§5a): a member gets `Error::HostOnly`.
//!
//! Safer rather than smarter: a file that changed again after the target commit is NOT
//! merged or overwritten. The revert is refused as a whole (nothing is committed) and names
//! each such file with the later commit and its author, so the host reverts that one first.
use super::Engine;
use crate::group;
use crate::remote::Changes;
use crate::{Error, GroupId};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RevertAction {
    /// The file goes back to its older content.
    Restore,
    /// The commit added the file, so the revert deletes it.
    Delete,
    /// The commit deleted the file, so the revert brings it back.
    Recreate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertFile {
    pub path: String,
    pub action: RevertAction,
    /// Why this file cannot be reverted now (it changed again); `None` when it can.
    pub blocked: Option<String>,
}

/// What reverting a commit would do (and, after `revert_commit`, did).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertPlan {
    pub sha: String,
    /// The target commit's first message line.
    pub summary: String,
    pub files: Vec<RevertFile>,
}

struct Planned {
    plan: RevertPlan,
    /// The content each reverted file has at the target commit, to check nothing moved.
    expected: Vec<(String, Option<Vec<u8>>)>,
    changes: Changes,
}

fn sha7(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// The first message line without the app's `login@device: ` prefix, kept short.
fn original_summary(message: &str) -> String {
    let first = message.lines().next().unwrap_or("").trim();
    let text = match first.split_once(": ") {
        Some((who, rest)) if who.contains('@') && !who.contains(char::is_whitespace) => rest,
        _ => first,
    };
    let mut out: String = text.chars().take(72).collect();
    if text.chars().count() > 72 {
        out.push('…');
    }
    out
}

impl Engine {
    /// The files reverting `sha` would change, and which of them cannot be reverted now.
    /// Host only. Writes nothing.
    pub async fn revert_preview(&self, id: &GroupId, sha: &str) -> crate::Result<RevertPlan> {
        self.require_joined(id)?;
        Ok(self.plan_revert(id, sha).await?.plan)
    }

    /// Revert `sha` with a new commit, then sync so this device applies it. Host only.
    pub async fn revert_commit(&self, id: &GroupId, sha: &str) -> crate::Result<RevertPlan> {
        let _busy = self.busy.lock().await;
        self.require_joined(id)?;
        let planned = self.plan_revert(id, sha).await?;
        let blocked: Vec<&str> = planned
            .plan
            .files
            .iter()
            .filter_map(|f| f.blocked.as_deref())
            .collect();
        if !blocked.is_empty() {
            return Err(Error::Invalid(format!(
                "Nothing was reverted. {}",
                blocked.join(" ")
            )));
        }
        if planned.changes.is_empty() {
            return Err(Error::Invalid(format!(
                "Nothing to revert: the files of {} already match what they were before it",
                sha7(&planned.plan.sha)
            )));
        }
        self.scan(&planned.changes)?;
        let summary = format!(
            "revert {} ({})",
            sha7(&planned.plan.sha),
            planned.plan.summary
        );
        let (changes, expected) = (&planned.changes, &planned.expected);
        self.commit_to_group(id, &summary, |_, current| {
            // The branch may have moved while retrying: re-check against the new head.
            for (path, at_target) in expected {
                if current.get(path) != at_target.as_ref() {
                    return Err(Error::Invalid(format!(
                        "Nothing was reverted: {path} changed again while reverting. Try again."
                    )));
                }
            }
            Ok(Some(changes.clone()))
        })
        .await?;
        match self.sync_group(id, None).await {
            (_, Some(e)) => Err(e),
            (_, None) => Ok(planned.plan),
        }
    }

    async fn plan_revert(&self, id: &GroupId, sha: &str) -> crate::Result<Planned> {
        self.require_host(id).await?;
        let remote = self.remotes.open(id);
        let touched = remote.commit_files(sha).await?;
        let sha = sha.to_string();
        let parent = touched.parent.ok_or_else(|| {
            Error::Invalid(format!(
                "{} is the first commit and cannot be reverted",
                sha7(&sha)
            ))
        })?;
        let summary = remote
            .commits_between(&parent, &sha)
            .await?
            .into_iter()
            .find(|c| c.sha == sha)
            .map(|c| original_summary(&c.message))
            .unwrap_or_default();
        let head = remote.head().await?;
        let mut paths: Vec<String> = touched
            .paths
            .into_iter()
            .filter(|p| group::is_allowlisted(p))
            .collect();
        paths.sort();
        paths.dedup();

        let (mut files, mut expected, mut changes) = (Vec::new(), Vec::new(), Changes::new());
        for path in paths {
            let before = remote.file_at(&parent, &path).await?;
            let at_target = remote.file_at(&sha, &path).await?;
            if before == at_target {
                continue;
            }
            let action = match (&before, &at_target) {
                (None, _) => RevertAction::Delete,
                (_, None) => RevertAction::Recreate,
                _ => RevertAction::Restore,
            };
            let now = remote.file_at(&head.commit, &path).await?;
            let blocked = if now == at_target {
                None
            } else {
                Some(
                    self.changed_again(id, &sha, &head.commit, &path, &at_target)
                        .await?,
                )
            };
            if blocked.is_none() {
                changes.insert(path.clone(), before);
                expected.push((path.clone(), at_target));
            }
            files.push(RevertFile {
                path,
                action,
                blocked,
            });
        }
        Ok(Planned {
            plan: RevertPlan {
                sha,
                summary,
                files,
            },
            expected,
            changes,
        })
    }

    /// The message for a file a later commit changed: which commit, and by whom.
    async fn changed_again(
        &self,
        id: &GroupId,
        sha: &str,
        head: &str,
        path: &str,
        at_target: &Option<Vec<u8>>,
    ) -> crate::Result<String> {
        let remote = self.remotes.open(id);
        for c in remote.commits_between(sha, head).await? {
            if remote.file_at(&c.sha, path).await? != *at_target {
                return Ok(format!(
                    "{path} changed again after this commit, in {} by {}; revert that first.",
                    sha7(&c.sha),
                    c.author
                ));
            }
        }
        Ok(format!(
            "{path} changed again after this commit; revert that first."
        ))
    }
}
