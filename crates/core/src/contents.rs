//! What a group gives this device: its plugin marketplaces, skills, agents, commands,
//! output styles, settings keys and CLAUDE.md, for the group screen's "From this group".
//!
//! Read-only and local: built from `manifest.json`, the files the apply layer placed (§7)
//! and `~/.claude/settings.json` (§8). Nothing here touches the network or writes a file.
use crate::apply::{Manifest, CLAUDE_MD, SHARED_SETTINGS};
use crate::{group, GroupId, Paths};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupContents {
    pub marketplaces: Vec<MarketplaceInfo>,
    pub skills: Vec<Item>,
    pub agents: Vec<Item>,
    pub commands: Vec<Item>,
    pub output_styles: Vec<Item>,
    pub settings: SettingsInfo,
    /// The group's `CLAUDE.md` copy under `groups/<owner>__<repo>/`, when it has one.
    pub claude_md: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceInfo {
    pub name: String,
    pub description: Option<String>,
    /// Its folder on this machine; `None` when this group's copy is not placed here.
    pub path: Option<String>,
    /// Another joined group placed a marketplace with this name first; it is that one's.
    pub held_by: Option<GroupId>,
    /// Plugins listed in its `.claude-plugin/marketplace.json` (empty when not placed).
    pub plugins: Vec<PluginInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    /// `enabledPlugins["<plugin>@<marketplace>"]` is `true` in `~/.claude/settings.json`.
    pub enabled: bool,
}

/// A skill, agent, command or output style the app placed for the group.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub name: String,
    /// `description:` from its frontmatter, when it has one.
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsInfo {
    /// Top-level keys of the group's `settings.shared.json`, in file order.
    pub keys: Vec<String>,
    /// Its `enabledPlugins` entries: `<plugin>@<marketplace>` -> on.
    pub enabled_plugins: Vec<(String, bool)>,
    /// This device stopped applying the group's settings ("Keep local").
    pub kept_local: bool,
}

/// Biggest file read for a description. Frontmatter sits at the top.
const MAX_READ: u64 = 64 * 1024;

pub fn group_contents(paths: &Paths, group: &GroupId) -> crate::Result<GroupContents> {
    let m = Manifest::load(paths)?;
    let entry = m
        .groups
        .get(&group.to_string())
        .cloned()
        .unwrap_or_default();
    let enabled = enabled_plugins(&paths.claude_home.join("settings.json"));
    let mut out = GroupContents::default();

    let mut skills = BTreeSet::new();
    let mut kinds: [(&str, BTreeSet<String>); 3] = [
        ("agents", BTreeSet::new()),
        ("commands", BTreeSet::new()),
        ("output-styles", BTreeSet::new()),
    ];
    let mut shipped_mps = BTreeSet::new();
    for (rel, mf) in &m.files {
        if mf.group != *group {
            continue;
        }
        if let Some(mp) = group::marketplace_of(rel) {
            shipped_mps.insert(mp.to_string());
        } else if let Some(rest) = rel.strip_prefix("skills/") {
            if let Some((name, _)) = rest.split_once('/') {
                skills.insert(name.to_string());
            }
        } else if let Some((dir, file)) = rel.split_once('/') {
            if let Some((_, names)) = kinds.iter_mut().find(|(d, _)| *d == dir) {
                if let Some(name) = file.strip_suffix(".md") {
                    names.insert(name.to_string());
                }
            }
        }
    }
    out.skills = skills
        .into_iter()
        .map(|name| {
            let file = paths
                .claude_home
                .join("skills")
                .join(&name)
                .join("SKILL.md");
            Item {
                description: description_of(&file),
                name,
            }
        })
        .collect();
    let [agents, commands, styles] = kinds.map(|(dir, names)| {
        names
            .into_iter()
            .map(|name| Item {
                description: description_of(
                    &paths.claude_home.join(dir).join(format!("{name}.md")),
                ),
                name,
            })
            .collect::<Vec<_>>()
    });
    (out.agents, out.commands, out.output_styles) = (agents, commands, styles);

    // Marketplaces: the ones the group holds here, plus the ones it ships that are not
    // placed (another group has the name, or the marketplace is invalid).
    let mut names: BTreeSet<String> = m
        .marketplaces
        .iter()
        .filter(|(_, o)| o.group == *group)
        .map(|(n, _)| n.clone())
        .collect();
    names.extend(shipped_mps);
    names.extend(
        entry
            .excluded
            .iter()
            .filter_map(|p| group::marketplace_of(p))
            .map(str::to_string),
    );
    for name in names {
        let owner = m.marketplaces.get(&name).map(|o| &o.group);
        let mut info = MarketplaceInfo {
            name: name.clone(),
            ..Default::default()
        };
        match owner {
            Some(o) if o == group => {
                let dir = paths.marketplaces_dir().join(&name);
                let dir = std::path::absolute(&dir).unwrap_or(dir);
                read_marketplace(&dir, &name, &enabled, &mut info);
                info.path = Some(dir.display().to_string());
            }
            Some(o) => info.held_by = Some(o.clone()),
            None => {}
        }
        out.marketplaces.push(info);
    }

    // Settings and CLAUDE.md, from the group's copies as last applied.
    let gdir = paths.group_dir(group);
    if entry.settings.is_some() {
        if let Some(Value::Object(obj)) = read_json(&gdir.join(SHARED_SETTINGS)) {
            out.settings.keys = obj.keys().cloned().collect();
            if let Some(Value::Object(ep)) = obj.get("enabledPlugins") {
                out.settings.enabled_plugins = ep
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_bool() == Some(true)))
                    .collect();
            }
        }
    }
    out.settings.kept_local = entry.keep_local.contains(SHARED_SETTINGS);
    let md = gdir.join(CLAUDE_MD);
    if entry.claude_md.is_some() && md.is_file() {
        let md = std::path::absolute(&md).unwrap_or(md);
        out.claude_md = Some(md.display().to_string());
    }
    Ok(out)
}

