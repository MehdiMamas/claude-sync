//! `import_v1`: read every v1 state file once into the store (v2-ErSchema "Import", v2-Cutover).
//! v1 files are only read, never moved, renamed or deleted. A file that cannot be read is
//! skipped and reported (screen InstallImportFailed). Base trees, `packs-setup/`, logs and
//! backups stay as files; the store points at them.
use crate::{Db, Result};
use rusqlite::{params, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Where v1 kept the choices, under `~/.claude` (choices.rs:36-41).
const CHOICES: &str = "personal-state/claude-sync/choices.json";

/// What the importer does with a file of the v1 state folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Read into the store.
    Imported,
    /// Stays a file in place; the store does not read it.
    Stays,
    /// Not known to the importer: the fixture test fails on it.
    Unknown,
}

/// `rel` is the path under the v1 state folder with `/` separators.
pub fn classify(rel: &str) -> Class {
    use Class::*;
    let p: Vec<&str> = rel.split('/').collect();
    let name = p.last().copied().unwrap_or("");
    // Leftovers of an atomic write, and the `.bad` copy v1 made of a bad file.
    if name.ends_with(".tmp") || name.ends_with(".bad") {
        return Stays;
    }
    match p.as_slice() {
        ["state.json"
        | "manifest.json"
        | "jobs.json"
        | "notifications.json"
        | "requests.json"
        | "app.json"
        | "activity.json"
        | "ssh-peers.json"
        | "settings-owned.json"
        | "plugins.json"
        | "packs.json"] => Imported,
        ["outbox", n] if n.ends_with(".json") => Imported,
        ["conflicts", _, _, _, ..] => Imported,
        ["groups", _, "status.json" | "claude-md-review.json"] => Imported,
        ["groups", _, "base", "head.json"] => Imported,
        ["groups", _, "base", ..] | ["groups", _, "CLAUDE.md"] => Stays,
        ["groups", _, d, ..] if d.starts_with("base.") => Stays,
        ["personal", "state.json"] | ["personal", "base", "head.json"] => Imported,
        ["personal", ..] => Stays,
        ["jobs", _, "last-run.json"] => Imported,
        ["jobs", _, "lock"] => Stays,
        ["logs" | "packs-setup" | "marketplaces" | "machines" | "backups" | "backup", ..] => Stays,
        ["sync.lock" | "daemon.lock" | "daemon.owner" | "token" | "store.db" | "store.db-wal"
        | "store.db-shm" | "claude-sync.db" | "claude-sync.db-wal" | "claude-sync.db-shm"] => Stays,
        _ => Unknown,
    }
}

