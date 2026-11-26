//! Group repo files: `claude-sync.json`, `settings.shared.json`, `changes/before-now.json`.
use super::{Doc, FormatError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const FILE: &str = "claude-sync.json";
pub const SCHEMA: u32 = 1;
pub const TOPIC: &str = "claude-sync-group";
pub const SHARED_SETTINGS: &str = "settings.shared.json";
pub const SCHEDULES: &str = "schedules.json";
pub const CHANGE_NOTES_PATH: &str = "changes/before-now.json";
/// The largest plugin file that syncs.
pub const MAX_PLUGIN_FILE: usize = 1 << 20;

fn yes() -> bool {
    true
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Default for SyncFlags {
    fn default() -> Self {
        SyncFlags {
            settings: true,
            claude_md: true,
            skills: true,
            agents: true,
            commands: true,
            output_styles: true,
            plugins: true,
        }
    }
}

/// `claude-sync.json`. Keys not listed (v2 `host`, `topics`, newer ones) stay in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupFile {
    pub schema: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub sync: SyncFlags,
    /// Absent: on.
    #[serde(default = "yes", rename = "autoAcceptClaudeMd")]
    pub auto_accept_claude_md: bool,
    /// Absent: off; v1 leaves the key out when off.
    #[serde(default, rename = "proposalMode", skip_serializing_if = "is_false")]
    pub proposal_mode: bool,
}

impl GroupFile {
    pub fn new(name: &str, host: &str) -> Self {
        GroupFile {
            schema: SCHEMA,
            name: name.into(),
            members: vec![host.into()],
            sync: SyncFlags::default(),
            auto_accept_claude_md: true,
            proposal_mode: false,
        }
    }

    /// A newer schema is an error: the group is then read-only.
    pub fn open(bytes: &[u8]) -> Result<Doc<GroupFile>, FormatError> {
        let doc = Doc::<GroupFile>::parse(bytes)?;
        if doc.value.schema > SCHEMA {
            return Err(FormatError::TooNew {
                file: FILE,
                found: doc.value.schema.into(),
                max: SCHEMA,
            });
        }
        Ok(doc)
    }

    pub fn has_member(&self, login: &str) -> bool {
        self.members.iter().any(|m| m.eq_ignore_ascii_case(login))
    }
}

/// `settings.shared.json`: any JSON object; every key is kept.
pub fn open_settings(bytes: &[u8]) -> Result<Doc<Value>, FormatError> {
    let doc = Doc::<Value>::parse(bytes)?;
    if !doc.value.is_object() {
        return Err(FormatError::Invalid(format!(
            "{SHARED_SETTINGS} is not a JSON object"
        )));
    }
    Ok(doc)
}

/// `settings.shared.json#/permissions/allow` -> `Some("/permissions/allow")`; the file itself
/// -> `Some("")`; any other path -> `None`.
pub fn settings_pointer(path: &str) -> Option<&str> {
    match path.strip_prefix(SHARED_SETTINGS)? {
        "" => Some(""),
        rest => rest.strip_prefix('#'),
    }
}

/// What a change did before and what it does now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeNote {
    pub before: Option<String>,
    pub now: Option<String>,
}

/// `changes/before-now.json`: `{ "<commit sha>": { "before": "...", "now": "..." } }`. The file
/// is a [`Doc<Value>`] so every entry stays; this reads the usable notes. Bad shapes and
/// entries with neither text are skipped.
pub fn change_notes(file: &Value) -> BTreeMap<String, ChangeNote> {
    let Some(map) = file.as_object() else {
        return BTreeMap::new();
    };
    let text = |v: &Value, k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    map.iter()
        .filter_map(|(sha, v)| {
            let note = ChangeNote {
                before: text(v, "before"),
                now: text(v, "now"),
            };
            (note.before.is_some() || note.now.is_some()).then(|| (sha.clone(), note))
        })
        .collect()
}
