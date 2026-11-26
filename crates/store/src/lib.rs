//! The SQLite store (`store.db`, v2-ErSchema) and the one-time import of the v1 state files.
//!
//! v2-Parity rows covered: 8 and 9 (state kept across the update), 13 (the first v2 pass is
//! never a first sync: bases point at the v1 base trees), 30 (the request counts survive).
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo)
)]

mod bases;
mod groups;
mod import;
mod jobs;
pub use bases::{blob_sha, BaseRec, PERSONAL};
pub use groups::{GroupRec, NotificationRec};
pub use import::{classify, import_v1, Class, DaemonLockHeld, Outcome, Report, Skipped};
pub use jobs::JobRec;

use claude_sync_engine::traits::{IoError, IoResult, Store};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("store: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("store file: {0}")]
    Io(#[from] std::io::Error),
    #[error("store.db is schema {0}, this build knows up to {1}; update claude-sync")]
    TooNew(u32, u32),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Each entry moves the schema one version up; `user_version` says how many ran.
const MIGRATIONS: &[&str] = &[
    "
CREATE TABLE groups (
  repo TEXT PRIMARY KEY, role TEXT NOT NULL CHECK (role IN ('host','member')),
  state TEXT NOT NULL, order_idx INTEGER NOT NULL, off INTEGER NOT NULL DEFAULT 0,
  renamed_from TEXT);
CREATE TABLE bases (
  repo TEXT PRIMARY KEY REFERENCES groups(repo) ON DELETE CASCADE,
  head_sha TEXT NOT NULL, etag TEXT, synced_at INTEGER NOT NULL DEFAULT 0);
CREATE TABLE base_files (
  repo TEXT NOT NULL REFERENCES groups(repo) ON DELETE CASCADE, path TEXT NOT NULL,
  blob_sha TEXT NOT NULL, local_hash TEXT NOT NULL, PRIMARY KEY (repo, path));
CREATE INDEX base_files_blob ON base_files(blob_sha);
CREATE TABLE review_items (
  id INTEGER PRIMARY KEY, repo TEXT NOT NULL REFERENCES groups(repo) ON DELETE CASCADE,
  line TEXT NOT NULL, author TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL CHECK (status IN ('pending','accepted','declined')));
CREATE TABLE jobs (
  repo TEXT NOT NULL REFERENCES groups(repo) ON DELETE CASCADE, job_id TEXT NOT NULL,
  spec_hash TEXT NOT NULL DEFAULT '', accepted_hash TEXT, enabled INTEGER NOT NULL DEFAULT 1,
  last_result TEXT NOT NULL DEFAULT '', last_run INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (repo, job_id));
CREATE TABLE notifications (
  id INTEGER PRIMARY KEY, kind TEXT NOT NULL, text TEXT NOT NULL, read INTEGER NOT NULL DEFAULT 0,
  title TEXT NOT NULL DEFAULT '', time INTEGER NOT NULL DEFAULT 0, repo TEXT);
CREATE TABLE conflicts (
  id INTEGER PRIMARY KEY AUTOINCREMENT, repo TEXT NOT NULL, path TEXT NOT NULL,
  saved_at INTEGER NOT NULL DEFAULT 0);
CREATE TABLE requests (
  caller TEXT NOT NULL, endpoint TEXT NOT NULL, status TEXT NOT NULL, n INTEGER NOT NULL,
  PRIMARY KEY (caller, endpoint, status));
CREATE TABLE feedback_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT NOT NULL, tries INTEGER NOT NULL DEFAULT 0);
CREATE TABLE settings (key TEXT PRIMARY KEY, value BLOB NOT NULL);
",
    "
ALTER TABLE base_files ADD COLUMN bytes BLOB;
CREATE TABLE personal_base_files (path TEXT PRIMARY KEY, bytes BLOB NOT NULL);
",
];

/// One counted line of GitHub requests (the shell turns the github client's counts into these).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRow {
    pub caller: String,
    pub endpoint: String,
    pub status: String,
    pub n: u64,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Open or create `path` (mode 0600 on Unix), WAL, and run the migrations it still needs.
    pub fn open(path: &Path) -> Result<Self> {
        create_private(path)?;
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0))?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .lock()
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    /// Add `rows` to the counts (`n` is what was counted since the last flush, not a total).
    /// The shell calls this once a minute with `github::Client::take_counts()`.
    pub fn add_requests(&self, rows: &[RequestRow]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut st = tx.prepare(
                "INSERT INTO requests (caller, endpoint, status, n) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (caller, endpoint, status) DO UPDATE SET n = n + excluded.n",
            )?;
            for r in rows {
                st.execute(params![r.caller, r.endpoint, r.status, r.n as i64])?;
            }
        }
        Ok(tx.commit()?)
    }

    /// All counts, most frequent first.
    pub fn requests(&self) -> Result<Vec<RequestRow>> {
        let conn = self.lock();
        let mut st = conn.prepare(
            "SELECT caller, endpoint, status, n FROM requests ORDER BY n DESC, caller, endpoint, status",
        )?;
        let rows = st.query_map([], |r| {
            Ok(RequestRow {
                caller: r.get(0)?,
                endpoint: r.get(1)?,
                status: r.get(2)?,
                n: r.get::<_, i64>(3)? as u64,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The report of the v1 import, `None` while it has not run.
    pub fn import_report(&self) -> Result<Option<Report>> {
        let v = self.get("import_v1")?;
        Ok(v.and_then(|b| serde_json::from_slice(&b).ok()))
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .lock()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }
}

/// The engine's keys live in the `settings` table.
impl Store for Db {
    fn read(&self, key: &str) -> IoResult<Option<Vec<u8>>> {
        self.get(key).map_err(|e| IoError(e.to_string()))
    }
    fn write(&self, key: &str, bytes: &[u8]) -> IoResult<()> {
        self.lock()
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, bytes],
            )
            .map(|_| ())
            .map_err(|e| IoError(e.to_string()))
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let have: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let known = MIGRATIONS.len() as u32;
    if have > known {
        return Err(Error::TooNew(have, known));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(have as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// The file exists with mode 0600 before SQLite opens it; the `-wal` and `-shm` files copy it.
fn create_private(path: &Path) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        o.mode(0o600);
        o.open(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        o.open(path).map(|_| ())
    }
}
