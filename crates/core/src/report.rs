//! The status text shared by `claude-sync status` (app) and `claude-sync-cli status`.
use crate::engine::GroupStatus;
use crate::personal::PersonalStatus;
use crate::{Engine, Paths};
use serde_json::{json, Value};

/// Everything `status` shows, read once.
pub struct StatusReport {
    pub login: Option<String>,
    pub expired_login: Option<String>,
    pub groups: Vec<GroupStatus>,
    pub groups_error: Option<String>,
    pub personal: Option<PersonalStatus>,
    pub requests: Option<crate::github::RequestSnapshot>,
}

impl StatusReport {
    /// `engine` is `None` when signed out.
    pub async fn gather(
        paths: &Paths,
        login: Option<String>,
        expired_login: Option<String>,
        engine: Option<&Engine>,
    ) -> Self {
        let (groups, groups_error) = match engine {
            Some(e) => match e.groups().await {
                Ok(g) => (g, None),
                Err(err) => (Vec::new(), Some(err.to_string())),
            },
            None => (Vec::new(), None),
        };
        Self {
            login,
            expired_login,
            groups,
            groups_error,
            personal: engine.map(|e| e.personal_status()),
            requests: crate::github::read_request_counts(&paths.requests_file()),
        }
    }

    /// The `--json` form (keys are a public interface).
    pub fn json(&self) -> Value {
        json!({
            "requests": self.requests,
            "login": self.login,
            "expiredLogin": self.expired_login,
            "groups": self.groups,
            "groupsError": self.groups_error,
            "personal": self.personal,
        })
    }

    pub fn text(&self, now: u64) -> String {
        let mut out = status_text(
            self.login.as_deref(),
            self.expired_login.as_deref(),
            &self.groups,
            self.personal.as_ref(),
            now,
        );
        if let Some(e) = &self.groups_error {
            out.push_str(&format!("\nCould not read the groups: {e}"));
        }
        out.push_str(&requests_text(self.requests.as_ref()));
        out
    }
}

/// "5 min ago", "never".
pub fn ago(now: u64, then: Option<u64>) -> String {
    let Some(t) = then else {
        return "never".into();
    };
    let s = now.saturating_sub(t);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

pub fn personal_text(p: &PersonalStatus, now: u64) -> String {
    if !p.enabled {
        return "Personal memory: off".into();
    }
    let k = &p.kinds;
    let kinds: Vec<&str> = [
        (k.claude_md, "CLAUDE.md"),
        (k.memory, "memory"),
        (k.state, "state"),
        (k.settings, "settings"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    let mut out = format!(
        "Personal memory: on ({})\n  kinds: {}\n  last sync: {}",
        p.repo,
        if kinds.is_empty() {
            "none".into()
        } else {
            kinds.join(", ")
        },
        ago(now, p.last_sync_unix)
    );
    if let Some(e) = &p.last_error {
        out.push_str(&format!("\n  last error: {e}"));
    }
    out
}

/// The top 10 GitHub requests of the last 10 minutes, as the running app counted them.
pub fn requests_text(requests: Option<&crate::github::RequestSnapshot>) -> String {
    let Some(r) = requests else {
        return String::new();
    };
    let mut out = String::from(
        "
GitHub requests, last 10 min (count, endpoint, result, caller):",
    );
    if r.last10min.is_empty() {
        out.push_str(
            "
  none",
        );
    }
    for line in crate::github::top_lines(&r.last10min, 10) {
        out.push_str(&format!(
            "
  {line}"
        ));
    }
    out
}

pub fn status_text(
    login: Option<&str>,
    expired: Option<&str>,
    groups: &[GroupStatus],
    personal: Option<&PersonalStatus>,
    now: u64,
) -> String {
    let mut out = match (login, expired) {
        (Some(l), _) => format!("Signed in as {l}"),
        (None, Some(l)) => format!("Sign-in expired for {l}: open the app to sign in again"),
        (None, None) => return "Not signed in".into(),
    };
    if groups.is_empty() {
        out.push_str("\nGroups: none");
    } else {
        out.push_str("\nGroups:");
    }
    for g in groups {
        let state = if g.disabled {
            "off on this device"
        } else {
            "on"
        };
        out.push_str(&format!(
            "\n  {} ({state}), last sync {}",
            g.id,
            ago(now, g.last_sync_unix)
        ));
        if let Some(e) = &g.last_error {
            out.push_str(&format!("\n    last error: {e}"));
        }
    }
    if let Some(p) = personal {
        out.push('\n');
        out.push_str(&personal_text(p, now));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::personal::Kinds;

    #[test]
    fn status_shows_the_top_requests() {
        use crate::github::{RequestEntry, RequestSnapshot};
        let row = |count, endpoint: &str| RequestEntry {
            caller: "tick".into(),
            endpoint: endpoint.into(),
            result: "200".into(),
            count,
        };
        let snap = RequestSnapshot {
            since: 0,
            last10min: (1..=12).map(|n| row(n, &format!("GET e{n}"))).collect(),
            total: vec![],
        };
        let text = requests_text(Some(&snap));
        assert!(text.contains("GET e1  200  (tick)"));
        assert_eq!(text.lines().count(), 1 + 1 + 10, "heading and 10 rows");
        assert_eq!(requests_text(None), "");
        // The same file the app writes is what `status --json` embeds.
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["last10min"][0]["endpoint"], "GET e1");
    }

    fn personal(enabled: bool) -> PersonalStatus {
        PersonalStatus {
            enabled,
            decided: true,
            offer: false,
            kinds: Kinds {
                settings: false,
                ..Kinds::default()
            },
            repo: "me/claude-sync-personal".into(),
            repo_url: String::new(),
            device_id: String::new(),
            devices: vec![],
            new_devices_join: None,
            last_sync_unix: Some(1000),
            last_error: Some("offline".into()),
            rate_limited_until: None,
            conflicts: vec![],
            skipped: vec![],
        }
    }

    #[test]
    fn personal_text_on_and_off() {
        assert_eq!(
            personal_text(&personal(false), 2000),
            "Personal memory: off"
        );
        let t = personal_text(&personal(true), 1000 + 300);
        assert!(t.contains("on (me/claude-sync-personal)"));
        assert!(t.contains("kinds: CLAUDE.md, memory, state\n"));
        assert!(t.contains("last sync: 5 min ago"));
        assert!(t.contains("last error: offline"));
    }

    #[test]
    fn status_text_signed_out_and_in() {
        assert_eq!(status_text(None, None, &[], None, 0), "Not signed in");
        assert!(status_text(None, Some("me"), &[], None, 0).contains("expired for me"));
        let t = status_text(Some("me"), None, &[], Some(&personal(false)), 0);
        assert_eq!(t, "Signed in as me\nGroups: none\nPersonal memory: off");
    }

    #[test]
    fn ago_text() {
        assert_eq!(ago(10, None), "never");
        assert_eq!(ago(100, Some(90)), "just now");
        assert_eq!(ago(10_000, Some(10_000 - 7200)), "2 h ago");
        assert_eq!(ago(300_000, Some(0)), "3 d ago");
    }
}