/// Proof that the caller holds `daemon.lock` (v2-Cutover): the import never stops a v1 daemon.
pub struct DaemonLockHeld(());
impl DaemonLockHeld {
    /// Call only while this process holds `daemon.lock`.
    pub fn claim() -> Self {
        Self(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub file: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Files read into the store (paths under the state folder; `choices.json` is under
    /// `~/.claude`).
    pub imported: Vec<String>,
    /// Files that could not be read; left in place.
    pub skipped: Vec<Skipped>,
    /// Files the importer does not know; left in place.
    pub unknown: Vec<String>,
    /// Where the choices came from: `file`, `old-fields` (no choices.json) or `unreadable`.
    pub choices_from: String,
}

#[derive(Debug)]
pub enum Outcome {
    Done(Report),
    AlreadyDone,
}

/// Import once. `state_dir` is `~/.claude-sync`, `claude_home` is `~/.claude`.
pub fn import_v1(
    db: &Db,
    state_dir: &Path,
    claude_home: &Path,
    _held: &DaemonLockHeld,
) -> Result<Outcome> {
    let mut conn = db.lock();
    if conn
        .query_row("SELECT 1 FROM settings WHERE key = 'import_v1'", [], |_| {
            Ok(())
        })
        .is_ok()
    {
        return Ok(Outcome::AlreadyDone);
    }
    let tx = conn.transaction()?;
    let mut r = Reader {
        state: state_dir,
        report: Report::default(),
    };
    for rel in walk(state_dir) {
        if classify(&rel) == Class::Unknown {
            r.report.unknown.push(rel);
        }
    }
    r.run(&tx, claude_home)?;
    tx.execute(
        "INSERT INTO settings (key, value) VALUES ('import_v1', ?1)",
        [serde_json::to_vec(&r.report).unwrap_or_default()],
    )?;
    tx.commit()?;
    Ok(Outcome::Done(r.report))
}

struct Reader<'a> {
    state: &'a Path,
    report: Report,
}

impl Reader<'_> {
    /// A JSON object at `path`; `None` when missing, or unreadable (then reported).
    fn json_at(&mut self, path: &Path, label: &str) -> Option<Value> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => return self.skip(label, e.to_string()),
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) if v.is_object() => {
                self.report.imported.push(label.into());
                Some(v)
            }
            Ok(_) => self.skip(label, "not a JSON object".into()),
            Err(e) => self.skip(label, e.to_string()),
        }
    }
    fn json(&mut self, rel: &str) -> Option<Value> {
        self.json_at(&self.state.join(rel), rel)
    }
    fn skip(&mut self, file: &str, why: String) -> Option<Value> {
        self.report.skipped.push(Skipped {
            file: file.into(),
            why,
        });
        None
    }
    /// `json`, and the parsed value is kept whole in `settings` as `v1/<rel>`.
    fn json_raw(&mut self, tx: &Transaction, rel: &str) -> Result<Option<Value>> {
        let v = self.json(rel);
        if let Some(v) = &v {
            put(tx, &format!("v1/{rel}"), v)?;
        }
        Ok(v)
    }

    fn run(&mut self, tx: &Transaction, claude_home: &Path) -> Result<()> {
        // choices.json first, and it wins over the old fields.
        let choices_path = claude_home.join(CHOICES);
        let choices = self.json_at(&choices_path, "choices.json");
        let state = self.json_raw(tx, "state.json")?.unwrap_or_default();
        let jobs = self.json_raw(tx, "jobs.json")?.unwrap_or_default();
        for f in [
            "manifest.json",
            "app.json",
            "activity.json",
            "ssh-peers.json",
            "settings-owned.json",
            "plugins.json",
            "packs.json",
            "personal/state.json",
            "personal/base/head.json",
        ] {
            self.json_raw(tx, f)?;
        }

        // Each group's folder, read once.
        let mut reviews: Map<String, Value> = Map::new();
        let mut statuses: Map<String, Value> = Map::new();
        let listed: Vec<String> = match &choices {
            Some(c) => strings(&c["order"]["list"])
                .into_iter()
                .chain(
                    c["groups"]
                        .as_object()
                        .into_iter()
                        .flat_map(|m| m.keys().cloned()),
                )
                .collect(),
            None => strings(&state["groups"]),
        };
        for g in &listed {
            let dir = format!("groups/{}", g.replace('/', "__"));
            if let Some(v) = self.json_raw(tx, &format!("{dir}/status.json"))? {
                statuses.insert(g.clone(), v);
            }
            if let Some(v) = self.json_raw(tx, &format!("{dir}/claude-md-review.json"))? {
                reviews.insert(g.clone(), v);
            }
        }

        let torn = self.report.skipped.iter().any(|s| s.file == "choices.json");
        self.report.choices_from = match (&choices, torn) {
            (Some(_), _) => "file",
            (None, true) => "unreadable",
            (None, false) => "old-fields",
        }
        .into();
        // `consents`: old accepted/auto-run values count only when there is no choices file at
        // all; an unreadable one must not bring back a consent that was taken back.
        let eff = match choices {
            Some(c) => c,
            None => old_choices(&state, &jobs, &reviews, !torn),
        };
        put(tx, "choices", &eff)?;
        put(tx, "choices.source", &json!(self.report.choices_from))?;

        // Joined groups in order.
        let joined = joined(&eff);
        let login = state["login"].as_str().unwrap_or("");
        for (i, g) in joined.iter().enumerate() {
            let off = eff["groups"][g]["off"].as_bool().unwrap_or(false);
            let host = statuses
                .get(g)
                .and_then(|s| s["host"].as_str())
                .is_some_and(|h| !login.is_empty() && h.eq_ignore_ascii_case(login));
            tx.execute(
                "INSERT INTO groups (repo, role, state, order_idx, off) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    g,
                    if host { "host" } else { "member" },
                    if off { "off" } else { "active" },
                    i as i64,
                    off
                ],
            )?;
            let sync = statuses.get(g).and_then(|s| s["lastSyncUnix"].as_i64());
            self.base(tx, g, sync)?;
            self.reviews(tx, g, &eff, reviews.get(g))?;
        }
        self.jobs(tx, &jobs, &eff, &joined)?;
        self.notifications(tx)?;
        self.requests(tx)?;
        self.outbox(tx)?;
        self.conflicts(tx)?;
        Ok(())
    }

    /// `groups/<g>/base/head.json` and its files: the base tree stays on disk, the store keeps
    /// the head, its ETag and every file's blob sha, so the first v2 pass is not a first sync.
    fn base(&mut self, tx: &Transaction, g: &str, last_sync: Option<i64>) -> Result<()> {
        let dir = format!("groups/{}/base", g.replace('/', "__"));
        let Some(head) = self.json(&format!("{dir}/head.json")) else {
            return Ok(());
        };
        let Some(commit) = head["commit"].as_str() else {
            self.report.imported.pop();
            self.skip(&format!("{dir}/head.json"), "no commit".into());
            return Ok(());
        };
        let date = head["date"]
            .as_str()
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
            .map(|d| d.timestamp());
        tx.execute(
            "INSERT INTO bases (repo, head_sha, etag, synced_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                g,
                commit,
                head["etag"].as_str(),
                last_sync.or(date).unwrap_or(0)
            ],
        )?;
        let root = self.state.join(&dir).join("files");
        for rel in walk(&root) {
            let bytes = match std::fs::read(root.join(&rel)) {
                Ok(b) => b,
                Err(e) => {
                    self.skip(&format!("{dir}/files/{rel}"), e.to_string());
                    continue;
                }
            };
            tx.execute(
                "INSERT INTO base_files (repo, path, blob_sha, local_hash) VALUES (?1, ?2, ?3, ?4)",
                params![g, rel, blob_sha(&bytes), hex(&Sha256::digest(&bytes))],
            )?;
        }
        Ok(())
    }

    /// Dropped lines (choices) and the review file's pending and declined items.
    fn reviews(
        &mut self,
        tx: &Transaction,
        g: &str,
        eff: &Value,
        review: Option<&Value>,
    ) -> Result<()> {
        let add = |line: &str, author: &str, status: &str| {
            tx.execute(
                "INSERT INTO review_items (repo, line, author, status) VALUES (?1, ?2, ?3, ?4)",
                params![g, line, author, status],
            )
        };
        for l in strings(&eff["groups"][g]["droppedLines"]) {
            add(&l, "", "declined")?;
        }
        let Some(r) = review else { return Ok(()) };
        // v2's review state has v1's shape (format 2): carry it over, so the first v2 pass
        // holds the pending lines instead of accepting the group's text. The per-group
        // choice wins over the flag in the file.
        let mut st = r.clone();
        if let Some(c) = eff["groups"][g]["reviewClaudeMd"].as_bool() {
            st["review"] = json!(c);
        }
        put(tx, &format!("review/{g}"), &st)?;
        for it in r["pending"].as_array().into_iter().flatten() {
            let lines = strings(&it["lines"]).join("\n");
            add(&lines, it["author"].as_str().unwrap_or(""), "pending")?;
        }
        // v1 kept only the id of a declined item, not its text.
        for it in r["declined"].as_array().into_iter().flatten() {
            add(it["id"].as_str().unwrap_or(""), "", "declined")?;
        }
        Ok(())
    }

    fn jobs(
        &mut self,
        tx: &Transaction,
        jobs: &Value,
        eff: &Value,
        joined: &[String],
    ) -> Result<()> {
        let mut seen: Vec<(String, String)> = Vec::new();
        for (id, rec) in jobs["jobs"].as_object().into_iter().flatten() {
            if let Some(g) = rec["group"].as_str() {
                seen.push((g.into(), id.clone()));
            }
        }
        for g in joined {
            for id in eff["groups"][g]["acceptedJobs"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, _)| k)
            {
                seen.push((g.clone(), id.clone()));
            }
        }
        seen.sort();
        seen.dedup();
        for (g, id) in seen.into_iter().filter(|(g, _)| joined.contains(g)) {
            // The id names a folder below jobs/: never follow one that climbs out of it.
            if id.contains(['/', '\\']) || id == ".." {
                continue;
            }
            let c = &eff["groups"][&g];
            let last = self.json(&format!("jobs/{id}/last-run.json"));
            tx.execute(
                "INSERT INTO jobs (repo, job_id, spec_hash, accepted_hash, enabled, last_result, last_run)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    g,
                    id,
                    jobs["jobs"][&id]["registered"].as_str().unwrap_or(""),
                    c["acceptedJobs"][&id].as_str(),
                    !strings(&c["offJobs"]).contains(&id),
                    last.as_ref().and_then(|l| l["status"].as_str()).unwrap_or(""),
                    last.as_ref().and_then(|l| l["endUnix"].as_i64()).unwrap_or(0),
                ],
            )?;
        }
        Ok(())
    }

    fn notifications(&mut self, tx: &Transaction) -> Result<()> {
        let Some(n) = self.json("notifications.json") else {
            return Ok(());
        };
        for it in n["items"].as_array().into_iter().flatten() {
            let Some(id) = it["id"].as_i64() else {
                continue;
            };
            tx.execute(
                "INSERT OR REPLACE INTO notifications (id, kind, text, read, title, time, repo)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    it["kind"].as_str().unwrap_or("other"),
                    it["body"].as_str().unwrap_or(""),
                    it["read"].as_bool().unwrap_or(false),
                    it["title"].as_str().unwrap_or(""),
                    it["time"].as_i64().unwrap_or(0),
                    it["group"].as_str()
                ],
            )?;
        }
        // Ids are never reused, even after Clear all.
        put(tx, "notifications.next_id", &n["nextId"])?;
        Ok(())
    }

    fn requests(&mut self, tx: &Transaction) -> Result<()> {
        let Some(r) = self.json("requests.json") else {
            return Ok(());
        };
        for e in r["total"].as_array().into_iter().flatten() {
            tx.execute(
                "INSERT INTO requests (caller, endpoint, status, n) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (caller, endpoint, status) DO UPDATE SET n = n + excluded.n",
                params![
                    e["caller"].as_str().unwrap_or("other"),
                    e["endpoint"].as_str().unwrap_or(""),
                    e["result"].as_str().unwrap_or("other"),
                    e["count"].as_i64().unwrap_or(0)
                ],
            )?;
        }
        put(tx, "requests.since", &r["since"])?;
        Ok(())
    }

    /// `outbox/*.json`: reports waiting to be sent (their `.tmp` leftovers stay).
    fn outbox(&mut self, tx: &Transaction) -> Result<()> {
        for rel in walk(&self.state.join("outbox")) {
            let label = format!("outbox/{rel}");
            if classify(&label) != Class::Imported {
                continue;
            }
            let body = match std::fs::read_to_string(self.state.join(&label)) {
                Ok(b) => b,
                Err(e) => {
                    self.skip(&label, e.to_string());
                    continue;
                }
            };
            match serde_json::from_str::<Value>(&body) {
                Ok(_) => {
                    tx.execute(
                        "INSERT INTO feedback_queue (body, tries) VALUES (?1, 0)",
                        [&body],
                    )?;
                    self.report.imported.push(label);
                }
                Err(e) => {
                    self.skip(&label, e.to_string());
                }
            }
        }
        Ok(())
    }

    /// `conflicts/<owner>__<repo>|personal/<UTC stamp>/<path>`: one row per kept copy; the
    /// copies stay where they are.
    fn conflicts(&mut self, tx: &Transaction) -> Result<()> {
        for rel in walk(&self.state.join("conflicts")) {
            let mut p = rel.splitn(3, '/');
            let (Some(g), Some(stamp), Some(path)) = (p.next(), p.next(), p.next()) else {
                continue;
            };
            let at = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ")
                .map_or(0, |t| t.and_utc().timestamp());
            tx.execute(
                "INSERT INTO conflicts (repo, path, saved_at) VALUES (?1, ?2, ?3)",
                params![g.replace("__", "/"), path, at],
            )?;
            self.report.imported.push(format!("conflicts/{rel}"));
        }
        Ok(())
    }
}

