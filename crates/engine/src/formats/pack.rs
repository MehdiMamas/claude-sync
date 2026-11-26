//! `packs/<id>/pack.json`. [`PackManifest::check`] holds the limits of the file itself; checks
//! that need other repo files (setup.md, plugins, jobs) belong to the planner.
use super::{Doc, FormatError};
use serde::{Deserialize, Serialize};

pub const MANIFEST: &str = "pack.json";
pub const SCHEMA: u32 = 1;
pub const MAX_PACKS: usize = 20;
pub const MAX_MANIFEST: usize = 64 * 1024;
pub const MAX_SETUP: usize = 8 * 1024;
pub const MAX_CHECK_ARGS: usize = 8;
pub const DEFAULT_TIMEOUT: u32 = 60;
pub const MAX_TIMEOUT: u32 = 120;
pub const OSES: [&str; 3] = ["windows", "macos", "linux"];

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_zero(n: &i32) -> bool {
    *n == 0
}

/// `^[a-z0-9][a-z0-9-]{0,30}$`
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 31
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Need {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckSpec {
    pub run: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackManifest {
    pub schema: u32,
    pub id: String,
    pub title: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<Need>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_md: Option<String>,
    pub setup: String,
    pub check: CheckSpec,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub host_carry_over: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub order: i32,
}

fn too_long(s: &str, max: usize) -> bool {
    s.chars().count() > max
}

fn plugin_ok(p: &str) -> bool {
    let word = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    p.split_once('@')
        .is_some_and(|(name, market)| word(name) && word(market))
}

/// `skills/<name>` or `agents/<name>.md`.
fn file_path_ok(p: &str) -> bool {
    match p.split('/').collect::<Vec<_>>().as_slice() {
        ["skills", name] => !name.is_empty() && !name.starts_with('.') && !name.contains(':'),
        ["agents", file] => file.ends_with(".md") && file.len() > 3 && !file.starts_with('.'),
        _ => false,
    }
}

impl PackManifest {
    pub fn open(bytes: &[u8]) -> Result<Doc<PackManifest>, FormatError> {
        if bytes.len() > MAX_MANIFEST {
            return Err(FormatError::Invalid(format!(
                "{MANIFEST} is larger than 64 KiB"
            )));
        }
        Doc::parse(bytes)
    }

    /// The limits of the file alone; `folder_id` is the name of its folder.
    pub fn check(&self, folder_id: &str) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err(format!(
                "schema {} is not supported (this app reads schema {SCHEMA})",
                self.schema
            ));
        }
        if !valid_id(&self.id) {
            return Err(format!(
                "id {:?} must be lowercase letters, digits and dashes (at most 31, starting with a letter or digit)",
                self.id
            ));
        }
        if self.id != folder_id {
            return Err(format!(
                "id {:?} must equal the folder name {folder_id:?}",
                self.id
            ));
        }
        let (title, summary) = (self.title.trim(), self.summary.trim());
        if title.is_empty() || too_long(title, 40) {
            return Err("title must be 1 to 40 characters".into());
        }
        if summary.is_empty() || too_long(summary, 200) {
            return Err("summary must be 1 to 200 characters".into());
        }
        for n in &self.needs {
            let name = n.name.trim();
            if name.is_empty() || too_long(name, 80) {
                return Err("a need's name must be 1 to 80 characters".into());
            }
            if let Some(u) = &n.url {
                if !u.starts_with("https://") || u.contains(char::is_whitespace) {
                    return Err(format!("need {name:?}: url must start with https://"));
                }
            }
        }
        if let Some(os) = &self.os {
            if os.is_empty() {
                return Err("os is empty; list windows, macos and/or linux".into());
            }
            if let Some(bad) = os.iter().find(|o| !OSES.contains(&o.as_str())) {
                return Err(format!(
                    "os {bad:?} must be one of {} (WSL is linux)",
                    OSES.join(", ")
                ));
            }
        }
        for (i, p) in self.plugins.iter().enumerate() {
            if !plugin_ok(p) {
                return Err(format!("plugin {p:?} must look like name@marketplace"));
            }
            if self.plugins[..i].contains(p) {
                return Err(format!("plugin {p} is listed twice"));
            }
        }
        for f in &self.files {
            if !file_path_ok(f) {
                return Err(format!(
                    "file {f:?} must be skills/<name> or agents/<name>.md"
                ));
            }
        }
        if let Some(c) = &self.claude_md {
            if c != "CLAUDE.md" {
                return Err(format!("claudeMd must be \"CLAUDE.md\", not {c:?}"));
            }
        }
        if self.setup != "setup.md" {
            return Err(format!("setup must be \"setup.md\", not {:?}", self.setup));
        }
        let run = &self.check.run;
        if run.is_empty() || run.len() > MAX_CHECK_ARGS {
            return Err(format!(
                "check.run needs 1 to {MAX_CHECK_ARGS} items, not {}",
                run.len()
            ));
        }
        if run.iter().any(|a| a.is_empty() || a.contains('\0')) {
            return Err("check.run has an empty item or a NUL character".into());
        }
        match self.check.timeout_seconds {
            Some(t) if !(1..=MAX_TIMEOUT).contains(&t) => Err(format!(
                "check.timeoutSeconds {t} must be from 1 to {MAX_TIMEOUT}"
            )),
            _ => Ok(()),
        }
    }
}
