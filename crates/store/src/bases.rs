//! The base of each repo (what the last pass left in agreement), saved in one transaction so
//! a pass killed before it is simply re-planned. A group's rows live in `bases`/`base_files`
//! (the v1 import fills hashes only: the bytes of those come from the v1 `base/files` tree
//! until the first v2 pass saves them), the personal repo's in `personal_base_files`.
use crate::{Db, Result};
use claude_sync_engine::plan::FileSet;
use rusqlite::{params, OptionalExtension};
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use std::path::Path;

/// The repo key of the personal repo.
pub const PERSONAL: &str = "personal";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseRec {
    pub head_sha: String,
    pub etag: Option<String>,
    pub files: FileSet,
}

/// The git blob sha of a file (what GitHub lists), to download only changed paths.
pub fn blob_sha(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", bytes.len()));
    h.update(bytes);
    hex(&h.finalize())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Db {
    /// `None`: never synced. `state` is the state folder, for v1 base trees not yet saved here.
    pub fn load_base(&self, repo: &str, state: &Path) -> Result<Option<BaseRec>> {
        let conn = self.lock();
        let head: Option<(String, Option<String>)> = if repo == PERSONAL {
            conn.query_row(
                "SELECT value FROM settings WHERE key = 'personal.head'",
                [],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .and_then(|v| serde_json::from_slice::<(String, Option<String>)>(&v).ok())
        } else {
            conn.query_row(
                "SELECT head_sha, etag FROM bases WHERE repo = ?1",
                [repo],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        };
        let Some((head_sha, etag)) = head else {
            return Ok(None);
        };
        let mut files = FileSet::new();
        if repo == PERSONAL {
            let mut st = conn.prepare("SELECT path, bytes FROM personal_base_files")?;
            for r in st.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })? {
                let (p, b) = r?;
                files.insert(p, b);
            }
        } else {
            let mut st = conn.prepare("SELECT path, bytes FROM base_files WHERE repo = ?1")?;
            let v1 = state
                .join("groups")
                .join(repo.replace('/', "__"))
                .join("base/files");
            for r in st.query_map([repo], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
            })? {
                let (p, b) = r?;
                // an unreadable v1 file leaves the path out: the planner then sees it as new
                if let Some(b) = b.or_else(|| std::fs::read(v1.join(&p)).ok()) {
                    files.insert(p, b);
                }
            }
        }
        Ok(Some(BaseRec {
            head_sha,
            etag,
            files,
        }))
    }

    /// Replace the base of `repo`; all or nothing.
    pub fn save_base(&self, repo: &str, b: &BaseRec, now: i64) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        if repo == PERSONAL {
            let head = serde_json::to_vec(&(&b.head_sha, &b.etag)).unwrap_or_default();
            tx.execute(
                "INSERT INTO settings (key, value) VALUES ('personal.head', ?1)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                [head],
            )?;
            tx.execute("DELETE FROM personal_base_files", [])?;
            for (p, bytes) in &b.files {
                tx.execute(
                    "INSERT INTO personal_base_files (path, bytes) VALUES (?1, ?2)",
                    params![p, bytes],
                )?;
            }
        } else {
            // a group that joined before its first pass has no row yet; the joined list owns role and order
            tx.execute(
                "INSERT OR IGNORE INTO groups (repo, role, state, order_idx)
                 VALUES (?1, 'member', 'active', (SELECT COALESCE(MAX(order_idx) + 1, 0) FROM groups))",
                [repo],
            )?;
            tx.execute(
                "INSERT INTO bases (repo, head_sha, etag, synced_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (repo) DO UPDATE SET head_sha = excluded.head_sha,
                 etag = excluded.etag, synced_at = excluded.synced_at",
                params![repo, b.head_sha, b.etag, now],
            )?;
            tx.execute("DELETE FROM base_files WHERE repo = ?1", [repo])?;
            for (p, bytes) in &b.files {
                let blob = blob_sha(bytes);
                tx.execute(
                    "INSERT INTO base_files (repo, path, blob_sha, local_hash, bytes)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![repo, p, blob, hex(&Sha256::digest(bytes)), bytes],
                )?;
            }
        }
        Ok(tx.commit()?)
    }

    pub fn add_notification(
        &self,
        kind: &str,
        text: &str,
        repo: Option<&str>,
        now: i64,
    ) -> Result<()> {
        self.lock().execute(
            "INSERT INTO notifications (kind, text, time, repo) VALUES (?1, ?2, ?3, ?4)",
            params![kind, text, now, repo],
        )?;
        Ok(())
    }

    pub fn add_conflict(&self, repo: &str, path: &str, now: i64) -> Result<()> {
        self.lock().execute(
            "INSERT INTO conflicts (repo, path, saved_at) VALUES (?1, ?2, ?3)",
            params![repo, path, now],
        )?;
        Ok(())
    }

    /// The saved conflict copies, newest first (path, when).
    pub fn conflicts(&self, limit: u32) -> Result<Vec<(String, String, i64)>> {
        let conn = self.lock();
        let mut st =
            conn.prepare("SELECT repo, path, saved_at FROM conflicts ORDER BY id DESC LIMIT ?1")?;
        let rows = st.query_map([limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}
