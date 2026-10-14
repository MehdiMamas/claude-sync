//! `claude-sync.json` (§4, issue #14).
//!
//! `README.md` lives in the group repo (written once by "create group") but is not part of a
//! group's `FileSet`: it is not allowlisted, so the engine never fetches, applies or pushes it.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const FILE: &str = "claude-sync.json";
pub const SCHEMA: u32 = 1;
pub const TOPIC: &str = "claude-sync-group";
pub const DEFAULT_REPO: &str = "claude-sync-data";
pub const README: &str = "README.md";
pub const CLAUDE_MD: &str = "CLAUDE.md";
pub const SHARED_SETTINGS: &str = "settings.shared.json";
/// Plugin marketplaces bundled in the group repo: `plugins/<marketplace>/...`.
pub const PLUGINS: &str = "plugins";
/// The largest plugin file that syncs. Bigger files are reported and skipped.
pub const MAX_PLUGIN_FILE: usize = 1 << 20;
/// A security note shown wherever someone joins a group, and in the group README.
pub const PLUGINS_WARNING: &str =
    "Plugins in this group run code on every member's machine; only join groups whose members you trust.";

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncFlags {
    #[serde(default = "yes")]
    pub settings: bool,
    #[serde(default = "yes")]
    pub claude_md: bool,
    #[serde(default = "yes")]
    pub skills: bool,
    #[serde(default = "yes")]
    pub agents: bool,
    #[serde(default = "yes")]
    pub commands: bool,
    #[serde(default = "yes")]
    pub output_styles: bool,
    #[serde(default = "yes")]
    pub plugins: bool,
    /// Unknown keys, kept on rewrite.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for SyncFlags {
    fn default() -> Self {
        Self {
            settings: true,
            claude_md: true,
            skills: true,
            agents: true,
            commands: true,
            output_styles: true,
            plugins: true,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupFile {
    pub schema: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub sync: SyncFlags,
    /// Unknown keys, kept on rewrite.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl GroupFile {
    /// A new group with `host` as the only member.
    pub fn new(name: &str, host: &str) -> Self {
        Self {
            schema: SCHEMA,
            name: name.into(),
            members: vec![host.into()],
            sync: SyncFlags::default(),
            extra: Map::new(),
        }
    }

    /// `Err(SchemaTooNew)` when the file is from a newer app: the group is then read-only.
    pub fn parse(bytes: &[u8]) -> crate::Result<Self> {
        let value: Value = serde_json::from_slice(bytes)?;
        let schema = value
            .get("schema")
            .and_then(Value::as_u64)
            .ok_or_else(|| crate::Error::Invalid(format!("{FILE} has no schema number")))?;
        if schema > u64::from(SCHEMA) {
            return Err(crate::Error::SchemaTooNew(
                schema.min(u64::from(u32::MAX)) as u32
            ));
        }
        Ok(serde_json::from_value(value)?)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec_pretty(self).expect("group file serializes");
        out.push(b'\n');
        out
    }

    pub fn has_member(&self, login: &str) -> bool {
        self.members.iter().any(|m| m.eq_ignore_ascii_case(login))
    }

    /// False when `login` was already a member.
    pub fn add_member(&mut self, login: &str) -> bool {
        if self.has_member(login) {
            return false;
        }
        self.members.push(login.into());
        true
    }

    /// False when `login` was not a member.
    pub fn remove_member(&mut self, login: &str) -> bool {
        let before = self.members.len();
        self.members.retain(|m| !m.eq_ignore_ascii_case(login));
        self.members.len() != before
    }

    /// Whether the `sync` flags let `path` (already allowlisted) through.
    pub fn syncs(&self, path: &str) -> bool {
        let s = &self.sync;
        match path.split('/').next().unwrap_or_default() {
            FILE => true,
            CLAUDE_MD => s.claude_md,
            SHARED_SETTINGS => s.settings,
            "skills" => s.skills,
            "agents" => s.agents,
            "commands" => s.commands,
            "output-styles" => s.output_styles,
            PLUGINS => s.plugins,
            _ => false,
        }
    }
}

/// True when `path` (repo-relative, forward slashes) is something a group syncs (§4).
pub fn is_allowlisted(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return false;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
        return false;
    }
    if let [PLUGINS, marketplace, rest @ ..] = parts.as_slice() {
        // Plugins keep their dot-files (`.claude-plugin/`, `.gitignore`), but never git
        // internals, Finder litter, env files or installed dependencies.
        return is_marketplace_name(marketplace)
            && !rest.is_empty()
            && !rest.iter().any(|p| {
                matches!(*p, ".git" | ".DS_Store" | ".env" | "node_modules") || p.starts_with(".env.")
            });
    }
    // Dot-file segments (`.git`, `.DS_Store`) never sync outside `plugins/`.
    if parts.iter().any(|p| p.starts_with('.')) {
        return false;
    }
    match parts.as_slice() {
        [FILE | CLAUDE_MD | SHARED_SETTINGS] => true,
        ["skills", _name, _rest @ ..] => parts.len() >= 3,
        ["agents" | "commands" | "output-styles", file] => file.ends_with(".md") && file.len() > 3,
        _ => false,
    }
}

/// A marketplace folder name: `[a-z0-9][a-z0-9-]*`.
pub fn is_marketplace_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The marketplace a `plugins/<marketplace>/...` path belongs to.
pub fn marketplace_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("plugins/")?;
    let (name, tail) = rest.split_once('/')?;
    (!tail.is_empty() && is_marketplace_name(name)).then_some(name)
}

/// The manifest that makes `plugins/<marketplace>/` a marketplace.
pub fn marketplace_manifest(marketplace: &str) -> String {
    format!("{PLUGINS}/{marketplace}/.claude-plugin/marketplace.json")
}

/// True when `path` is a plugin file over the size cap (§4).
pub fn too_large(path: &str, len: usize) -> bool {
    len > MAX_PLUGIN_FILE && marketplace_of(path).is_some()
}

/// Files that exist only in the repo, never in `~/.claude`. The engine supplies their local
/// side itself instead of reading it back from the apply layer.
pub fn is_repo_only(path: &str) -> bool {
    path == FILE || path == README
}

/// The README written into a new group repo.
pub fn readme(name: &str, host: &str) -> String {
    format!(
        "# {name}\n\n\
         This private repo holds a shared Claude Code setup for the group **{name}**, hosted by @{host}.\n\
         It is managed by the claude-sync app. Do not edit it by hand unless you know what you are doing.\n\n\
         ## Join\n\n\
         1. Accept the GitHub invitation to this repo (the host adds you from the app).\n\
         2. Install claude-sync and sign in with GitHub.\n\
         3. The app finds this group and sets it up on your machine.\n\n\
         ## What is shared\n\n\
         - `CLAUDE.md`: shared instructions, imported into your `~/.claude/CLAUDE.md`\n\
         - `settings.shared.json`: keys merged into your `~/.claude/settings.json`\n\
         - `skills/`, `agents/`, `commands/`, `output-styles/`\n\
         - `plugins/`: plugin marketplaces, installed for every member as a local directory marketplace\n\n\
         {PLUGINS_WARNING}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_example() {
        let g = GroupFile::parse(
            br#"{"schema":1,"name":"Mehdi & friends","members":["MehdiMamas","friend-login"],
               "sync":{"settings":true,"claudeMd":true,"skills":true,"agents":true,"commands":true,"outputStyles":false}}"#,
        )
        .unwrap();
        assert_eq!(g.name, "Mehdi & friends");
        assert_eq!(g.members, ["MehdiMamas", "friend-login"]);
        assert!(!g.sync.output_styles);
        assert!(g.sync.claude_md);
    }

    #[test]
    fn round_trip_keeps_unknown_keys() {
        let src = br#"{"schema":1,"name":"n","members":["a"],"sync":{"skills":false,"future":1},"theme":{"x":[1,2]}}"#;
        let mut g = GroupFile::parse(src).unwrap();
        g.add_member("b");
        let back: Value = serde_json::from_slice(&g.to_bytes()).unwrap();
        assert_eq!(back["theme"], serde_json::json!({"x":[1,2]}));
        assert_eq!(back["sync"]["future"], 1);
        assert_eq!(back["sync"]["skills"], false);
        assert_eq!(back["sync"]["agents"], true);
        assert_eq!(back["members"], serde_json::json!(["a", "b"]));
        assert_eq!(GroupFile::parse(&g.to_bytes()).unwrap(), g);
    }

    #[test]
    fn newer_schema_is_refused() {
        let err = GroupFile::parse(br#"{"schema":2,"name":"n","whatever":{}}"#).unwrap_err();
        assert!(matches!(err, crate::Error::SchemaTooNew(2)));
        assert!(GroupFile::parse(br#"{"name":"n"}"#).is_err());
        assert!(GroupFile::parse(b"[1]").is_err());
    }

    #[test]
    fn members_compare_case_insensitively() {
        let mut g = GroupFile::new("n", "Host");
        assert!(g.has_member("host"));
        assert!(!g.add_member("HOST"));
        assert!(g.add_member("Friend"));
        assert!(g.remove_member("friend"));
        assert!(!g.remove_member("friend"));
        assert_eq!(g.members, ["Host"]);
    }

    #[test]
    fn allowlist() {
        for ok in [
            "claude-sync.json",
            "CLAUDE.md",
            "settings.shared.json",
            "skills/review/SKILL.md",
            "skills/review/scripts/run.sh",
            "agents/x.md",
            "commands/deploy.md",
            "output-styles/terse.md",
            "plugins/claude-mods/.claude-plugin/marketplace.json",
            "plugins/claude-mods/.gitignore",
            "plugins/claude-mods/mods/model-router/.claude-plugin/plugin.json",
            "plugins/claude-mods/mods/model-router/hooks/hooks.json",
            "plugins/claude-mods/mods/session-manager/types/index.d.ts",
            "plugins/0x/README",
            "plugins/a/.envrc",
        ] {
            assert!(is_allowlisted(ok), "{ok}");
        }
        for bad in [
            "",
            "README.md",
            "settings.json",
            "skills/review",
            "skills/SKILL.md",
            "skills/../x/y",
            "skills/a/../../etc",
            "skills/a/./b",
            "skills/a//b",
            "skills/a/.DS_Store",
            ".git/config",
            "agents/x.txt",
            "agents/.md",
            "agents/sub/x.md",
            "/CLAUDE.md",
            "C:/x/CLAUDE.md",
            "skills\\a\\b",
            "agents/x.md/",
            "other/file.md",
            "plugins",
            "plugins/claude-mods",
            "plugins/claude-mods/",
            "plugins/Claude-Mods/x.json",
            "plugins/-mods/x.json",
            "plugins/claude_mods/x.json",
            "plugins/.hidden/x.json",
            "plugins/a/.git/config",
            "plugins/a/sub/.git",
            "plugins/a/.DS_Store",
            "plugins/a/.env",
            "plugins/a/sub/.env.local",
            "plugins/a/node_modules/x/index.js",
            "plugins/a/mods/b/node_modules/y.js",
            "plugins/a/../b/x",
            "plugins/a/./x",
            "plugins/a//x",
            "plugins/a/x:y",
            "plugins/a\\x",
        ] {
            assert!(!is_allowlisted(bad), "{bad}");
        }
    }

    #[test]
    fn sync_flags_filter_paths() {
        let mut g = GroupFile::new("n", "h");
        g.sync.skills = false;
        g.sync.claude_md = false;
        assert!(!g.syncs("skills/a/SKILL.md"));
        assert!(!g.syncs("CLAUDE.md"));
        assert!(g.syncs("agents/a.md"));
        assert!(g.syncs(FILE));
        assert!(g.syncs(SHARED_SETTINGS));
        assert!(g.syncs("plugins/a/x.json"));
        g.sync.plugins = false;
        assert!(!g.syncs("plugins/a/x.json"));
    }

    #[test]
    fn plugin_paths() {
        assert_eq!(marketplace_of("plugins/claude-mods/mods/x.ts"), Some("claude-mods"));
        assert_eq!(marketplace_of("plugins/claude-mods"), None);
        assert_eq!(marketplace_of("plugins/Bad/x"), None);
        assert_eq!(marketplace_of("skills/a/b"), None);
        assert_eq!(
            marketplace_manifest("m"),
            "plugins/m/.claude-plugin/marketplace.json"
        );
        assert!(too_large("plugins/m/big.bin", MAX_PLUGIN_FILE + 1));
        assert!(!too_large("plugins/m/big.bin", MAX_PLUGIN_FILE));
        assert!(!too_large("skills/a/big.bin", MAX_PLUGIN_FILE + 1));
    }

    #[test]
    fn readme_warns_about_plugins() {
        let r = readme("n", "h");
        assert!(r.contains("`plugins/`"));
        assert!(r.contains(PLUGINS_WARNING));
    }
}
