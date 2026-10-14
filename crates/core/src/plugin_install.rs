//! Installing the plugins a group turns on (§18).
//!
//! Placing a bundled marketplace and setting `enabledPlugins` is not enough: Claude Code only
//! loads a plugin that is installed in its plugin cache, and it does not install one on its
//! own. So after an apply that changed a group's marketplaces or the plugins it turns on, the
//! app runs the `claude plugin` CLI in the background: `marketplace update` once per changed
//! marketplace, `install --scope user` for each group-enabled plugin that is not installed,
//! and `update` for an installed one whose marketplace entry changed. Nothing is ever
//! uninstalled. Failures become warnings on the group screen (with Retry); they never fail a
//! sync. A plugin that needs `-y` (marketplace-declared commands) is reported, not confirmed.
use crate::apply::{sha256_hex, Manifest, SHARED_SETTINGS};
use crate::state::write_json;
use crate::{claude_cli, secrets, GroupId, Paths};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

/// `<state_dir>/plugins.json`: what was last done per group.
pub const STATE_FILE: &str = "plugins.json";
/// Per `claude plugin` command.
pub const TIMEOUT: Duration = Duration::from_secs(120);
pub const CLAUDE_NOT_FOUND: &str =
    "Claude Code not found (the claude command), so the group's plugins were not installed";

/// One installed plugin as `claude plugin list --json` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// `name@marketplace`.
    pub id: String,
    pub version: Option<String>,
    pub scope: Option<String>,
}

/// The `claude plugin` commands the app runs; tests use a fake.
pub trait PluginCli {
    fn list(&self) -> Result<Vec<Installed>, String>;
    fn marketplace_update(&self, name: &str) -> Result<(), String>;
    fn install(&self, id: &str) -> Result<(), String>;
    fn update(&self, id: &str) -> Result<(), String>;
}

/// The real CLI: no console window, killed after [`TIMEOUT`], output logged redacted.
pub struct RealCli {
    claude: PathBuf,
    /// `CLAUDE_CONFIG_DIR` when the app's `~/.claude` is not the default one.
    config_dir: Option<PathBuf>,
}

impl RealCli {
    pub fn new(claude: PathBuf, paths: &Paths, home: &Path) -> Self {
        let config_dir =
            (paths.claude_home != home.join(".claude")).then(|| paths.claude_home.clone());
        Self { claude, config_dir }
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut cmd = Command::new(&self.claude);
        cmd.args(args);
        if let Some(dir) = &self.config_dir {
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        let out = claude_cli::run_command(cmd, TIMEOUT)
            .map_err(|e| format!("could not start claude: {e}"))?;
        tracing::info!(
            args = ?args,
            code = ?out.code,
            stdout = %secrets::redact(tail(&out.stdout, 2000)),
            stderr = %secrets::redact(tail(&out.stderr, 2000)),
            "claude plugin"
        );
        match out.code {
            Some(0) => Ok(out.stdout),
            None => Err(format!("timed out after {} min", TIMEOUT.as_secs() / 60)),
            Some(code) => Err(reason(&out.stdout, &out.stderr, code)),
        }
    }
}

impl PluginCli for RealCli {
    fn list(&self) -> Result<Vec<Installed>, String> {
        parse_list(&self.run(&["plugin", "list", "--json"])?)
    }
    fn marketplace_update(&self, name: &str) -> Result<(), String> {
        self.run(&["plugin", "marketplace", "update", name])
            .map(drop)
    }
    fn install(&self, id: &str) -> Result<(), String> {
        self.run(&["plugin", "install", id, "--scope", "user"])
            .map(drop)
    }
    fn update(&self, id: &str) -> Result<(), String> {
        self.run(&["plugin", "update", id, "--scope", "user"])
            .map(drop)
    }
}

/// The last `max` bytes of `s` (on a char boundary).
fn tail(s: &str, max: usize) -> &str {
    let mut start = s.len().saturating_sub(max);
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
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
        .map(secrets::redact);
    match line {
        Some(l) if l.chars().count() > 200 => {
            format!("{}…", l.chars().take(200).collect::<String>())
        }
        Some(l) => l,
        None => format!("exit code {code}"),
    }
}

/// `claude plugin list --json`: an array of `{id, version, scope, ...}`.
pub fn parse_list(stdout: &str) -> Result<Vec<Installed>, String> {
    let v: Vec<Value> = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("unexpected output of claude plugin list --json: {e}"))?;
    let s = |p: &Value, k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
    Ok(v.iter()
        .filter_map(|p| {
            Some(Installed {
                id: s(p, "id")?,
                version: s(p, "version"),
                scope: s(p, "scope"),
            })
        })
        .collect())
}

