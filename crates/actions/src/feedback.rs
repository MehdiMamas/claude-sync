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
    Later,
    Refused(String),
}

fn send(c: &claude_sync_github::Client, payload: &Value) -> Sent {
    match c.feedback(payload) {
        Ok(r) => Sent::Filed(r.number, r.url),
        Err(GhError::NotSignedIn) => Sent::Refused(msg::FEEDBACK_SIGN_IN.into()),
        Err(GhError::Github { status, .. }) if status == 429 || status >= 500 => Sent::Later,
        Err(GhError::Github { message, .. }) => Sent::Refused(message),
        Err(GhError::Network(_) | GhError::RateLimited { .. }) => Sent::Later,
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
                number: Some(n),
                url: Some(u),
            }),
            Sent::Later => {
                self.db.queue_feedback(&payload.to_string())?;
                Ok(Event::Feedback {
                    queued: true,
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
                Sent::Later => break,
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
