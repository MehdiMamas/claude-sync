//! Counts of the requests this process sent to GitHub, per caller, endpoint and result.
//! The app writes them to `requests.json`; `claude-sync status` reads that file.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;

/// Minutes of per-minute buckets kept.
const WINDOW_MINUTES: u64 = 10;

tokio::task_local! {
    static CALLER: String;
}

/// Run `fut` with every GitHub request it makes (in this task) tagged with `caller`.
pub async fn with_caller<F: std::future::Future>(caller: impl Into<String>, fut: F) -> F::Output {
    CALLER.scope(caller.into(), fut).await
}

/// The caller of the current task, `other` when none was set.
fn current_caller() -> String {
    CALLER
        .try_with(Clone::clone)
        .unwrap_or_else(|_| "other".into())
}

/// One row of the counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub caller: String,
    /// Method and normalized endpoint, e.g. `GET repos/:repo/pulls`.
    pub endpoint: String,
    /// `200`, `304` or `other`.
    pub result: String,
    pub count: u64,
}

/// The content of `requests.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Unix time the counting started.
    pub since: u64,
    pub last10min: Vec<Entry>,
    pub total: Vec<Entry>,
}

type Key = (String, String, String);

#[derive(Default)]
struct Inner {
    since: u64,
    total: BTreeMap<Key, u64>,
    /// (minute number, counts in that minute), oldest first.
    minutes: VecDeque<(u64, BTreeMap<Key, u64>)>,
}

/// Shared by all clones of a `GitHub`.
pub struct RequestCounts(Mutex<Inner>);

impl Default for RequestCounts {
    fn default() -> Self {
        Self(Mutex::new(Inner {
            since: crate::state::now_unix(),
            ..Inner::default()
        }))
    }
}

impl RequestCounts {
    /// Count one response of the current task's caller.
    pub(crate) fn record(&self, method: &str, path: &str, status: u16) {
        self.record_at(
            &current_caller(),
            method,
            path,
            status,
            crate::state::now_unix(),
        );
    }

    pub(crate) fn record_at(&self, caller: &str, method: &str, path: &str, status: u16, now: u64) {
        let result = match status {
            304 => "304",
            200..=299 => "200",
            _ => "other",
        };
        let key = (
            caller.to_string(),
            format!("{method} {}", normalize(path)),
            result.to_string(),
        );
        let mut g = self.0.lock().unwrap();
        *g.total.entry(key.clone()).or_default() += 1;
        let minute = now / 60;
        if g.minutes.back().map(|(m, _)| *m) != Some(minute) {
            g.minutes.push_back((minute, BTreeMap::new()));
        }
        *g.minutes.back_mut().unwrap().1.entry(key).or_default() += 1;
        prune(&mut g.minutes, minute);
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_at(crate::state::now_unix())
    }

    pub(crate) fn snapshot_at(&self, now: u64) -> Snapshot {
        let mut g = self.0.lock().unwrap();
        prune(&mut g.minutes, now / 60);
        let mut recent: BTreeMap<Key, u64> = BTreeMap::new();
        for (_, m) in &g.minutes {
            for (k, n) in m {
                *recent.entry(k.clone()).or_default() += n;
            }
        }
        Snapshot {
            since: g.since,
            last10min: entries(&recent),
            total: entries(&g.total),
        }
    }

    /// Write the snapshot to `path` (temp file, then rename).
    pub fn write(&self, path: &Path) -> crate::Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.snapshot())
            .map_err(|e| crate::Error::Invalid(e.to_string()))?;
        crate::state::write_atomic(path, &bytes)
    }
}

/// Read `requests.json`; `None` when it is missing or unreadable.
pub fn read(path: &Path) -> Option<Snapshot> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn prune(minutes: &mut VecDeque<(u64, BTreeMap<Key, u64>)>, current: u64) {
    while minutes
        .front()
        .is_some_and(|(m, _)| m + WINDOW_MINUTES <= current)
    {
        minutes.pop_front();
    }
}

fn entries(map: &BTreeMap<Key, u64>) -> Vec<Entry> {
    let mut v: Vec<Entry> = map
        .iter()
        .map(|((caller, endpoint, result), count)| Entry {
            caller: caller.clone(),
            endpoint: endpoint.clone(),
            result: result.clone(),
            count: *count,
        })
        .collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.count));
    v
}

/// The text lines of the top `n` rows: `   12  GET repos/:repo/pulls  200  (tick)`.
pub fn top_lines(rows: &[Entry], n: usize) -> Vec<String> {
    rows.iter()
        .take(n)
        .map(|e| {
            format!(
                "{:>5}  {}  {}  ({})",
                e.count, e.endpoint, e.result, e.caller
            )
        })
        .collect()
}