/// Fill `info` from `<dir>/.claude-plugin/marketplace.json`. A plugin's missing description
/// or version comes from its own `.claude-plugin/plugin.json` when its source is a folder
/// inside the marketplace.
fn read_marketplace(
    dir: &Path,
    name: &str,
    enabled: &BTreeMap<String, bool>,
    info: &mut MarketplaceInfo,
) {
    let Some(mp) = read_json(&dir.join(".claude-plugin").join("marketplace.json")) else {
        return;
    };
    info.description = text(&mp, "description").or_else(|| {
        mp.get("metadata")
            .and_then(|meta| text(meta, "description"))
    });
    let Some(list) = mp.get("plugins").and_then(Value::as_array) else {
        return;
    };
    for p in list {
        let Some(pname) = text(p, "name") else {
            continue;
        };
        let own = p
            .get("source")
            .and_then(Value::as_str)
            .and_then(|s| s.strip_prefix("./"))
            .filter(|s| s.split('/').all(|seg| !seg.is_empty() && seg != ".."))
            .and_then(|s| read_json(&dir.join(s).join(".claude-plugin").join("plugin.json")));
        let from_own = |k: &str| own.as_ref().and_then(|o| text(o, k));
        info.plugins.push(PluginInfo {
            description: text(p, "description").or_else(|| from_own("description")),
            version: text(p, "version").or_else(|| from_own("version")),
            enabled: enabled.get(&format!("{pname}@{name}")) == Some(&true),
            name: pname,
        });
    }
}

/// `enabledPlugins` of a settings file; empty when it is missing or unreadable.
fn enabled_plugins(file: &Path) -> BTreeMap<String, bool> {
    let Some(Value::Object(ep)) = read_json(file).and_then(|v| v.get("enabledPlugins").cloned())
    else {
        return BTreeMap::new();
    };
    ep.into_iter()
        .map(|(k, v)| (k, v.as_bool() == Some(true)))
        .collect()
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn read_small(p: &Path) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut buf = Vec::new();
    std::fs::File::open(p)
        .ok()?
        .take(MAX_READ)
        .read_to_end(&mut buf)
        .ok()?;
    Some(buf)
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_slice(&read_small(p)?).ok()
}

fn description_of(p: &Path) -> Option<String> {
    frontmatter_description(&String::from_utf8_lossy(&read_small(p)?))
}

/// `description:` from a `---` frontmatter block. Handles plain and quoted values and
/// YAML block scalars (`>`, `|`), whose indented lines are joined with spaces.
pub fn frontmatter_description(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let mut lines = lines.take_while(|l| l.trim_end() != "---").peekable();
    while let Some(line) = lines.next() {
        let Some(value) = line.strip_prefix("description:") else {
            continue;
        };
        let value = value.trim();
        let joined = if value.starts_with(['>', '|']) {
            let mut parts = Vec::new();
            while let Some(next) =
                lines.next_if(|l| l.starts_with([' ', '\t']) || l.trim().is_empty())
            {
                parts.push(next.trim());
            }
            parts.retain(|s| !s.is_empty());
            parts.join(" ")
        } else {
            unquote(value).to_string()
        };
        return Some(joined).filter(|s| !s.is_empty());
    }
    None
}

