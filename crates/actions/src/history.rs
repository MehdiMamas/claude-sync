//! What a group gives this device, its history with change notes, and reverting a commit
//! (Parity 34, 35). Revert and its preview are host only; a revert is a new commit.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{ContentItem, Event, HistoryItem, RevertFile, RevertPlan};
use crate::msg;
use claude_sync_engine::formats::group::{change_notes, CHANGE_NOTES_PATH};
use claude_sync_engine::paths::{allowed_by, is_planned_path};
use claude_sync_engine::traits::Remote;
use std::collections::{BTreeMap, BTreeSet};

type Pushes = BTreeMap<String, Option<Vec<u8>>>;

/// `#123` chips in a commit text.
fn issue_chips(text: &str) -> Vec<u64> {
    let mut out = vec![];
    let b = text.as_bytes();
    for (i, _) in text.match_indices('#') {
        let digits: String = text[i + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let word_start = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if let (true, Ok(n)) = (word_start, digits.parse::<u64>()) {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// The `Before:` and `Now:` lines of a commit text.
fn message_note(text: &str) -> (Option<String>, Option<String>) {
    let line = |key: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(key))
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    (line("Before:"), line("Now:"))
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

impl Core {
    pub(crate) fn history(&mut self, repo: &str, limit: u32) -> Result<Event> {
        let (o, n) = crate::groups::split(repo)?;
        let list = self.client.commits(o, n, "main", limit.clamp(1, 20))?;
        let mut notes = None;
        let mut out = vec![];
        for c in &list {
            let (before, now) = message_note(&c.commit.message);
            let (before, now) = if before.is_some() || now.is_some() {
                (before, now)
            } else {
                let all = notes.get_or_insert_with(|| {
                    let head = list.first().map(|c| c.sha.clone()).unwrap_or_default();
                    self.read_notes(o, n, &head)
                });
                all.get(&c.sha)
                    .map(|x| (x.before.clone(), x.now.clone()))
                    .unwrap_or_default()
            };
            out.push(HistoryItem {
                sha: c.sha.clone(),
                summary: c.commit.message.lines().next().unwrap_or("").to_string(),
                author: c
                    .author
                    .as_ref()
                    .map(|a| a.login.clone())
                    .or_else(|| c.commit.author.as_ref().map(|a| a.name.clone()))
                    .unwrap_or_default(),
                at: c.commit.author.as_ref().and_then(|a| a.date.clone()),
                issues: issue_chips(&c.commit.message),
                before,
                now,
            });
        }
        Ok(Event::History(out))
    }

    fn read_notes(
        &self,
        o: &str,
        n: &str,
        head: &str,
    ) -> BTreeMap<String, claude_sync_engine::formats::group::ChangeNote> {
        let Ok(tree) = self.client.tree(o, n, head, true) else {
            return BTreeMap::new();
        };
        let Some(sha) = tree
            .tree
            .iter()
            .find(|e| e.path == CHANGE_NOTES_PATH)
            .and_then(|e| e.sha.clone())
        else {
            return BTreeMap::new();
        };
        self.client
            .blob(o, n, &sha)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .map(|v| change_notes(&v))
            .unwrap_or_default()
    }

    /// Kinds and names of what the group delivered at the last sync.
    pub(crate) fn contents(&mut self, repo: &str) -> Result<Event> {
        let base = self.base(repo)?;
        let mut seen = BTreeSet::new();
        let mut out = vec![];
        // CLAUDE.md is delivered as a copy next to the state, not into the base
        if claude_sync_fs::apply::copy_path(&self.home, repo, "CLAUDE.md").is_file() {
            seen.insert(("claudeMd", "CLAUDE.md".to_string()));
            out.push(ContentItem {
                kind: "claudeMd".into(),
                name: "CLAUDE.md".into(),
            });
        }
        for p in base.files.keys() {
            let mut it = p.splitn(3, '/');
            let (top, name) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            let (kind, name) = match top {
                "CLAUDE.md" => ("claudeMd", "CLAUDE.md"),
                "skills" => ("skill", name),
                "agents" => ("agent", name),
                "commands" => ("command", name),
                "output-styles" => ("outputStyle", name),
                "plugins" => ("plugin", name),
                "settings.shared.json" => ("settings", "settings.shared.json"),
                "schedules.json" => ("jobs", "schedules.json"),
                _ => continue,
            };
            if seen.insert((kind, name.to_string())) {
                out.push(ContentItem {
                    kind: kind.into(),
                    name: name.into(),
                });
            }
        }
        Ok(Event::Contents(out))
    }

    /// The plan and what to push; nothing is written.
    fn revert_work(&self, repo: &str, sha: &str) -> Result<(RevertPlan, Pushes, String)> {
        self.require_host(repo)?;
        let (o, n) = crate::groups::split(repo)?;
        let detail = self.client.commit_detail(o, n, sha)?;
        let Some(parent) = detail.parents.first().map(|p| p.sha.clone()) else {
            return Err(Error::new(msg::first_commit(short(sha))));
        };
        let flags = crate::groups::cached(&self.db, repo)
            .map(|g| g.sync)
            .unwrap_or_default();
        let remote = self.remote(repo)?;
        let head = remote
            .head(None)?
            .ok_or_else(|| Error::new(msg::not_a_group(repo)))?;
        let none = BTreeMap::new();
        let (at_parent, at_target, at_head) = (
            remote.files(&parent, &none)?,
            remote.files(&detail.sha, &none)?,
            remote.files(&head.sha, &none)?,
        );
        let mut files = vec![];
        let mut pushes = BTreeMap::new();
        let touched: BTreeSet<&String> = detail.files.iter().map(|f| &f.filename).collect();
        for p in touched {
            if !is_planned_path(p) || !allowed_by(&flags, p) {
                continue;
            }
            let (before, target, now) = (at_parent.get(p), at_target.get(p), at_head.get(p));
            if before == target {
                continue;
            }
            files.push(RevertFile {
                path: p.clone(),
                action: match (before, target) {
                    (None, _) => "delete",
                    (_, None) => "recreate",
                    _ => "restore",
                }
                .into(),
                blocked: (now != target).then(|| msg::changed_again(p)),
            });
            pushes.insert(p.clone(), before.cloned());
        }
        if files.is_empty() {
            return Err(Error::new(msg::nothing_to_revert(short(sha))));
        }
        let summary = detail
            .commit
            .and_then(|c| c.message.lines().next().map(String::from))
            .unwrap_or_default();
        let plan = RevertPlan {
            sha: detail.sha,
            summary,
            files,
        };
        Ok((plan, pushes, head.sha))
    }

    pub(crate) fn revert_preview(&mut self, repo: &str, sha: &str) -> Result<Event> {
        Ok(Event::RevertPlan(self.revert_work(repo, sha)?.0))
    }

    pub(crate) fn revert(&mut self, repo: &str, sha: &str) -> Result<Event> {
        let (plan, pushes, head) = self.revert_work(repo, sha)?;
        if let Some(b) = plan.files.iter().find_map(|f| f.blocked.clone()) {
            return Err(Error::new(b));
        }
        let me = self.me()?;
        let note = format!(
            "{me}@{}: revert {} ({})",
            self.env.device,
            short(&plan.sha),
            plan.summary
        );
        self.remote(repo)?.push(&head, &pushes, &note)?;
        Ok(Event::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_and_notes() {
        assert_eq!(issue_chips("fix #12 and #7, not a#3, #12"), vec![12, 7]);
        let (b, n) = message_note("x\n\nBefore: tabs\nNow: spaces\n");
        assert_eq!((b.as_deref(), n.as_deref()), (Some("tabs"), Some("spaces")));
    }
}
