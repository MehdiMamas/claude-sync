//! The `groups` and `notifications` tables as the actor needs them: rows in the person's
//! order, off switch, state text, a rename that moves every child row.
use crate::{Db, Result};
use rusqlite::params;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRec {
    pub repo: String,
    pub host: bool,
    pub state: String,
    pub off: bool,
    pub renamed_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRec {
    pub id: i64,
    pub kind: String,
    pub text: String,
    pub read: bool,
    pub time: i64,
    pub repo: Option<String>,
}

impl Db {
    /// Groups in the person's order.
    pub fn groups(&self) -> Result<Vec<GroupRec>> {
        let conn = self.lock();
        let mut st = conn.prepare(
            "SELECT repo, role, state, off, renamed_from FROM groups ORDER BY order_idx, repo",
        )?;
        let rows = st.query_map([], |r| {
            Ok(GroupRec {
                repo: r.get(0)?,
                host: r.get::<_, String>(1)? == "host",
                state: r.get(2)?,
                off: r.get::<_, i64>(3)? != 0,
                renamed_from: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Joined last; a repo already joined keeps its row (its role is updated).
    pub fn add_group(&self, repo: &str, host: bool) -> Result<()> {
        self.lock().execute(
            "INSERT INTO groups (repo, role, state, order_idx)
             VALUES (?1, ?2, 'active', (SELECT COALESCE(MAX(order_idx) + 1, 0) FROM groups))
             ON CONFLICT (repo) DO UPDATE SET role = excluded.role",
            params![repo, if host { "host" } else { "member" }],
        )?;
        Ok(())
    }

    /// The group and everything kept for it (bases, review items, jobs) go.
    pub fn remove_group(&self, repo: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM groups WHERE repo = ?1", [repo])?;
        Ok(())
    }

    pub fn set_group_off(&self, repo: &str, off: bool) -> Result<()> {
        self.lock().execute(
            "UPDATE groups SET off = ?2 WHERE repo = ?1",
            params![repo, off as i64],
        )?;
        Ok(())
    }

    pub fn set_group_state(&self, repo: &str, state: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE groups SET state = ?2 WHERE repo = ?1",
            params![repo, state],
        )?;
        Ok(())
    }

    pub fn set_group_role(&self, repo: &str, host: bool) -> Result<()> {
        self.lock().execute(
            "UPDATE groups SET role = ?2 WHERE repo = ?1",
            params![repo, if host { "host" } else { "member" }],
        )?;
        Ok(())
    }

    /// Order is exactly `repos`; the caller checked the list holds every joined group.
    pub fn set_group_order(&self, repos: &[String]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        for (i, r) in repos.iter().enumerate() {
            tx.execute(
                "UPDATE groups SET order_idx = ?2 WHERE repo = ?1",
                params![r, i as i64],
            )?;
        }
        Ok(tx.commit()?)
    }

    /// The repo was renamed or transferred on GitHub: every row follows it.
    pub fn rename_group(&self, old: &str, new: &str) -> Result<()> {
        if old == new {
            return Ok(());
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.pragma_update(None, "defer_foreign_keys", true)?;
        tx.execute(
            "UPDATE groups SET repo = ?2, renamed_from = ?1 WHERE repo = ?1",
            params![old, new],
        )?;
        for t in ["bases", "base_files", "review_items", "jobs"] {
            tx.execute(
                &format!("UPDATE {t} SET repo = ?2 WHERE repo = ?1"),
                params![old, new],
            )?;
        }
        Ok(tx.commit()?)
    }

    /// Newest first.
    pub fn notifications(&self, limit: u32) -> Result<Vec<NotificationRec>> {
        let conn = self.lock();
        let mut st = conn.prepare(
            "SELECT id, kind, text, read, time, repo FROM notifications ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = st.query_map([limit], |r| {
            Ok(NotificationRec {
                id: r.get(0)?,
                kind: r.get(1)?,
                text: r.get(2)?,
                read: r.get::<_, i64>(3)? != 0,
                time: r.get(4)?,
                repo: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn unread_notifications(&self) -> Result<u32> {
        Ok(self.lock().query_row(
            "SELECT COUNT(*) FROM notifications WHERE read = 0",
            [],
            |r| r.get(0),
        )?)
    }

    /// Whether a notification with this text exists unread (so a repeated notice is not added).
    pub fn has_unread(&self, kind: &str, text: &str) -> Result<bool> {
        Ok(self.lock().query_row(
            "SELECT COUNT(*) FROM notifications WHERE read = 0 AND kind = ?1 AND text = ?2",
            params![kind, text],
            |r| r.get::<_, i64>(0),
        )? > 0)
    }

    pub fn mark_read(&self, id: i64) -> Result<()> {
        self.lock()
            .execute("UPDATE notifications SET read = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn mark_all_read(&self) -> Result<()> {
        self.lock()
            .execute("UPDATE notifications SET read = 1 WHERE read = 0", [])?;
        Ok(())
    }

    pub fn clear_notifications(&self) -> Result<()> {
        self.lock().execute("DELETE FROM notifications", [])?;
        Ok(())
    }

    /// Reports saved while offline, oldest first, as (id, JSON body).
    pub fn feedback_queue(&self) -> Result<Vec<(i64, String)>> {
        let conn = self.lock();
        let mut st = conn.prepare("SELECT id, body FROM feedback_queue ORDER BY id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn queue_feedback(&self, body: &str) -> Result<()> {
        self.lock().execute(
            "INSERT INTO feedback_queue (body, tries) VALUES (?1, 0)",
            [body],
        )?;
        Ok(())
    }

    pub fn drop_feedback(&self, id: i64) -> Result<()> {
        self.lock()
            .execute("DELETE FROM feedback_queue WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn last_synced(&self, repo: &str) -> Result<Option<i64>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .lock()
            .query_row("SELECT synced_at FROM bases WHERE repo = ?1", [repo], |r| {
                r.get(0)
            })
            .optional()?)
    }
}
