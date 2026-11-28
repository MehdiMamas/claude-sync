use chrono::{DateTime, Local};

pub type Result<T> = std::result::Result<T, Error>;

/// Messages are the v1 texts (v1-SpecGithub4) the window shows as they are.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("not signed in")]
    NotSignedIn,
    #[error("Your GitHub sign-in expired. Sign in again.")]
    SignInExpired,
    #[error("GitHub returned {status}: {message}")]
    Github { status: u16, message: String },
    #[error("GitHub limit reached, resumes at {}", resume_text(*reset_unix))]
    RateLimited { reset_unix: i64 },
    #[error("network error: {0}")]
    Network(String),
    /// Another device moved the branch first (a 422 on the ref update).
    #[error("someone else pushed first")]
    PushRace,
    #[error("{0}")]
    Invalid(String),
}

impl Error {
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Github { status, .. } => Some(*status),
            _ => None,
        }
    }
}

/// Time of day, with the weekday when it is not today, in local time (v1 error.rs:14).
fn resume_text(reset_unix: i64) -> String {
    let Some(t) = DateTime::from_timestamp(reset_unix, 0) else {
        return "later".into();
    };
    let t = t.with_timezone(&Local);
    let fmt = if t.date_naive() == Local::now().date_naive() {
        "%-I:%M %p"
    } else {
        "%a %-I:%M %p"
    };
    t.format(fmt).to_string()
}