/// The choices as v1's first load made them from the old per-device fields (choices.rs).
fn old_choices(state: &Value, jobs: &Value, reviews: &Map<String, Value>, consents: bool) -> Value {
    let disabled = strings(&state["disabled"]);
    let auto = strings(&jobs["autoAccept"]);
    let mut groups = Map::new();
    for g in strings(&state["groups"]) {
        let (mut accepted, mut off_jobs) = (Map::new(), Vec::new());
        for (id, rec) in jobs["jobs"].as_object().into_iter().flatten() {
            if rec["group"].as_str() != Some(g.as_str()) || !consents {
                continue;
            }
            if let Some(a) = rec["accepted"].as_str() {
                accepted.insert(id.clone(), json!(a));
            }
            if rec["enabled"].as_bool() == Some(false) {
                off_jobs.push(id.clone());
            }
        }
        groups.insert(
            g.clone(),
            json!({
                "off": disabled.contains(&g),
                "reviewClaudeMd": reviews.get(&g).and_then(|r| r["review"].as_bool()),
                "autoRunJobs": consents && auto.contains(&g),
                "acceptedJobs": accepted,
                "offJobs": off_jobs,
                "droppedLines": [],
                "member": true,
                "ts": 0,
            }),
        );
    }
    json!({
        "groups": groups,
        "personal": {"off": false, "ts": 0},
        "order": {"list": state["groups"], "ts": 0},
        "app": {"paused": state["paused"].as_bool().unwrap_or(false),
                "intervalSecs": state["pollSeconds"], "ts": 0},
    })
}