/// `name@marketplace` with plain characters only (it is passed as an argument).
pub fn valid_plugin_id(id: &str) -> bool {
    let part = |s: &str| {
        !s.is_empty()
            && !s.starts_with(['-', '.'])
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    id.split_once('@')
        .is_some_and(|(name, mp)| part(name) && part(mp))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupState {
    /// Bundled marketplace name -> fingerprint of its folder when last updated.
    #[serde(default)]
    pub marketplaces: BTreeMap<String, String>,
    /// The group-enabled plugin ids last handled.
    #[serde(default)]
    pub enabled: Vec<String>,
    /// Plugin id -> hash of its marketplace entry when last handled.
    #[serde(default)]
    pub entries: BTreeMap<String, String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    groups: BTreeMap<String, GroupState>,
}

impl State {
    fn load(paths: &Paths) -> crate::Result<Self> {
        match std::fs::read(paths.state_dir.join(STATE_FILE)) {
            Ok(b) => Ok(serde_json::from_slice(&b).unwrap_or_default()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    fn save(&self, paths: &Paths) -> crate::Result<()> {
        write_json(&paths.state_dir.join(STATE_FILE), self)
    }
}

/// The warnings of the last run for `group` (shown on the group screen).
pub fn warnings(paths: &Paths, group: &GroupId) -> crate::Result<Vec<String>> {
    Ok(State::load(paths)?
        .groups
        .remove(&group.to_string())
        .map(|g| g.warnings)
        .unwrap_or_default())
}

/// Forget a group that was left (its plugins stay installed).
pub fn forget(paths: &Paths, group: &GroupId) -> crate::Result<()> {
    let mut st = State::load(paths)?;
    if st.groups.remove(&group.to_string()).is_some() {
        st.save(paths)?;
    }
    Ok(())
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
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

/// Plugins the group turns on that are still on here: `true` in the group's
/// `settings.shared.json` as last applied and in `~/.claude/settings.json`.
pub fn group_enabled(paths: &Paths, group: &GroupId) -> Vec<String> {
    let Some(shared) = read_value(&paths.group_dir(group).join(SHARED_SETTINGS)) else {
        return Vec::new();
    };
    let local = read_value(&paths.claude_home.join("settings.json")).unwrap_or(Value::Null);
    let local: Vec<&str> = enabled_ids(&local).collect();
    let mut ids: Vec<String> = enabled_ids(&shared)
        .filter(|id| valid_plugin_id(id) && local.contains(id))
        .map(str::to_string)
        .collect();
    ids.sort();
    ids
}

/// A hash of every file under `dir` (paths and contents); empty when there is none.
fn fingerprint(dir: &Path) -> String {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
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

/// The entry of plugin `name@mp` in a bundled marketplace: its hash and declared version.
fn entry(paths: &Paths, id: &str) -> Option<(String, Option<String>)> {
    let (name, mp) = id.split_once('@')?;
    let file = paths
        .marketplaces_dir()
        .join(mp)
        .join(".claude-plugin")
        .join("marketplace.json");
    let v = read_value(&file)?;
    let e = v
        .get("plugins")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(name))?;
    let version = e.get("version").and_then(Value::as_str).map(str::to_string);
    Some((sha256_hex(e.to_string().as_bytes()), version))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Something changed (or `force`): the CLI was asked.
    pub ran: bool,
    pub marketplaces_updated: Vec<String>,
    pub installed: Vec<String>,
    pub updated: Vec<String>,
    pub warnings: Vec<String>,
}

static RUNNING: Mutex<()> = Mutex::new(());

/// Bring `group`'s plugins in line after an apply (see the module doc). Does nothing unless
/// one of its bundled marketplaces or its enabled plugins changed since the last run, or
/// `force` (Retry). `cli` is called only when there is work, and `None` means Claude Code is
/// not installed: that is a warning, and the next call tries again.
pub fn ensure(
    paths: &Paths,
    group: &GroupId,
    cli: &dyn Fn() -> Option<Box<dyn PluginCli>>,
    force: bool,
) -> crate::Result<Outcome> {
    let _one_at_a_time = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    let manifest = Manifest::load(paths)?;
    let mps: BTreeMap<String, String> = manifest
        .marketplaces
        .iter()
        .filter(|(_, m)| &m.group == group)
        .map(|(name, _)| {
            (
                name.clone(),
                fingerprint(&paths.marketplaces_dir().join(name)),
            )
        })
        .collect();
    let enabled = group_enabled(paths, group);

    let mut st = State::load(paths)?;
    let key = group.to_string();
    let prev = st.groups.get(&key).cloned().unwrap_or_default();
    let changed: Vec<&String> = mps
        .iter()
        .filter(|(name, f)| force || prev.marketplaces.get(*name) != Some(*f))
        .map(|(name, _)| name)
        .collect();
    let added = enabled.iter().any(|id| !prev.enabled.contains(id));
    let mut out = Outcome::default();
    let record = |st: &mut State, warnings: Vec<String>| {
        let entries = enabled
            .iter()
            .filter_map(|id| entry(paths, id).map(|(h, _)| (id.clone(), h)))
            .collect();
        st.groups.insert(
            key.clone(),
            GroupState {
                marketplaces: mps.clone(),
                enabled: enabled.clone(),
                entries,
                warnings,
            },
        );
    };
    if !force && changed.is_empty() && !added {
        // At most plugins left the group: nothing to install, and nothing is uninstalled.
        if prev.enabled != enabled {
            record(&mut st, prev.warnings.clone());
            st.save(paths)?;
        }
        return Ok(out);
    }

    let Some(cli) = cli() else {
        if prev.warnings != [CLAUDE_NOT_FOUND] {
            tracing::warn!(group = %group, "claude not found; group plugins not installed");
        }
        // Not recorded as done: the next sync tries again.
        let mut gs = prev;
        gs.warnings = vec![CLAUDE_NOT_FOUND.to_string()];
        st.groups.insert(key, gs);
        st.save(paths)?;
        out.warnings = vec![CLAUDE_NOT_FOUND.to_string()];
        return Ok(out);
    };
    out.ran = true;
    for name in changed {
        match cli.marketplace_update(name) {
            Ok(()) => out.marketplaces_updated.push(name.clone()),
            Err(e) => out
                .warnings
                .push(format!("couldn't update the {name} marketplace: {e}")),
        }
    }
    if !enabled.is_empty() {
        match cli.list() {
            Err(e) => out
                .warnings
                .push(format!("couldn't list the installed plugins: {e}")),
            Ok(installed) => {
                for id in &enabled {
                    let have = installed
                        .iter()
                        .find(|i| &i.id == id && i.scope.as_deref().is_none_or(|s| s == "user"));
                    let Some(have) = have else {
                        match cli.install(id) {
                            Ok(()) => out.installed.push(id.clone()),
                            Err(e) => out.warnings.push(format!("couldn't install {id}: {e}")),
                        }
                        continue;
                    };
                    let current = entry(paths, id);
                    let entry_changed = current
                        .as_ref()
                        .is_some_and(|(h, _)| prev.entries.get(id).is_some_and(|p| p != h));
                    let version_differs = current
                        .as_ref()
                        .and_then(|(_, v)| v.as_deref())
                        .is_some_and(|v| have.version.as_deref().is_some_and(|iv| iv != v));
                    if entry_changed || version_differs {
                        match cli.update(id) {
                            Ok(()) => out.updated.push(id.clone()),
                            Err(e) => out.warnings.push(format!("couldn't update {id}: {e}")),
                        }
                    }
                }
            }
        }
    }
    record(&mut st, out.warnings.clone());
    st.save(paths)?;
    if out
        != (Outcome {
            ran: true,
            ..Outcome::default()
        })
    {
        tracing::info!(
            group = %group,
            marketplaces = ?out.marketplaces_updated,
            installed = ?out.installed,
            updated = ?out.updated,
            warnings = ?out.warnings,
            "group plugins ensured"
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::ManagedMarketplace;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Fake {
        installed: RefCell<Vec<Installed>>,
        calls: RefCell<Vec<String>>,
        fail: RefCell<BTreeMap<String, String>>,
    }

    struct Handle(Rc<Fake>);

    impl Fake {
        fn call(&self, c: String) -> Result<(), String> {
            self.calls.borrow_mut().push(c.clone());
            match self.fail.borrow().get(&c) {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
    }

    impl PluginCli for Handle {
        fn list(&self) -> Result<Vec<Installed>, String> {
            self.0.call("list".into())?;
            Ok(self.0.installed.borrow().clone())
        }
        fn marketplace_update(&self, name: &str) -> Result<(), String> {
            self.0.call(format!("marketplace update {name}"))
        }
        fn install(&self, id: &str) -> Result<(), String> {
            self.0.call(format!("install {id}"))?;
            self.0.installed.borrow_mut().push(Installed {
                id: id.into(),
                version: None,
                scope: Some("user".into()),
            });
            Ok(())
        }
        fn update(&self, id: &str) -> Result<(), String> {
            self.0.call(format!("update {id}"))
        }
    }

    struct Env {
        _tmp: tempfile::TempDir,
        paths: Paths,
        group: GroupId,
        fake: Rc<Fake>,
    }

    impl Env {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let paths = Paths {
                claude_home: tmp.path().join("claude"),
                state_dir: tmp.path().join("sync"),
            };
            let group = GroupId::new("acme", "data");
            let mut m = Manifest::default();
            m.marketplaces.insert(
                "mods".into(),
                ManagedMarketplace {
                    group: group.clone(),
                    setting: None,
                },
            );
            write_json(&paths.state_dir.join(crate::apply::MANIFEST_FILE), &m).unwrap();
            assert_eq!(Manifest::load(&paths).unwrap(), m, "manifest file name");
            let env = Self {
                _tmp: tmp,
                paths,
                group,
                fake: Rc::default(),
            };
            env.marketplace(&[("router", "1.0.0"), ("manager", "1.0.0")]);
            env.settings(
                &["router@mods", "manager@mods"],
                &["router@mods", "manager@mods"],
            );
            env
        }
        fn marketplace(&self, plugins: &[(&str, &str)]) {
            let list: Vec<Value> = plugins
                .iter()
                .map(|(n, v)| serde_json::json!({"name": n, "version": v, "source": format!("./{n}")}))
                .collect();
            let v = serde_json::json!({"name": "mods", "plugins": list});
            write_json(
                &self
                    .paths
                    .marketplaces_dir()
                    .join("mods/.claude-plugin/marketplace.json"),
                &v,
            )
            .unwrap();
        }
        fn settings(&self, shared: &[&str], local: &[&str]) {
            let obj = |ids: &[&str]| serde_json::json!({ "enabledPlugins": ids.iter().map(|i| (i.to_string(), Value::Bool(true))).collect::<serde_json::Map<_, _>>() });
            write_json(
                &self.paths.group_dir(&self.group).join(SHARED_SETTINGS),
                &obj(shared),
            )
            .unwrap();
            write_json(&self.paths.claude_home.join("settings.json"), &obj(local)).unwrap();
        }
        fn ensure(&self, force: bool) -> Outcome {
            let fake = self.fake.clone();
            let cli = move || Some(Box::new(Handle(fake.clone())) as Box<dyn PluginCli>);
            ensure(&self.paths, &self.group, &cli, force).unwrap()
        }
        fn calls(&self) -> Vec<String> {
            std::mem::take(&mut *self.fake.calls.borrow_mut())
        }
    }

    #[test]
    fn plugin_ids() {
        assert!(valid_plugin_id("model-router@claude-mods"));
        assert!(valid_plugin_id("a.b_c@x"));
        for bad in [
            "", "x", "@m", "x@", "-x@m", "x@-m", "a b@m", "x@m@n", "x@m;rm",
        ] {
            assert!(!valid_plugin_id(bad), "{bad}");
        }
    }

    #[test]
    fn parses_the_list_and_short_reasons() {
        let l = parse_list(
            r#"[{"id":"a@m","version":"1.2.0","scope":"user","enabled":true},{"id":"b@m"},{"x":1}]"#,
        )
        .unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].version.as_deref(), Some("1.2.0"));
        assert!(parse_list("not json").is_err());
        assert_eq!(
            reason("", "\nError: plugin not found\n\n", 1),
            "Error: plugin not found"
        );
        assert_eq!(reason("", "", 3), "exit code 3");
        assert!(reason("Use --yes to accept", "", 1).contains("confirmation"));
    }

    #[test]
    fn group_enabled_needs_both_files() {
        let e = Env::new();
        e.settings(
            &["router@mods", "manager@mods", "x y@bad"],
            &["router@mods", "other@m"],
        );
        assert_eq!(group_enabled(&e.paths, &e.group), ["router@mods"]);
    }

    #[test]
    fn installs_missing_plugins_once() {
        let e = Env::new();
        e.fake.installed.borrow_mut().push(Installed {
            id: "router@mods".into(),
            version: Some("1.0.0".into()),
            scope: Some("user".into()),
        });
        let o = e.ensure(false);
        assert_eq!(o.installed, ["manager@mods"]);
        assert!(o.warnings.is_empty());
        assert_eq!(
            e.calls(),
            ["marketplace update mods", "list", "install manager@mods"]
        );
        // Nothing changed: the CLI is not asked again.
        assert_eq!(e.ensure(false), Outcome::default());
        assert!(e.calls().is_empty());
    }

    #[test]
    fn a_changed_entry_updates_and_a_new_plugin_installs() {
        let e = Env::new();
        e.ensure(false);
        e.calls();
        // A new version of router in the marketplace.
        e.marketplace(&[("router", "1.1.0"), ("manager", "1.0.0")]);
        let o = e.ensure(false);
        assert_eq!(o.updated, ["router@mods"]);
        assert_eq!(
            e.calls(),
            ["marketplace update mods", "list", "update router@mods"]
        );

        // The group turns on another plugin (no marketplace change).
        e.settings(
            &["router@mods", "manager@mods", "new@other"],
            &["router@mods", "manager@mods", "new@other"],
        );
        let o = e.ensure(false);
        assert_eq!(o.installed, ["new@other"]);
        assert_eq!(e.calls(), ["list", "install new@other"]);
    }

    #[test]
    fn a_plugin_installed_by_the_member_is_left_alone() {
        let e = Env::new();
        e.ensure(false);
        e.calls();
        // The group drops manager: nothing is uninstalled, the CLI is not even asked.
        e.settings(&["router@mods"], &["router@mods", "manager@mods"]);
        let o = e.ensure(false);
        assert!(!o.ran);
        // The group drops everything.
        e.settings(&[], &["router@mods", "manager@mods"]);
        let o = e.ensure(false);
        assert!(!o.ran);
        assert!(e.calls().is_empty());
    }

    #[test]
    fn failures_become_warnings_and_retry_runs_again() {
        let e = Env::new();
        e.fake
            .fail
            .borrow_mut()
            .insert("install router@mods".into(), "network down".into());
        let o = e.ensure(false);
        assert_eq!(o.installed, ["manager@mods"]);
        assert_eq!(o.warnings, ["couldn't install router@mods: network down"]);
        assert_eq!(warnings(&e.paths, &e.group).unwrap(), o.warnings);
        e.calls();

        // A plain sync does not retry; Retry does.
        assert!(!e.ensure(false).ran);
        e.fake.fail.borrow_mut().clear();
        let o = e.ensure(true);
        assert_eq!(o.installed, ["router@mods"]);
        assert!(o.warnings.is_empty());
        assert!(warnings(&e.paths, &e.group).unwrap().is_empty());
        assert_eq!(
            e.calls(),
            ["marketplace update mods", "list", "install router@mods"]
        );
    }

    #[test]
    fn missing_claude_is_one_warning_and_tried_again() {
        let e = Env::new();
        let none = || None;
        let o = ensure(&e.paths, &e.group, &none, false).unwrap();
        assert_eq!(o.warnings, [CLAUDE_NOT_FOUND]);
        assert_eq!(warnings(&e.paths, &e.group).unwrap(), [CLAUDE_NOT_FOUND]);
        // Claude Code gets installed: the next sync does the work.
        let o = e.ensure(false);
        assert_eq!(o.installed, ["manager@mods", "router@mods"]);
        assert!(warnings(&e.paths, &e.group).unwrap().is_empty());
        forget(&e.paths, &e.group).unwrap();
        assert!(e.ensure(false).ran);
    }

    #[test]
    fn list_failure_is_a_warning() {
        let e = Env::new();
        e.fake
            .fail
            .borrow_mut()
            .insert("list".into(), "boom".into());
        let o = e.ensure(false);
        assert_eq!(o.warnings, ["couldn't list the installed plugins: boom"]);
        assert!(o.installed.is_empty());
    }
}
