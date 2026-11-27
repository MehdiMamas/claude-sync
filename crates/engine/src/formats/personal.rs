//! Personal repo files: `claude-sync-personal.json`, `devices.json`, and
//! `state/claude-sync/choices.json`.
use super::{Doc, FormatError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const REPO: &str = "claude-sync-personal";
pub const FILE: &str = "claude-sync-personal.json";
pub const DEVICES: &str = "devices.json";
pub const FORMAT: u32 = 1;
/// Larger files stay on their device.
pub const MAX_FILE: usize = 256 * 1024;
/// All synced files together.
pub const MAX_TOTAL: usize = 16 * 1024 * 1024;
pub const CHOICES_PATH: &str = "state/claude-sync/choices.json";

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalFile {
    pub format: u32,
    #[serde(default = "yes", rename = "newDevicesJoin")]
    pub new_devices_join: bool,
}

impl PersonalFile {
    pub fn new() -> Self {
        PersonalFile {
            format: FORMAT,
            new_devices_join: true,
        }
    }

    pub fn open(bytes: &[u8]) -> Result<Doc<PersonalFile>, FormatError> {
        let doc = Doc::<PersonalFile>::parse(bytes)?;
        if doc.value.format > FORMAT {
            return Err(FormatError::TooNew {
                file: FILE,
                found: doc.value.format.into(),
                max: FORMAT,
            });
        }
        Ok(doc)
    }
}

impl Default for PersonalFile {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub os: String,
    #[serde(default)]
    pub last_seen_unix: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicesFile {
    #[serde(default)]
    pub devices: Vec<Device>,
}

// ---- choices (architecture §16); `ts` is Unix milliseconds, the newer ts wins on merge.

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupChoice {
    pub off: bool,
    pub review_claude_md: Option<bool>,
    pub auto_run_jobs: bool,
    /// Job id -> consent hash of the version accepted.
    pub accepted_jobs: BTreeMap<String, String>,
    pub off_jobs: BTreeSet<String>,
    pub dropped_lines: Vec<String>,
    /// `Some(true)` joined, `Some(false)` left, `None` no choice recorded.
    pub member: Option<bool>,
    pub ts: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Order {
    pub list: Vec<String>,
    pub ts: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppChoice {
    pub paused: bool,
    pub interval_secs: Option<u64>,
    pub start_at_login: Option<bool>,
    pub ts: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersonalChoice {
    pub off: bool,
    pub ts: u64,
    /// Kinds this person does not sync (`claudeMd`, `memory`, `state`, `settings`).
    #[serde(
        rename = "kindsOff",
        default,
        skip_serializing_if = "BTreeSet::is_empty"
    )]
    pub kinds_off: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshPeersChoice {
    pub on: bool,
    pub ts: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivityChoice {
    pub share: bool,
    pub ts: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choices {
    pub groups: BTreeMap<String, GroupChoice>,
    pub personal: PersonalChoice,
    #[serde(rename = "sshPeers")]
    pub ssh_peers: SshPeersChoice,
    pub activity: ActivityChoice,
    pub order: Order,
    pub app: AppChoice,
}
