//! Notifications (Parity 49) and feedback (Parity 47, 50, 70): a report goes to the relay with
//! the person's GitHub token; offline or rate limited, it waits in the store and goes later.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{Event, FeedbackInput};
use crate::msg;
use base64::Engine as _;
use claude_sync_fs::redact::redact;
use claude_sync_github::Error as GhError;
use serde_json::{json, Value};

const MAX_SHOT: usize = 3 * 1024 * 1024;
const MAX_LOG: usize = 200 * 1024;

enum Sent {
    Filed(u64, String),
    /// Network trouble, a 429 or a 5xx: keep the report.
    Later {
        limited: bool,
    },
    Refused(String),
}

fn send(c: &claude_sync_github::Client, payload: &Value) -> Sent {
    match c.feedback(payload) {
        Ok(r) => Sent::Filed(r.number, r.url),
        Err(GhError::NotSignedIn) => Sent::Refused(msg::FEEDBACK_SIGN_IN.into()),
        Err(GhError::Github { status: 429, .. }) => Sent::Later { limited: true },
        Err(GhError::Github { status, .. }) if status >= 500 => Sent::Later { limited: false },
        Err(GhError::Github { message, .. }) => Sent::Refused(message),
        Err(GhError::RateLimited { .. }) => Sent::Later { limited: true },
        Err(GhError::Network(_)) => Sent::Later { limited: false },
        Err(e) => Sent::Refused(e.to_string()),
    }
}

/// The last 200 KiB of the logs, redacted.
fn logs(text: &str) -> String {
    let mut t = text;
    if t.len() > MAX_LOG {
        let mut i = t.len() - MAX_LOG;
        while !t.is_char_boundary(i) {
            i += 1;
        }
        t = &t[i..];
    }
    redact(t)
}

impl Core {
    /// `<state>/logs/claude-sync.<date>.log`, the newest by name (v1's place and names).
    pub(crate) fn recent_logs(&self) -> Result<Event> {
        let dir = self.home.state.join("logs");
        let newest = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with("claude-sync.") && n.ends_with(".log")
            })
            .max_by_key(|e| e.file_name());
        let text = match newest {
            Some(e) => String::from_utf8_lossy(&std::fs::read(e.path())?).into_owned(),
            None => String::new(),
        };
        Ok(Event::Text(logs(&text)))
    }

    pub(crate) fn notes(
        &mut self,
        f: impl FnOnce(&claude_sync_store::Db) -> claude_sync_store::Result<()>,
    ) -> Result<Event> {
        f(&self.db)?;
        self.show();
        Ok(Event::Done)
    }

    pub(crate) fn send_feedback(&mut self, f: FeedbackInput) -> Result<Event> {
        let title = f.title.trim().to_string();
        if title.is_empty() {
            return Err(Error::new(msg::TITLE_NEEDED));
        }
        if !matches!(f.kind.as_str(), "bug" | "idea") {
            return Err(Error::new(msg::BAD_KIND));
        }
        if let Some(s) = &f.screenshot {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(s.trim())
                .map_err(|_| Error::new(msg::SHOT_BAD))?;
            if bytes.len() > MAX_SHOT {
                return Err(Error::new(msg::SHOT_TOO_LARGE));
            }
            if !(bytes.starts_with(&[0x89, b'P', b'N', b'G'])
                || bytes.starts_with(&[0xff, 0xd8, 0xff]))
            {
                return Err(Error::new(msg::SHOT_BAD));
            }
        }
        self.me().map_err(|_| Error::new(msg::FEEDBACK_SIGN_IN))?;
        let payload = json!({
            "title": title, "description": f.description, "kind": f.kind,
            "appVersion": f.app_version, "os": f.os,
            "logs": logs(&f.logs), "screenshot": f.screenshot,
        });
        match send(&self.client, &payload) {
            Sent::Filed(n, u) => Ok(Event::Feedback {
                queued: false,
                limited: false,
                number: Some(n),
                url: Some(u),
            }),
            Sent::Later { limited } => {
                self.db.queue_feedback(&payload.to_string())?;
                Ok(Event::Feedback {
                    queued: true,
                    limited,
                    number: None,
                    url: None,
                })
            }
            Sent::Refused(e) => Err(Error::new(e)),
        }
    }

    /// Sends the saved reports once `/api/health` answers; stops at the first one that has to wait.
    pub(crate) fn retry_feedback(&mut self) -> Result<Event> {
        let queue = self.db.feedback_queue()?;
        if queue.is_empty() || self.client.health().is_err() {
            return Ok(Event::Done);
        }
        let now = self.now();
        for (id, body) in queue {
            let Ok(payload) = serde_json::from_str::<Value>(&body) else {
                self.db.drop_feedback(id)?;
                continue;
            };
            let title = payload["title"].as_str().unwrap_or_default().to_string();
            match send(&self.client, &payload) {
                Sent::Filed(n, _) => {
                    self.db.drop_feedback(id)?;
                    let t = format!("\"{title}\" was filed as #{n}.");
                    self.db.add_notification("feedback", &t, None, now)?;
                }
                Sent::Later { .. } => break,
                Sent::Refused(e) => {
                    self.db.drop_feedback(id)?;
                    let t = format!("\"{title}\" was not filed: {e}");
                    self.db.add_notification("feedback", &t, None, now)?;
                }
            }
        }
        self.show();
        Ok(Event::Done)
    }
}
