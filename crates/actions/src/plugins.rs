//! Group plugins (Parity 26, 27, 28): placing a bundled marketplace and turning a plugin on is
//! not enough, Claude Code only loads an installed one. After a pass the local `claude` CLI
//! registers the group marketplace (`marketplace add`, or `update` once it is known), installs
//! each plugin the group turns on and updates one whose marketplace entry or folder changed.
//! Nothing is ever uninstalled. Failures are warnings on the group (with Retry); they never
//! fail a sync, and the same work is not retried for 5, 15, 30, then 60 minutes. The allowlist
//! is `engine::paths`, the per-file size cap is `fs::pass` (it records what it kept out).
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::Event;
use claude_sync_engine::formats::group::MAX_PLUGIN_FILE;
use claude_sync_engine::traits::Store;
use claude_sync_fs::atomic::sha256_hex;
use claude_sync_fs::redact::redact;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Per `claude plugin` command.
const TIMEOUT: Duration = Duration::from_secs(120);
const BACKOFF_MINUTES: [i64; 4] = [5, 15, 30, 60];

/// One group's failures since the app started (memory only).
pub(crate) struct Backoff {
    failures: usize,
    until: i64,
    /// What the failed run was for; different work runs at once.
    work: String,
}

/// `plugins/<repo>` in the store: what was last done for a group.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct State {
    /// Bundled marketplace name to the fingerprint of its folder when last handled.
    marketplaces: BTreeMap<String, String>,
    enabled: Vec<String>,
    /// Plugin id to the hash of its marketplace entry when last handled.
    entries: BTreeMap<String, String>,
    warnings: Vec<String>,
}

fn key(repo: &str) -> String {
    format!("plugins/{repo}")
}

/// `(id, version, scope)` of an installed plugin.
type Installed = (String, Option<String>, Option<String>);

struct Cli {
    claude: PathBuf,
    /// `CLAUDE_CONFIG_DIR` when `.claude` is not the default folder.
    config_dir: Option<PathBuf>,
}

impl Cli {
    fn run(&self, args: &[&str]) -> std::result::Result<String, String> {
        let mut c = Command::new(&self.claude);
        claude_sync_fs::quiet(&mut c);
        c.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(d) = &self.config_dir {
            c.env("CLAUDE_CONFIG_DIR", d);
        }
        let mut child = c
            .spawn()
            .map_err(|e| format!("could not start claude: {e}"))?;
        let drain = |r: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut s = String::new();
                if let Some(mut r) = r {
                    let _ = r.read_to_string(&mut s);
                }
                s
            })
        };
        let out = drain(child.stdout.take().map(|r| Box::new(r) as _));
        let err = drain(child.stderr.take().map(|r| Box::new(r) as _));
        let start = Instant::now();
        let code = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s.code().unwrap_or(-1),
                Ok(None) if start.elapsed() >= TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("timed out after {} min", TIMEOUT.as_secs() / 60));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(e) => return Err(e.to_string()),
            }
        };
        let (out, err) = (
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default(),
        );
        if code == 0 {
            Ok(out)
        } else {
            Err(reason(&out, &err, code))
        }
    }

    fn list(&self) -> std::result::Result<Vec<Installed>, String> {
        let out = self.run(&["plugin", "list", "--json"])?;
        let v: Vec<Value> = serde_json::from_str(out.trim())
            .map_err(|e| format!("unexpected output of claude plugin list --json: {e}"))?;
        let s = |p: &Value, k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        Ok(v.iter()
            .filter_map(|p| Some((s(p, "id")?, s(p, "version"), s(p, "scope"))))
            .collect())
    }

    fn marketplaces(&self) -> std::result::Result<Vec<String>, String> {
        let out = self.run(&["plugin", "marketplace", "list", "--json"])?;
        let v: Vec<Value> = serde_json::from_str(out.trim())
            .map_err(|e| format!("unexpected output of claude plugin marketplace list: {e}"))?;
        Ok(v.iter()
            .filter_map(|m| m.get("name").and_then(Value::as_str).map(str::to_string))
            .collect())
    }

    /// Register `name` when Claude Code does not know it yet (it only reads
    /// `extraKnownMarketplaces` at a session start), else update it.
    fn add_or_update(&self, dir: &Path, name: &str) -> std::result::Result<(), String> {
        let add = |why: &str| {
            self.run(&["plugin", "marketplace", "add", &dir.to_string_lossy()])
                .map(drop)
                .map_err(|e| format!("could not add the marketplace ({why}): {e}"))
        };
        if self
            .marketplaces()
            .is_ok_and(|known| !known.iter().any(|k| k == name))
        {
            return add("not registered yet");
        }
        match self.run(&["plugin", "marketplace", "update", name]) {
            Err(e) if e.to_lowercase().contains("not found") => add(&e),
            r => r.map(drop),
        }
    }
}

