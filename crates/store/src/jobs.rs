//! The `jobs` table: what this device registered with the OS (`spec_hash`, empty: nothing) and
//! how the last run went. Acceptance and the off switch live in the choices, which follow the
//! person; the old `accepted_hash` and `enabled` columns are not read.
use crate::{Db, Result};
use rusqlite::params;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRec {
    pub repo: String,
    pub job_id: String,
    pub spec_hash: String,
    pub last_result: String,
    pub last_run: i64,
}

impl Db {
    pub fn jobs(&self) -> Result<Vec<JobRec>> {
        let conn = self.lock();
        let mut st = conn.prepare(
            "SELECT repo, job_id, spec_hash, last_result, last_run FROM jobs ORDER BY repo, job_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(JobRec {
                repo: r.get(0)?,
                job_id: r.get(1)?,
                spec_hash: r.get(2)?,
                last_result: r.get(3)?,
                last_run: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The task registered for the job is described by `hash` (empty: none).
    pub fn set_job_spec(&self, repo: &str, id: &str, hash: &str) -> Result<()> {
        self.lock().execute(
            "INSERT INTO jobs (repo, job_id, spec_hash) VALUES (?1, ?2, ?3)
             ON CONFLICT (repo, job_id) DO UPDATE SET spec_hash = excluded.spec_hash",
            params![repo, id, hash],
        )?;
        Ok(())
    }

    pub fn set_job_result(&self, repo: &str, id: &str, result: &str, at: i64) -> Result<()> {
        self.lock().execute(
            "INSERT INTO jobs (repo, job_id, last_result, last_run) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (repo, job_id) DO UPDATE SET last_result = excluded.last_result,
               last_run = excluded.last_run",
            params![repo, id, result, at],
        )?;
        Ok(())
    }
}