/// `/repos/o/r/git/trees/<sha>?x=1` -> `repos/:repo/git/trees/:id`. Names of unbounded
/// variety (file paths, refs) collapse to one placeholder.
pub fn normalize(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or("");
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        let s = segs[i];
        match (out.last().map(String::as_str), s) {
            (None, "repos") => {
                out.push("repos".into());
                out.push(":repo".into());
                i += 3;
                continue;
            }
            (Some("users" | "orgs"), _) => out.push(":user".into()),
            (Some("contents"), _) => {
                out.push(":path".into());
                break;
            }
            (Some("ref" | "refs" | "heads" | "tags"), _) => {
                out.push(":ref".into());
                break;
            }
            _ if is_id(s) => out.push(":id".into()),
            _ => out.push(s.to_string()),
        }
        i += 1;
    }
    out.join("/")
}

/// Digits, or a hex string of 7 or more that has a digit (a sha).
fn is_id(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_digit())
        || (s.len() >= 7
            && s.chars().all(|c| c.is_ascii_hexdigit())
            && s.chars().any(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_normalized() {
        let n = normalize;
        assert_eq!(
            n("/repos/me/grp/pulls?state=open&page=2"),
            "repos/:repo/pulls"
        );
        assert_eq!(n("/repos/me/grp/pulls/42"), "repos/:repo/pulls/:id");
        assert_eq!(
            n("/repos/me/grp/git/trees/0123456789abcdef0123456789abcdef01234567?recursive=1"),
            "repos/:repo/git/trees/:id"
        );
        assert_eq!(
            n("/repos/me/grp/contents/a/b.md"),
            "repos/:repo/contents/:path"
        );
        assert_eq!(
            n("/repos/me/grp/git/ref/heads/main"),
            "repos/:repo/git/ref/:ref"
        );
        assert_eq!(n("/repos/me/grp"), "repos/:repo");
        assert_eq!(n("/graphql"), "graphql");
        assert_eq!(n("/user"), "user");
        assert_eq!(n("/user/invitations"), "user/invitations");
        assert_eq!(n("/users/octocat"), "users/:user");
        assert_eq!(n("/users/octocat/keys"), "users/:user/keys");
    }

    #[test]
    fn results_and_callers_are_kept_apart() {
        let c = RequestCounts::default();
        c.record_at("tick", "GET", "/repos/a/b/pulls", 200, 600);
        c.record_at("tick", "GET", "/repos/a/b/pulls", 200, 600);
        c.record_at("tick", "GET", "/repos/a/b/pulls", 304, 600);
        c.record_at("ui:x", "GET", "/repos/a/b/pulls", 404, 600);
        let s = c.snapshot_at(600);
        let find = |caller: &str, result: &str| {
            s.total
                .iter()
                .find(|e| e.caller == caller && e.result == result)
                .map(|e| e.count)
        };
        assert_eq!(find("tick", "200"), Some(2));
        assert_eq!(find("tick", "304"), Some(1));
        assert_eq!(find("ui:x", "other"), Some(1));
        assert_eq!(s.total[0].count, 2, "sorted by count");
        assert_eq!(s.total[0].endpoint, "GET repos/:repo/pulls");
    }

    #[test]
    fn minute_buckets_roll_over() {
        let c = RequestCounts::default();
        c.record_at("tick", "GET", "/user", 200, 0);
        c.record_at("tick", "GET", "/user", 200, 60 * 5);
        assert_eq!(c.snapshot_at(60 * 5).last10min[0].count, 2);
        // Minute 0 is 10 minutes old at minute 10.
        assert_eq!(c.snapshot_at(60 * 10).last10min[0].count, 1);
        assert!(c.snapshot_at(60 * 15).last10min.is_empty());
        assert_eq!(c.snapshot_at(60 * 15).total[0].count, 2);
    }

    #[test]
    fn requests_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("cs-requests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("requests.json");
        let _ = std::fs::remove_file(&path);
        assert_eq!(read(&path), None);
        let c = RequestCounts::default();
        c.record("POST", "/graphql", 200);
        c.write(&path).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back, c.snapshot());
        assert_eq!(back.total[0].endpoint, "POST graphql");
        assert_eq!(back.total[0].caller, "other");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn caller_comes_from_the_task() {
        assert_eq!(current_caller(), "other");
        assert_eq!(
            with_caller("tick", async { current_caller() }).await,
            "tick"
        );
    }
}