/// A short reason for a failed command: its last non-empty output line, redacted.
fn reason(stdout: &str, stderr: &str, code: i32) -> String {
    let all = format!("{stdout}\n{stderr}").to_lowercase();
    if all.contains("--yes") || all.contains(" -y") {
        return "it needs your confirmation (it declares commands); install it in a terminal \
                with claude plugin install"
            .into();
    }
    let line = [stderr, stdout]
        .iter()
        .find_map(|s| s.lines().rev().map(str::trim).find(|l| !l.is_empty()))
        .map(redact);
    match line {
        Some(l) if l.chars().count() > 200 => {
            format!("{}…", l.chars().take(200).collect::<String>())
        }
        Some(l) => l,
        None => format!("exit code {code}"),
    }
}

/// `name@marketplace` with plain characters only (it is passed as an argument).
fn valid_plugin_id(id: &str) -> bool {
    let part = |s: &str| {
        !s.is_empty()
            && !s.starts_with(['-', '.'])
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    id.split_once('@').is_some_and(|(n, m)| part(n) && part(m))
}

fn enabled_ids(settings: &Value) -> impl Iterator<Item = &str> {
    settings
        .get("enabledPlugins")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, v)| v.as_bool() == Some(true))
        .map(|(k, _)| k.as_str())
}

fn read_value(p: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

/// A hash of every file under `dir` (paths and contents), the app's `COMMIT` marker aside.
fn fingerprint(dir: &Path) -> String {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if rel.is_empty() && name == "COMMIT" {
                continue;
            }
            let rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path(), &rel, out),
                Ok(t) if t.is_file() => {
                    out.push((rel, std::fs::read(e.path()).unwrap_or_default()))
                }
                _ => {}
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, "", &mut files);
    if files.is_empty() {
        return String::new();
    }
    files.sort();
    let mut buf = Vec::new();
    for (rel, bytes) in files {
        buf.extend_from_slice(rel.as_bytes());
        buf.push(0);
        buf.extend_from_slice(sha256_hex(&bytes).as_bytes());
        buf.push(b'\n');
    }
    sha256_hex(&buf)
}

/// The folder a relative `source` of a bundled marketplace entry points at (under
/// `metadata.pluginRoot` when set); `None` for a remote source or one leaving the marketplace.
fn source_dir(root: &Path, v: &Value, e: &Value) -> Option<PathBuf> {
    let src = e.get("source")?.as_str()?;
    let base = v
        .pointer("/metadata/pluginRoot")
        .and_then(Value::as_str)
        .filter(|_| !src.starts_with("./"))
        .unwrap_or("");
    let rel = Path::new(base).join(src);
    rel.components()
        .all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
        .then(|| root.join(rel))
}

/// The entry of plugin `name@mp`: a hash of it and of its source folder (a plugin with no
/// version whose files changed still counts as changed), and its declared version.
fn entry(state: &Path, id: &str) -> Option<(String, Option<String>)> {
    let (name, mp) = id.split_once('@')?;
    let root = state.join("marketplaces").join(mp);
    let v = read_value(&root.join(".claude-plugin/marketplace.json"))?;
    let e = v
        .get("plugins")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(name))?;
    let version = e.get("version").and_then(Value::as_str).map(str::to_string);
    let files = source_dir(&root, &v, e)
        .map(|d| fingerprint(&d))
        .unwrap_or_default();
    Some((sha256_hex(format!("{e}\n{files}").as_bytes()), version))
}

/// What a plugin task needs, owned so it can leave the actor.
struct Work {
    cli: Cli,
    state: PathBuf,
    changed: Vec<String>,
    enabled: Vec<String>,
    current: BTreeMap<String, (String, Option<String>)>,
    prev_entries: BTreeMap<String, String>,
}

/// The end of one plugin task: what to record for the group.
#[derive(Debug, Clone)]
pub struct Done {
    repo: String,
    now: i64,
    work: String,
    record: State,
}