/// Joined groups in priority order: the order list minus the ones recorded as left, then any
/// group marked joined that the list forgot.
fn joined(eff: &Value) -> Vec<String> {
    let member = |g: &str| eff["groups"][g]["member"].as_bool();
    let mut out: Vec<String> = strings(&eff["order"]["list"])
        .into_iter()
        .filter(|g| member(g) != Some(false))
        .collect();
    for g in eff["groups"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, _)| k)
    {
        if member(g) == Some(true) && !out.contains(g) {
            out.push(g.clone());
        }
    }
    out.dedup();
    out
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(String::from))
        .collect()
}

fn put(tx: &Transaction, key: &str, v: &Value) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![key, v.to_string().into_bytes()],
    )?;
    Ok(())
}

/// Git's blob id: sha1 of `blob <len>\0<bytes>` (what the GitHub tree lists).
fn blob_sha(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", bytes.len()));
    h.update(bytes);
    hex(&h.finalize())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Every file under `root` as a `/` path relative to it, sorted; links are not followed.
pub(crate) fn walk(root: &Path) -> Vec<String> {
    fn go(dir: &Path, rel: &str, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if t.is_dir() {
                go(&e.path(), &r, out);
            } else if t.is_file() {
                out.push(r);
            }
        }
    }
    let mut out = Vec::new();
    go(root, "", &mut out);
    out.sort();
    out
}
