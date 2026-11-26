//! Copy to another group (Parity 33): the source's last synced CLAUDE.md text (redacted),
//! skills and agents go into the target's repo in one commit, or one proposal when the target
//! is in proposal mode. Needs push access to the target. Nothing is applied here: the next
//! sync delivers the files, so the base of the target is left alone.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{CopyItem, CopyPreviewView, CopyRequest, Event};
use crate::msg;
use claude_sync_engine::plan::FileSet;
use claude_sync_engine::secrets::{scan, ALLOW_MARKER};
use claude_sync_engine::traits::Remote;
use claude_sync_fs::redact::redact;
use std::collections::{BTreeMap, BTreeSet};

fn squash(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Files of `skills/<name>/` or `agents/<name>` in `base`.
fn pick(base: &FileSet, dir: &str, name: &str) -> FileSet {
    let (exact, prefix) = (format!("{dir}/{name}"), format!("{dir}/{name}/"));
    base.iter()
        .filter(|(p, _)| **p == exact || p.starts_with(&prefix))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}

fn names(base: &FileSet, dir: &str) -> BTreeSet<String> {
    let pre = format!("{dir}/");
    base.keys()
        .filter_map(|p| p.strip_prefix(&pre))
        .map(|r| r.split('/').next().unwrap_or(r).to_string())
        .collect()
}

impl Core {
    fn copy_ends(&self, from: &str, to: &str) -> Result<(FileSet, FileSet)> {
        if from == to {
            return Err(Error::new(msg::copy_same_group()));
        }
        Ok((self.base(from)?.files, self.base(to)?.files))
    }

    pub(crate) fn copy_preview(&mut self, from: &str, to: &str) -> Result<Event> {
        let (src, dst) = self.copy_ends(from, to)?;
        let (o, n) = crate::groups::split(to)?;
        let can_push = self
            .client
            .repo(o, n)
            .map(|r| r.permissions.is_none_or(|p| p.push))
            .unwrap_or(true);
        let g = crate::groups::cached(&self.db, to);
        let flags = g.as_ref().map(|g| g.sync.clone()).unwrap_or_default();
        let items = |dir: &str| {
            names(&src, dir)
                .into_iter()
                .map(|name| {
                    let mine = pick(&src, dir, &name);
                    let theirs = pick(&dst, dir, &name);
                    CopyItem {
                        in_target: !theirs.is_empty(),
                        same: mine == theirs,
                        name,
                    }
                })
                .collect()
        };
        Ok(Event::CopyPreview(CopyPreviewView {
            to: to.into(),
            to_name: g.map(|g| g.name).unwrap_or_else(|| to.into()),
            host: o.into(),
            can_push,
            syncs_claude_md: flags.claude_md,
            syncs_skills: flags.skills,
            syncs_agents: flags.agents,
            claude_md: std::fs::read(claude_sync_fs::apply::copy_path(
                &self.home,
                from,
                "CLAUDE.md",
            ))
            .map(|b| redact(&String::from_utf8_lossy(&b)))
            .unwrap_or_default(),
            skills: items("skills"),
            agents: items("agents"),
        }))
    }

    pub(crate) fn copy(&mut self, from: &str, to: &str, req: CopyRequest) -> Result<Event> {
        let Event::CopyPreview(pv) = self.copy_preview(from, to)? else {
            return Err(Error::new("copy failed"));
        };
        if !pv.can_push {
            return Err(Error::new(msg::no_push(&pv.to_name, &pv.host)));
        }
        let (src, dst) = self.copy_ends(from, to)?;
        let from_name = crate::groups::cached(&self.db, from)
            .map(|g| g.name)
            .unwrap_or_else(|| from.into());
        let mut put = FileSet::new();
        let mut summary = vec![];
        let text = redact(req.claude_md.trim());
        let mut claude_md = false;
        if !text.is_empty() {
            if !pv.syncs_claude_md {
                return Err(Error::new(msg::does_not_sync(to, "CLAUDE.md")));
            }
            // the target's newest text, not the last synced one
            let remote = self.remote(to)?;
            let head = remote
                .head(None)?
                .ok_or_else(|| Error::new(msg::not_a_group(to)))?;
            let old = remote
                .files(&head.sha, &BTreeMap::new())?
                .remove("CLAUDE.md")
                .unwrap_or_default();
            let old = String::from_utf8_lossy(&old).replace("\r\n", "\n");
            if !squash(&old).contains(&squash(&text)) {
                let new = match old.trim_end() {
                    "" => text.clone(),
                    o => format!("{o}\n\n## {from_name}\n\n{text}"),
                };
                put.insert("CLAUDE.md".into(), new.into_bytes());
                claude_md = true;
                summary.push("CLAUDE.md".to_string());
            }
        }
        for (dir, what, list, ok) in [
            ("skills", "skill", &req.skills, pv.syncs_skills),
            ("agents", "agent", &req.agents, pv.syncs_agents),
        ] {
            for name in list {
                let mine = pick(&src, dir, name);
                if mine.is_empty() {
                    return Err(Error::new(msg::no_such_item(from, what, name)));
                }
                if !ok {
                    return Err(Error::new(msg::does_not_sync(to, dir)));
                }
                let theirs = pick(&dst, dir, name);
                if theirs == mine {
                    continue;
                }
                if !theirs.is_empty() {
                    return Err(Error::new(msg::different_item(to, what, name)));
                }
                put.extend(mine);
                summary.push(format!("{dir}/{name}"));
            }
        }
        for (p, b) in &put {
            let t = String::from_utf8_lossy(b);
            if !t.contains(ALLOW_MARKER) {
                if let Some(h) = scan(&t).first() {
                    return Err(Error::new(msg::secret_found(p, h.line, h.pattern)));
                }
            }
        }
        let files = put.len();
        if files == 0 {
            return Ok(Event::Copied {
                files: 0,
                claude_md: false,
                proposed: None,
            });
        }
        let what = format!("copy from {from}: {}", summary.join(", "));
        let proposed = match self.push_files_to(to, put, &what, false)? {
            Event::Proposed(n) => Some(n),
            _ => None,
        };
        Ok(Event::Copied {
            files,
            claude_md,
            proposed,
        })
    }
}
