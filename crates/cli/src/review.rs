//! `review list|accept|undo`: what waits for the person, over all groups, with one id
//! per item. Ids are `md:<group>:<item>` (a change to the group's CLAUDE.md),
//! `edit:<group>:<path>` (a local edit to a managed file) and `job:<group>:<job>` (a
//! scheduled job to approve).
use crate::jobs::with_jobs;
use claude_sync_core::state::AppState;
use claude_sync_core::watch::EditAction;
use claude_sync_core::{Engine, GroupId, Paths};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Id {
    Md(GroupId, String),
    Edit(GroupId, String),
    Job(GroupId, String),
}

impl Id {
    pub fn parse(s: &str) -> Result<Id, String> {
        let bad =
            || format!("`{s}` is not a review id; take one from `claude-sync-cli review list`");
        let mut parts = s.splitn(3, ':');
        let (Some(kind), Some(group), Some(rest)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        let group = GroupId::parse(group).ok_or_else(bad)?;
        if rest.is_empty() {
            return Err(bad());
        }
        match kind {
            "md" => Ok(Id::Md(group, rest.into())),
            "edit" => Ok(Id::Edit(group, rest.into())),
            "job" => Ok(Id::Job(group, rest.into())),
            _ => Err(bad()),
        }
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Id::Md(g, i) => write!(f, "md:{g}:{i}"),
            Id::Edit(g, i) => write!(f, "edit:{g}:{i}"),
            Id::Job(g, i) => write!(f, "job:{g}:{i}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub kind: &'static str,
    pub group: String,
    /// One line saying what it is.
    pub summary: String,
    /// What accept and undo do to it.
    pub accept: &'static str,
    pub undo: &'static str,
}

pub async fn list(engine: &Engine, paths: &Paths) -> Result<Vec<Entry>, String> {
    let state = AppState::load(paths).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for g in &state.groups {
        let review = engine.claude_md_review(g).map_err(|e| e.to_string())?;
        for item in review.pending {
            let verb = match item.kind {
                claude_sync_core::review::Kind::Add => "adds",
                claude_sync_core::review::Kind::Remove => "removes",
            };
            let who = if item.author.is_empty() {
                String::new()
            } else {
                format!(" ({})", item.author)
            };
            out.push(Entry {
                id: Id::Md(g.clone(), item.id.clone()).to_string(),
                kind: "claude-md",
                group: g.to_string(),
                summary: format!(
                    "CLAUDE.md change{who} {verb}: {}",
                    item.lines.first().map(String::as_str).unwrap_or("")
                ),
                accept: "put it in your CLAUDE.md copy",
                undo: "leave it out, and do not ask again",
            });
        }
    }
    for e in engine.local_edits().map_err(|e| e.to_string())? {
        out.push(Entry {
            id: Id::Edit(e.group.clone(), e.path.clone()).to_string(),
            kind: "edit",
            group: e.group.to_string(),
            summary: format!(
                "you {} {} here",
                if e.deleted { "deleted" } else { "changed" },
                e.path
            ),
            accept: "share it with the group",
            undo: "put the group's version back",
        });
    }
    for g in &state.groups {
        let overview = with_jobs(paths, |j| j.overview(g))?;
        for j in overview.jobs.into_iter().filter(|j| !j.accepted) {
            out.push(Entry {
                id: Id::Job(g.clone(), j.id.clone()).to_string(),
                kind: "job",
                group: g.to_string(),
                summary: format!(
                    "{} job \"{}\", {}, runs claude in {}",
                    if j.changed { "changed" } else { "new" },
                    j.title,
                    j.schedule,
                    j.cwd
                ),
                accept: "let it run on schedule",
                undo: "keep it off",
            });
        }
    }
    Ok(out)
}

/// Accept (`accept`) or undo one item. Returns a sentence saying what was done.
pub async fn decide(
    engine: &Engine,
    paths: &Paths,
    id: &Id,
    accept: bool,
) -> Result<String, String> {
    match id {
        Id::Md(g, item) => {
            let r = engine.claude_md_review(g).map_err(|e| e.to_string())?;
            if !r.pending.iter().any(|i| i.id == *item) {
                return Err(format!("{id} is not waiting any more"));
            }
            engine
                .decide_claude_md(g, Some(std::slice::from_ref(item)), accept)
                .await
                .map_err(|e| e.to_string())?;
            Ok(if accept {
                format!("Accepted the CLAUDE.md change in {g}.")
            } else {
                format!("Declined the CLAUDE.md change in {g}.")
            })
        }
        Id::Edit(g, path) => {
            let edit = engine
                .local_edits()
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|e| e.group == *g && e.path == *path)
                .ok_or_else(|| format!("{id} is not waiting any more"))?;
            let action = if accept {
                EditAction::Share
            } else {
                EditAction::Undo
            };
            engine
                .resolve_edit(&edit, action)
                .await
                .map_err(|e| e.to_string())?;
            Ok(if accept {
                format!("Shared your change to {path} with {g}.")
            } else {
                format!("Put the group's {path} back.")
            })
        }
        Id::Job(g, job) => {
            let view = with_jobs(paths, |j| j.overview(g))?
                .jobs
                .into_iter()
                .find(|j| j.id == *job)
                .ok_or_else(|| format!("{id} is not a job of {g}"))?;
            if accept {
                with_jobs(paths, |j| j.accept(g, job, &view.hash))?;
                Ok(format!("Job \"{}\" will run on schedule.", view.title))
            } else if view.accepted || view.changed {
                with_jobs(paths, |j| j.set_enabled(g, job, false))?;
                Ok(format!("Job \"{}\" is turned off.", view.title))
            } else {
                Ok(format!(
                    "Job \"{}\" stays off: it was never accepted.",
                    view.title
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for s in [
            "md:me/team:a1b2",
            "edit:me/team:settings.shared.json#/permissions/allow",
            "job:me/team:nightly",
            "edit:me/team:agents/x:y.md",
        ] {
            assert_eq!(Id::parse(s).unwrap().to_string(), s);
        }
        assert_eq!(
            Id::parse("edit:me/team:agents/x:y.md").unwrap(),
            Id::Edit(GroupId::new("me", "team"), "agents/x:y.md".into())
        );
    }

    #[test]
    fn bad_ids_are_refused() {
        for s in [
            "",
            "md",
            "md:me/team",
            "md:me/team:",
            "md:team:x",
            "zzz:me/team:x",
        ] {
            assert!(Id::parse(s).is_err(), "{s}");
        }
    }
}
