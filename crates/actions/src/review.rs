//! CLAUDE.md review (Parity 37, 38, 82): the engine's [`ReviewState`] per group, kept in the
//! store as JSON in the v1 shape (format 2). The sync pass asks `gate` for the text it may
//! import; the commands here list, accept, decline, drop and reset.
//!
//! A pass sees the head's text and the author of the head commit as one step; several commits
//! between two passes therefore arrive as one step by the newest author.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{ClashSide, ClashView, Event, ReviewLine};
use crate::msg;
use claude_sync_engine::clashes::{clashes, settings_clashes, SettingsFile, Text};
use claude_sync_engine::claude_md::strip;
use claude_sync_engine::review::{notification, Item, Kind, ReviewState, Step};
use claude_sync_engine::traits::{Remote, Store};
use claude_sync_fs::pass::Gated;
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
    pub(crate) fn gate(&self, repo: &str) -> Box<claude_sync_fs::pass::Gate<'_>> {
        let review = self.review_on(repo);
        let repo = repo.to_string();
        Box::new(move |md: Option<&[u8]>| {
            let text = String::from_utf8_lossy(md?).into_owned();
            let me = self.login.clone().unwrap_or_default();
            let now = self.now();
            let Some(mut st) = load(&self.db, &repo) else {
                let st = ReviewState::new(&text);
                save(&self.db, &repo, &st);
                let b = st.accepted.into_bytes();
                return Some(Gated {
                    before: b.clone(),
                    accepted: b,
                    applied: vec![],
                });
            };
            st.review = Some(review);
            let before = st.accepted.clone();
            let mut applied = vec![];
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
                applied = pulled.applied.clone();
                if let Some(t) = notification(&repo, &pulled.new) {
                    let _ = self.db.add_notification("review", &t, Some(&repo), now);
                }
            }
            if !review {
                applied.extend(st.accept(None));
            }
            save(&self.db, &repo, &st);
            Some(Gated {
                accepted: st.accepted.into_bytes(),
                before: before.into_bytes(),
                applied,
            })
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
    /// `kept`: `(accepted text before, changes applied)`: an edited copy keeps its edit and gets
    /// those changes; `None` replaces the copy (the old one is backed up).
    pub(crate) fn rewrite_copy(
        &self,
        repo: &str,
        st: &ReviewState,
        kept: Option<(&str, &[Item])>,
    ) -> Result<()> {
        save(&self.db, repo, st);
        claude_sync_fs::apply::write_claude_md_copy(
            &self.home,
            repo,
            Some(st.accepted.as_bytes()),
            kept.map(|k| k.0.as_bytes()),
            kept.map_or(&[], |k| k.1),
            self.now(),
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
                    at: i.at_unix,
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
        let before = st.accepted.clone();
        let mut applied = vec![];
        if accept {
            applied = st.accept(ids.as_deref());
        } else {
            st.decline(ids.as_deref(), self.now() as u64);
        }
        self.rewrite_copy(repo, &st, Some((&before, &applied)))?;
        Ok(Event::Done)
    }

    pub(crate) fn set_review_choice(
        &mut self,
        repo: &str,
        choice: crate::choices::ReviewChoice,
    ) -> Result<Event> {
        use crate::choices::ReviewChoice::*;
        let mut c = self.choices();
        c.group(repo, self.now() as u64 * 1000).review_claude_md = match choice {
            FollowGroup => None,
            ReviewFirst => Some(true),
            AutoAccept => Some(false),
        };
        c.save(&self.db)?;
        Ok(Event::Done)
    }

    pub(crate) fn drop_line(&mut self, repo: &str, line: &str) -> Result<Event> {
        let mut st = self.state(repo)?;
        let now = self.now();
        let before = st.accepted.clone();
        let item = st
            .drop_line(line, now as u64)
            .ok_or_else(|| Error::new(msg::group_line_gone(repo)))?;
        let mut c = self.choices();
        let g = c.group(repo, now as u64 * 1000);
        if !g.dropped_lines.iter().any(|l| l == line.trim()) {
            g.dropped_lines.push(line.trim().to_string());
        }
        c.save(&self.db)?;
        self.rewrite_copy(repo, &st, Some((&before, &[item])))?;
        Ok(Event::Done)
    }

    /// Parity 38 as v1 did it: a group line is replaced in the group's own CLAUDE.md and
    /// committed to the group (a proposal in proposal mode), then synced; the own text is
    /// rewritten on this machine.
    pub(crate) fn edit_line(
        &mut self,
        source: Option<&str>,
        line: &str,
        text: &str,
    ) -> Result<Event> {
        let text = claude_sync_fs::redact::redact(&text.replace("\r\n", "\n"));
        let text = text.trim_matches('\n');
        let Some(repo) = source else {
            let done = claude_sync_fs::apply::replace_own_line(&self.home, line, text, self.now())?;
            return if done {
                Ok(Event::Done)
            } else {
                Err(Error::new(msg::own_line_gone()))
            };
        };
        self.me()?;
        let flags = crate::groups::cached(&self.db, repo)
            .map(|f| f.sync)
            .unwrap_or_default();
        if !flags.claude_md {
            return Err(Error::new(msg::does_not_sync(repo, "CLAUDE.md")));
        }
        let head = self
            .remote(repo)?
            .head(None)?
            .ok_or_else(|| Error::new(msg::not_a_group(repo)))?;
        let old = self
            .remote(repo)?
            .files(&head.sha, &Default::default())?
            .remove("CLAUDE.md")
            .unwrap_or_default();
        let old = String::from_utf8_lossy(&old).replace("\r\n", "\n");
        let lines: Vec<&str> = old.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.trim() == line.trim())
            .ok_or_else(|| Error::new(msg::group_line_gone(repo)))?;
        let mut out: Vec<&str> = lines[..at].to_vec();
        out.extend(text.lines());
        out.extend(&lines[at + 1..]);
        let new = format!("{}\n", out.join("\n"));
        let ev = self.push_files_to(
            repo,
            [("CLAUDE.md".to_string(), new.into_bytes())].into(),
            "edit CLAUDE.md",
            true,
        )?;
        if ev == Event::Done {
            self.run_pass()?;
        }
        Ok(ev)
    }

    pub(crate) fn use_group_version(&mut self, repo: &str) -> Result<Event> {
        let mut st = self.state(repo)?;
        st.reset();
        let mut c = self.choices();
        c.group(repo, self.now() as u64 * 1000)
            .dropped_lines
            .clear();
        c.save(&self.db)?;
        self.rewrite_copy(repo, &st, None)?;
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
        let side = |s: claude_sync_engine::clashes::Side| ClashSide {
            source: s.source,
            name: s.name,
            text: s.text,
        };
        // settings two groups set differently, in rank order (the earlier group applies)
        let mut files = vec![];
        for g in self.db.groups()?.into_iter().filter(|g| !g.off) {
            let on = crate::groups::cached(&self.db, &g.repo).is_none_or(|f| f.sync.settings);
            let json = std::fs::read(claude_sync_fs::apply::copy_path(
                &self.home,
                &g.repo,
                "settings.shared.json",
            ))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
            if let (true, Some(json)) = (on, json) {
                files.push(SettingsFile {
                    name: g.repo.rsplit('/').next().unwrap_or(&g.repo).into(),
                    source: g.repo,
                    json,
                });
            }
        }
        let mut all = clashes(&texts);
        all.extend(settings_clashes(&files));
        Ok(Event::Clashes(
            all.into_iter()
                .map(|c| ClashView {
                    kind: format!("{:?}", c.kind).to_lowercase(),
                    target: c.target,
                    a: side(c.a),
                    b: side(c.b),
                })
                .collect(),
        ))
    }
}