fn unquote(s: &str) -> &str {
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            return inner;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::apply_group;
    use crate::FileSet;
    use serde_json::json;
    use std::fs;

    struct Env {
        _tmp: tempfile::TempDir,
        paths: Paths,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            claude_home: tmp.path().join("home").join(".claude"),
            state_dir: tmp.path().join("home").join(".claude-sync"),
        };
        Env { _tmp: tmp, paths }
    }

    fn set(files: &[(&str, &[u8])]) -> FileSet {
        files
            .iter()
            .map(|(p, b)| (p.to_string(), b.to_vec()))
            .collect()
    }

    fn put(p: &Path, b: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b).unwrap();
    }

    fn marketplace(name: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "name": name,
            "description": "Team mods.",
            "plugins": [
                { "name": "router", "source": "./mods/router", "description": "Routes turns." },
                { "name": "manager", "source": "./mods/manager", "version": "2.0.0" },
                { "name": "remote", "source": { "source": "github", "repo": "a/b" } }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn lists_what_the_group_placed() {
        let e = env();
        let g = GroupId::new("owner", "team");
        let mp = marketplace("mods");
        let manager = br#"{"name":"manager","version":"1.0.0","description":"Runs workers."}"#;
        let shared = br#"{"permissions":{"allow":["Bash(ls)"]},"enabledPlugins":{"router@mods":true,"manager@mods":false},"model":"opus"}"#;
        let files = set(&[
            ("CLAUDE.md", b"# Team\n"),
            ("settings.shared.json", shared),
            (
                "skills/review/SKILL.md",
                b"---\nname: review\ndescription: \"Reviews a diff.\"\n---\nBody\n",
            ),
            ("skills/review/notes.md", b"x"),
            ("skills/plain/SKILL.md", b"no frontmatter"),
            (
                "agents/scout.md",
                b"---\ndescription: >\n  Finds code\n  fast.\n---\n",
            ),
            ("commands/ship.md", b"ship it"),
            ("output-styles/terse.md", b"---\ndescription: Short.\n---\n"),
            ("plugins/mods/.claude-plugin/marketplace.json", &mp),
            (
                "plugins/mods/mods/manager/.claude-plugin/plugin.json",
                manager,
            ),
            ("plugins/mods/mods/router/hooks.json", b"{}"),
        ]);
        apply_group(&e.paths, &g, std::slice::from_ref(&g), &files).unwrap();

        let c = group_contents(&e.paths, &g).unwrap();
        let item = |n: &str, d: Option<&str>| Item {
            name: n.into(),
            description: d.map(Into::into),
        };
        assert_eq!(
            c.skills,
            vec![item("plain", None), item("review", Some("Reviews a diff."))]
        );
        assert_eq!(c.agents, vec![item("scout", Some("Finds code fast."))]);
        assert_eq!(c.commands, vec![item("ship", None)]);
        assert_eq!(c.output_styles, vec![item("terse", Some("Short."))]);

        assert_eq!(c.marketplaces.len(), 1);
        let m = &c.marketplaces[0];
        assert_eq!(m.name, "mods");
        assert_eq!(m.description.as_deref(), Some("Team mods."));
        assert_eq!(m.held_by, None);
        let dir = std::path::absolute(e.paths.marketplaces_dir().join("mods")).unwrap();
        assert_eq!(m.path.as_deref(), Some(dir.display().to_string().as_str()));
        let plugins: Vec<_> = m
            .plugins
            .iter()
            .map(|p| {
                (
                    p.name.as_str(),
                    p.description.as_deref(),
                    p.version.as_deref(),
                    p.enabled,
                )
            })
            .collect();
        assert_eq!(
            plugins,
            vec![
                ("router", Some("Routes turns."), None, true),
                // The marketplace entry's version wins over plugin.json's.
                ("manager", Some("Runs workers."), Some("2.0.0"), false),
                ("remote", None, None, false),
            ]
        );

        assert_eq!(
            c.settings.keys,
            vec!["permissions", "enabledPlugins", "model"]
        );
        assert_eq!(
            c.settings.enabled_plugins,
            vec![("router@mods".into(), true), ("manager@mods".into(), false)]
        );
        assert!(!c.settings.kept_local);
        let md = c.claude_md.expect("CLAUDE.md copy");
        assert!(md.ends_with("CLAUDE.md") && Path::new(&md).is_file());

        // The JSON shape the UI reads.
        let v = serde_json::to_value(GroupContents {
            claude_md: None,
            ..group_contents(&e.paths, &g).unwrap()
        })
        .unwrap();
        assert_eq!(v["outputStyles"][0]["name"], "terse");
        assert_eq!(v["marketplaces"][0]["heldBy"], Value::Null);
        assert_eq!(
            v["settings"]["enabledPlugins"][0],
            json!(["router@mods", true])
        );
        assert_eq!(v["settings"]["keptLocal"], false);
    }

    #[test]
    fn enabled_state_follows_local_settings() {
        let e = env();
        let g = GroupId::new("owner", "team");
        let mp = marketplace("mods");
        let files = set(&[("plugins/mods/.claude-plugin/marketplace.json", &mp)]);
        apply_group(&e.paths, &g, std::slice::from_ref(&g), &files).unwrap();
        let on = |c: &GroupContents| {
            c.marketplaces[0]
                .plugins
                .iter()
                .filter(|p| p.enabled)
                .count()
        };
        assert_eq!(on(&group_contents(&e.paths, &g).unwrap()), 0);

        let file = e.paths.claude_home.join("settings.json");
        let mut s: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        s["enabledPlugins"] = json!({ "manager@mods": true, "router@other": true });
        put(&file, &serde_json::to_vec(&s).unwrap());
        let c = group_contents(&e.paths, &g).unwrap();
        assert_eq!(on(&c), 1);
        assert!(c.marketplaces[0].plugins[1].enabled);
    }

    #[test]
    fn a_marketplace_another_group_holds_is_named_with_its_owner() {
        let e = env();
        let (a, b) = (GroupId::new("owner", "a"), GroupId::new("owner", "b"));
        let order = [a.clone(), b.clone()];
        let mp = marketplace("mods");
        let files = set(&[("plugins/mods/.claude-plugin/marketplace.json", &mp)]);
        apply_group(&e.paths, &a, &order, &files).unwrap();
        apply_group(&e.paths, &b, &order, &files).unwrap();

        let cb = group_contents(&e.paths, &b).unwrap();
        assert_eq!(cb.marketplaces.len(), 1);
        assert_eq!(cb.marketplaces[0].held_by.as_ref(), Some(&a));
        assert_eq!(cb.marketplaces[0].path, None);
        assert!(cb.marketplaces[0].plugins.is_empty());

        let ca = group_contents(&e.paths, &a).unwrap();
        assert_eq!(ca.marketplaces[0].held_by, None);
        assert_eq!(ca.marketplaces[0].plugins.len(), 3);
    }

    #[test]
    fn an_invalid_marketplace_is_listed_as_not_placed() {
        let e = env();
        let g = GroupId::new("owner", "team");
        let files = set(&[("plugins/broken/readme.md", b"no manifest")]);
        apply_group(&e.paths, &g, std::slice::from_ref(&g), &files).unwrap();
        let c = group_contents(&e.paths, &g).unwrap();
        assert_eq!(
            c.marketplaces,
            vec![MarketplaceInfo {
                name: "broken".into(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn other_groups_and_user_files_are_left_out() {
        let e = env();
        let (a, b) = (GroupId::new("owner", "a"), GroupId::new("owner", "b"));
        let order = [a.clone(), b.clone()];
        put(&e.paths.claude_home.join("skills/mine/SKILL.md"), b"mine");
        apply_group(
            &e.paths,
            &a,
            &order,
            &set(&[("skills/a-skill/SKILL.md", b"a")]),
        )
        .unwrap();
        apply_group(
            &e.paths,
            &b,
            &order,
            &set(&[
                ("skills/b-skill/SKILL.md", b"b"),
                ("skills/mine/SKILL.md", b"group"),
            ]),
        )
        .unwrap();
        let c = group_contents(&e.paths, &b).unwrap();
        let names: Vec<_> = c.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["b-skill"]);
        assert_eq!(c.claude_md, None);
        assert!(c.settings.keys.is_empty());
    }

    #[test]
    fn an_unknown_group_is_empty() {
        let e = env();
        let c = group_contents(&e.paths, &GroupId::new("x", "y")).unwrap();
        assert_eq!(c, GroupContents::default());
    }

    #[test]
    fn frontmatter_descriptions() {
        let d = frontmatter_description;
        assert_eq!(
            d("---\ndescription: plain text\n---\n").as_deref(),
            Some("plain text")
        );
        assert_eq!(
            d("---\r\nname: x\r\ndescription: 'quoted'\r\n---\r\n").as_deref(),
            Some("quoted")
        );
        assert_eq!(
            d("\u{feff}---\ndescription: |\n  line one\n\n  line two\nname: x\n---\n").as_deref(),
            Some("line one line two")
        );
        assert_eq!(
            d("---\nname: x\n---\ndescription: body, not frontmatter\n"),
            None
        );
        assert_eq!(d("description: no frontmatter\n"), None);
        assert_eq!(d("---\ndescription:\n---\n"), None);
    }
}