impl Work {
    fn run(self) -> Vec<String> {
        let Work {
            cli,
            state,
            changed,
            enabled,
            current,
            prev_entries,
        } = self;
        let mut warnings = Vec::new();
        for name in &changed {
            let dir = state.join("marketplaces").join(name);
            let dir = std::path::absolute(&dir).unwrap_or(dir);
            if let Err(e) = cli.add_or_update(&dir, name) {
                warnings.push(format!("couldn't update the {name} marketplace: {e}"));
            }
        }
        if enabled.is_empty() {
            return warnings;
        }
        match cli.list() {
            Err(e) => warnings.push(format!("couldn't list the installed plugins: {e}")),
            Ok(installed) => {
                for id in &enabled {
                    let have = installed
                        .iter()
                        .find(|i| &i.0 == id && i.2.as_deref().is_none_or(|s| s == "user"));
                    let args = |verb: &'static str| ["plugin", verb, id, "--scope", "user"];
                    let Some(have) = have else {
                        if let Err(e) = cli.run(&args("install")) {
                            warnings.push(format!("couldn't install {id}: {e}"));
                        }
                        continue;
                    };
                    let cur = current.get(id);
                    let entry_changed = cur.is_some_and(|(h, _)| prev_entries.get(id) != Some(h));
                    let version_differs = cur
                        .and_then(|(_, v)| v.as_deref())
                        .is_some_and(|v| have.1.as_deref().is_some_and(|iv| iv != v));
                    if entry_changed || version_differs {
                        if let Err(e) = cli.run(&args("update")) {
                            warnings.push(format!("couldn't update {id}: {e}"));
                        }
                    }
                }
            }
        }
        warnings
    }
}

