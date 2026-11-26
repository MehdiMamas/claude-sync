//! CLAUDE.md review (Parity 37, 38, 82): the engine's [`ReviewState`] per group, kept in the
//! store as JSON in the v1 shape (format 2). The sync pass asks `gate` for the text it may
//! import; the commands here list, accept, decline, drop and reset.
//!
//! A pass sees the head's text and the author of the head commit as one step; several commits
//! between two passes therefore arrive as one step by the newest author.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{Event, ReviewLine};
use crate::msg;
use claude_sync_engine::clashes::{clashes, Text};
use claude_sync_engine::claude_md::strip;
use claude_sync_engine::review::{notification, Kind, ReviewState, Step};
use claude_sync_engine::traits::Store;
use claude_sync_store::Db;

fn key(repo: &str) -> String {
    format!("review/{repo}")
}

pub(crate) fn load(db: &Db, repo: &str) -> Option<ReviewState> {
    Store::read(db, &key(repo))
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

pub(crate) fn save(db: &Db, repo: &str, s: &ReviewState) {
    if let Ok(b) = serde_json::to_vec(s) {
        let _ = Store::write(db, &key(repo), &b);
    }
}

/// Forget the review of a group (leave).
pub(crate) fn clear(db: &Db, repo: &str) {
    let _ = Store::write(db, &key(repo), b"");
    let _ = Store::write(db, &format!("group-file/{repo}"), b"");
    let _ = Store::write(db, &format!("kept-edits/{repo}"), b"");
}

impl Core {
    /// Review on for this group: the person's choice, else the opposite of the group's
    /// autoAcceptClaudeMd (auto-accept is the default, so review is off by default).
    fn review_on(&self, repo: &str) -> bool {
        self.choices()
            .groups
            .get(repo)
            .and_then(|g| g.review_claude_md)
            .unwrap_or_else(|| {
                !crate::groups::cached(&self.db, repo).is_none_or(|f| f.auto_accept_claude_md)
            })
    }

    /// The gate for one group's pass: text in, accepted text out.
    pub(crate) fn gate(&self, repo: &str) -> Box<dyn Fn(Option<&[u8]>) -> Option<Vec<u8>> + '_> {
        let review = self.review_on(repo);
        let repo = repo.to_string();
        Box::new(move |md: Option<&[u8]>| {
            let text = String::from_utf8_lossy(md?).into_owned();
            let me = self.login.clone().unwrap_or_default();
            let now = self.now();
            let Some(mut st) = load(&self.db, &repo) else {
                let st = ReviewState::new(&text);
                save(&self.db, &repo, &st);
                return Some(st.accepted.into_bytes());
            };
            st.review = Some(review);
            if text != st.group {
                let (author, message, commit) = self.head_commit(&repo);
                let step = Step {
                    text,
                    mine: !me.is_empty() && author == me,
                    author,
                    commit,
                    message,
                    at_unix: now as u64,
                };
                let pulled = st.pull(&[step]);
                if let Some(t) = notification(&repo, &pulled.new) {
                    let _ = self.db.add_notification("review", &t, Some(&repo), now);
                }
            }
            if !review {
                st.accept(None);
            }
            save(&self.db, &repo, &st);
            Some(st.accepted.into_bytes())
        })
    }

    fn head_commit(&self, repo: &str) -> (String, String, String) {
        let Some((o, n)) = repo.split_once('/') else {
            return Default::default();
        };
        let c = self
            .client
            .commits(o, n, "main", 1)
            .ok()
            .and_then(|v| v.into_iter().next());
        match c {
            Some(c) => (
                c.author.map(|a| a.login).unwrap_or_default(),
                c.commit.message.lines().next().unwrap_or("").to_string(),
                c.sha,
            ),
            None => Default::default(),
        }
    }

    fn state(&self, repo: &str) -> Result<ReviewState> {
        load(&self.db, repo).ok_or_else(|| Error::new(msg::no_claude_md_to_review(repo)))
    }

    /// Write the accepted text where Claude reads it and refresh the block.
    fn rewrite_copy(&self, repo: &str, st: &ReviewState) -> Result<()> {
        save(&self.db, repo, st);
        claude_sync_fs::apply::write_claude_md_copy(
            &self.home,
            repo,
            Some(st.accepted.as_bytes()),
        )?;
        let order: Vec<(String, bool)> = self
            .db
            .groups()?
            .into_iter()
            .map(|g| (g.repo, g.off))
            .collect();
        claude_sync_fs::apply::update_block(&self.home, &order, self.now())?;
        Ok(())
    }

    pub(crate) fn review_list(&self, repo: &str) -> Result<Event> {
        let st = self.state(repo)?;
        Ok(Event::Review(
            st.pending
                .iter()
                .map(|i| ReviewLine {
                    id: i.id.clone(),
                    add: i.kind == Kind::Add,
                    lines: i.lines.clone(),
                    author: i.author.clone(),
                })
                .collect(),
        ))
    }

    pub(crate) fn review_decide(
        &mut self,
        repo: &str,
        ids: Option<Vec<String>>,
        accept: bool,
    ) -> Result<Event> {
        let mut st = self.state(repo)?;
        if accept {
            st.accept(ids.as_deref());
        } else {
            st.decline(ids.as_deref(), self.now() as u64);
        }
        self.rewrite_copy(repo, &st)?;
        Ok(Event::Done)
    }

    pub(crate) fn drop_line(&mut self, repo: &str, line: &str) -> Result<Event> {
        let mut st = self.state(repo)?;
        let now = self.now();
        st.drop_line(line, now as u64)
            .ok_or_else(|| Error::new(msg::group_line_gone(repo)))?;
        let mut c = self.choices();
        let g = c.group(repo, now as u64 * 1000);
        if !g.dropped_lines.iter().any(|l| l == line.trim()) {
            g.dropped_lines.push(line.trim().to_string());
        }
        c.save(&self.db)?;
        self.rewrite_copy(repo, &st)?;
        Ok(Event::Done)
    }

    pub(crate) fn use_group_version(&mut self, repo: &str) -> Result<Event> {
        let mut st = self.state(repo)?;
        st.reset();
        let mut c = self.choices();
        c.group(repo, self.now() as u64 * 1000)
            .dropped_lines
            .clear();
        c.save(&self.db)?;
        self.rewrite_copy(repo, &st)?;
        Ok(Event::Done)
    }

    /// Lines that clash between the person's own CLAUDE.md and the accepted group texts.
    pub(crate) fn clashes(&self) -> Result<Event> {
        let mut texts = vec![];
        if let Ok(own) = std::fs::read(self.home.claude.join("CLAUDE.md")) {
            if let Some(t) = strip(&own) {
                texts.push(Text {
                    source: None,
                    name: "Your CLAUDE.md".into(),
                    text: String::from_utf8_lossy(&t).into_owned(),
                });
            }
        }
        for g in self.db.groups()?.into_iter().filter(|g| !g.off) {
            if let Some(st) = load(&self.db, &g.repo) {
                texts.push(Text {
                    name: g.repo.rsplit('/').next().unwrap_or(&g.repo).into(),
                    source: Some(g.repo),
                    text: st.accepted,
                });
            }
        }
        Ok(Event::Clashes(clashes(&texts)))
    }
}
