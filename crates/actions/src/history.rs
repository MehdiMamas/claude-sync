//! What a group gives this device, its history with change notes, and reverting a commit
//! (Parity 34, 35). Revert and its preview are host only; a revert is a new commit.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{ContentItem, Event, HistoryItem, RevertFile, RevertPlan};
use crate::msg;
use claude_sync_engine::formats::group::{change_notes, CHANGE_NOTES_PATH};
use claude_sync_engine::paths::{allowed_by, is_planned_path};
use claude_sync_engine::secrets::scan;
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

/// Before and now from a commit's body (v1 `changeParts`): `Before:`/`Was:` and `Now:`/`After:`
/// lines (bullets allowed), else one line `X -> Y`. Nothing is made up when they are absent.
fn message_note(text: &str) -> (Option<String>, Option<String>) {
    let (mut before, mut now) = (None::<String>, None::<String>);
    let mut rest = vec![];
    let key = |l: &str, names: [&str; 2]| {
        let (k, v) = l.split_once(':')?;
        let k = k.trim().to_lowercase();
        (names.contains(&k.as_str()) && !v.trim().is_empty()).then(|| v.trim().to_string())
    };
    let text = text.replace("\r\n", "\n");
    for raw in text.lines().skip(1) {
        let l = raw.trim().trim_start_matches(['-', '*', '\u{2022}']).trim();
        if l.is_empty() {
            continue;
        }
        if before.is_none() && key(l, ["before", "was"]).is_some() {
            before = key(l, ["before", "was"]);
        } else if now.is_none() && key(l, ["now", "after"]).is_some() {
            now = key(l, ["now", "after"]);
        } else {
            rest.push(l);
        }
    }
    if before.is_none() && now.is_none() {
        for l in rest {
            let Some((x, y)) = l.split_once('\u{2192}').or_else(|| l.split_once("->")) else {
                continue;
            };
            let x = x.split_once(": ").map_or(x, |(_, r)| r).trim();
            let y = y.trim().trim_end_matches(['.', ';']);
            if !x.is_empty() && !y.is_empty() {
                return (Some(x.into()), Some(y.into()));
            }
        }
    }
    (before, now)
}

/// The first message line without the app's `login@device: ` prefix, kept short (v1).
fn original_summary(message: &str) -> String {
    let first = message.lines().next().unwrap_or("").trim();
    let text = match first.split_once(": ") {
        Some((who, rest)) if who.contains('@') && !who.contains(char::is_whitespace) => rest,
        _ => first,
    };
    let mut out: String = text.chars().take(72).collect();
    if text.chars().count() > 72 {
        out.push('\u{2026}');
    }
    out
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
            .map(|c| original_summary(&c.message))
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
        for (p, v) in &pushes {
            if let Some(h) = v
                .as_ref()
                .and_then(|b| scan(&String::from_utf8_lossy(b)).into_iter().next())
            {
                return Err(Error::new(msg::secret_found(p, h.line, h.pattern)));
            }
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
        assert_eq!(
            message_note("x\n- Was: a\n- After: b"),
            (Some("a".into()), Some("b".into()))
        );
        assert_eq!(
            message_note("x\nuse tabs -> use spaces."),
            (Some("use tabs".into()), Some("use spaces".into()))
        );
        assert_eq!(message_note("Before: not the subject"), (None, None));
        assert_eq!(
            original_summary("ann@mac: revert abc1234 (x)"),
            "revert abc1234 (x)"
        );
        assert_eq!((b.as_deref(), n.as_deref()), (Some("tabs"), Some("spaces")));
    }
}
