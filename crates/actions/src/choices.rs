//! The person's choices, in the v1 `choices.json` shape (docs/architecture.md §16), kept in
//! the store's `settings` row `choices`. Keys this build does not know are kept. They follow
//! the person through the personal repo (see `personal.rs`).
use claude_sync_engine::traits::{IoError, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const KEY: &str = "choices";

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewChoice {
    /// Follow the group's autoAcceptClaudeMd.
    #[default]
    FollowGroup,
    ReviewFirst,
    AutoAccept,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupChoice {
    pub off: bool,
    pub review_claude_md: Option<bool>,
    pub auto_run_jobs: bool,
    pub accepted_jobs: BTreeMap<String, String>,
    pub off_jobs: BTreeSet<String>,
    pub dropped_lines: Vec<String>,
    pub member: Option<bool>,
    pub ts: u64,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
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
    pub desktop_notifications: Option<bool>,
    /// The Claude helper (Explain); on until turned off.
    pub helper: Option<bool>,
    pub ts: u64,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choices {
    pub groups: BTreeMap<String, GroupChoice>,
    pub order: Order,
    pub app: AppChoice,
    /// `personal`, `sshPeers`, `activity` and whatever a newer build adds.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

pub const DEFAULT_INTERVAL: u64 = 60;
pub const MIN_INTERVAL: u64 = 30;
pub const MAX_INTERVAL: u64 = 900;

impl Choices {
    pub fn load(s: &impl Store) -> Choices {
        s.read(KEY)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, s: &impl Store) -> Result<(), IoError> {
        s.write(KEY, &serde_json::to_vec(self).unwrap_or_default())
    }

    pub fn interval(&self) -> u64 {
        self.app
            .interval_secs
            .unwrap_or(DEFAULT_INTERVAL)
            .clamp(MIN_INTERVAL, MAX_INTERVAL)
    }

    pub fn group(&mut self, repo: &str, now_ms: u64) -> &mut GroupChoice {
        let g = self.groups.entry(repo.to_string()).or_default();
        g.ts = now_ms;
        g
    }

    pub fn review_choice(&self, repo: &str) -> ReviewChoice {
        match self.groups.get(repo).and_then(|g| g.review_claude_md) {
            None => ReviewChoice::FollowGroup,
            Some(true) => ReviewChoice::ReviewFirst,
            Some(false) => ReviewChoice::AutoAccept,
        }
    }
}