impl Core {
    fn plugin_state(&self, repo: &str) -> State {
        Store::read(&self.db, &key(repo))
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_plugin_state(&self, repo: &str, st: &State) -> Result<()> {
        Store::write(
            &self.db,
            &key(repo),
            &serde_json::to_vec(st).map_err(|e| Error::new(e.to_string()))?,
        )?;
        Ok(())
    }

    /// Forget a group that was left (its plugins stay installed).
    pub(crate) fn forget_plugins(&mut self, repo: &str) {
        if !self.plugin_state(repo).marketplaces.is_empty() {
            self.plugins_changed(repo);
        }
        let _ = Store::write(&self.db, &key(repo), b"");
        let _ = Store::write(&self.db, &format!("plugin-skipped/{repo}"), b"");
        self.plugin_backoff.remove(repo);
    }

    /// The window shows the notice above every screen while this notification is unread.
    fn plugins_changed(&mut self, repo: &str) {
        let _ = self.db.add_notification(
            "plugins",
            crate::msg::PLUGINS_CHANGED,
            Some(repo),
            self.now(),
        );
    }

    /// What the group screen lists: the last run's warnings and the files the size cap kept out.
    pub(crate) fn plugin_problems(&self, repo: &str) -> Vec<String> {
        let mut out = self.plugin_state(repo).warnings;
        let skipped = Store::read(&self.db, &format!("plugin-skipped/{repo}"))
            .ok()
            .flatten()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        out.extend(skipped.lines().filter(|l| !l.is_empty()).map(|p| {
            format!(
                "{p} is over the {} MiB limit and was not placed",
                MAX_PLUGIN_FILE >> 20
            )
        }));
        out
    }

    /// Plugins the group turns on that are still on here: `true` in the group's
    /// `settings.shared.json` as last delivered and in `~/.claude/settings.json`.
    fn group_enabled(&self, repo: &str) -> Vec<String> {
        let shared = self
            .db
            .load_base(repo, &self.home.state)
            .ok()
            .flatten()
            .and_then(|b| b.files.get("settings.shared.json").cloned())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let Some(shared) = shared else {
            return Vec::new();
        };
        let local = read_value(&self.home.claude.join("settings.json")).unwrap_or(Value::Null);
        let local: Vec<&str> = enabled_ids(&local).collect();
        let mut ids: Vec<String> = enabled_ids(&shared)
            .filter(|id| valid_plugin_id(id) && local.contains(id))
            .map(str::to_string)
            .collect();
        ids.sort();
        ids
    }

    /// After a pass: every group that syncs plugins.
    pub(crate) fn plugins_after_pass(&mut self) {
        let c = self.choices();
        let repos: Vec<String> = self
            .db
            .groups()
            .unwrap_or_default()
            .into_iter()
            .filter(|g| !g.off && !c.groups.get(&g.repo).is_some_and(|x| x.off))
            .map(|g| g.repo)
            .collect();
        for r in repos {
            if crate::groups::cached(&self.db, &r).is_some_and(|f| f.sync.plugins) {
                let _ = self.ensure_plugins(&r, false);
            }
        }
    }

    pub(crate) fn plugins_retry(&mut self, repo: &str) -> Result<Event> {
        self.ensure_plugins(repo, true)?;
        Ok(Event::Done)
    }

    /// Bring `repo`'s plugins in line. Does nothing unless a bundled marketplace or the enabled
    /// plugins changed since the last run, or `force` (Retry). Returns the warnings.
    pub(crate) fn ensure_plugins(&mut self, repo: &str, force: bool) -> Result<Vec<String>> {
        let state = self.home.state.clone();
        let names: std::collections::BTreeSet<String> = self
            .db
            .load_base(repo, &state)?
            .map(|b| {
                b.files
                    .keys()
                    .filter_map(|p| p.strip_prefix("plugins/")?.split('/').next())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mps: BTreeMap<String, String> = names
            .into_iter()
            .map(|n| {
                let f = fingerprint(&state.join("marketplaces").join(&n));
                (n, f)
            })
            .collect();
        let enabled = self.group_enabled(repo);
        let prev = self.plugin_state(repo);
        let changed: Vec<&String> = mps
            .iter()
            .filter(|(n, f)| force || prev.marketplaces.get(*n) != Some(*f))
            .map(|(n, _)| n)
            .collect();
        // v1 `plugins_changed`: marketplace files were written or removed, or a marketplace left
        if mps.iter().any(|(n, f)| prev.marketplaces.get(n) != Some(f))
            || prev.marketplaces.keys().any(|n| !mps.contains_key(n))
        {
            self.plugins_changed(repo);
        }
        let added = enabled.iter().any(|id| !prev.enabled.contains(id));
        let current: BTreeMap<String, (String, Option<String>)> = enabled
            .iter()
            .filter_map(|id| entry(&state, id).map(|e| (id.clone(), e)))
            .collect();
        let entries_changed = current
            .iter()
            .any(|(id, (h, _))| prev.entries.get(id) != Some(h));
        let record = |warnings: Vec<String>| State {
            marketplaces: mps.clone(),
            enabled: enabled.clone(),
            entries: current
                .iter()
                .map(|(id, (h, _))| (id.clone(), h.clone()))
                .collect(),
            warnings,
        };
        if !force && changed.is_empty() && !added && !entries_changed {
            // At most plugins left the group: nothing to install, and nothing is uninstalled.
            if prev.enabled != enabled {
                self.save_plugin_state(repo, &record(prev.warnings.clone()))?;
            }
            return Ok(prev.warnings);
        }

        if self.plugins_running.contains(repo) {
            return Ok(prev.warnings); // its task is still running; Retry after it ends
        }
        let now = self.now();
        let work = sha256_hex(format!("{mps:?}\n{enabled:?}\n{current:?}").as_bytes());
        match self.plugin_backoff.get(repo) {
            Some(b) if !force && b.work == work && now < b.until => return Ok(prev.warnings),
            Some(b) if force || b.work != work => {
                self.plugin_backoff.remove(repo);
            }
            _ => {}
        }

        let Some(claude) = (self.jobs_env.claude)() else {
            self.plugin_failed(repo, now, work);
            let mut st = prev;
            st.warnings = vec![crate::msg::CLAUDE_NOT_FOUND.to_string()];
            self.save_plugin_state(repo, &st)?;
            return Ok(st.warnings);
        };
        let job = Work {
            cli: Cli {
                claude,
                config_dir: (self.home.claude != self.home.home.join(".claude"))
                    .then(|| self.home.claude.clone()),
            },
            state: state.clone(),
            changed: changed.into_iter().cloned().collect(),
            enabled: enabled.clone(),
            current: current.clone(),
            prev_entries: prev.entries.clone(),
        };
        let repo_s = repo.to_string();
        let base = record(Vec::new());
        let run = move || {
            let warnings = job.run();
            Done {
                repo: repo_s,
                now,
                work,
                record: State { warnings, ..base },
            }
        };
        // The `claude` calls take up to 120 s each: a task runs them and sends `PluginsDone`.
        // Without an actor loop (the headless job runner) they run here.
        let Some(weak) = self.tx.clone() else {
            let d = run();
            let w = d.record.warnings.clone();
            self.plugins_done(d)?;
            return Ok(w);
        };
        self.plugins_running.insert(repo.to_string());
        std::thread::spawn(move || {
            let d = run();
            if let Some(tx) = weak.upgrade() {
                let _ = tx.blocking_send(crate::actor::Msg {
                    cmd: crate::model::Command::PluginsDone(d),
                    reply: None,
                });
            }
        });
        Ok(prev.warnings)
    }

    /// The task ended: only now the back-off and the group's plugin state change.
    pub(crate) fn plugins_done(&mut self, d: Done) -> Result<Event> {
        self.plugins_running.remove(&d.repo);
        if !self.db.groups()?.iter().any(|g| g.repo == d.repo) {
            return Ok(Event::Done); // the group was left meanwhile
        }
        if d.record.warnings.is_empty() {
            self.plugin_backoff.remove(&d.repo);
        } else {
            self.plugin_failed(&d.repo, d.now, d.work);
        }
        self.save_plugin_state(&d.repo, &d.record)?;
        Ok(Event::Done)
    }

    fn plugin_failed(&mut self, repo: &str, now: i64, work: String) {
        let failures = self.plugin_backoff.get(repo).map_or(0, |b| b.failures) + 1;
        let wait = BACKOFF_MINUTES[failures.clamp(1, BACKOFF_MINUTES.len()) - 1] * 60;
        self.plugin_backoff.insert(
            repo.to_string(),
            Backoff {
                failures,
                until: now + wait,
                work,
            },
        );
    }
}
