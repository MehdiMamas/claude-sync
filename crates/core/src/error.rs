use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not signed in")]
    NotSignedIn,
    /// GitHub no longer accepts the sign-in and it could not be refreshed.
    #[error("Your GitHub sign-in expired. Sign in again.")]
    SignInExpired,
    #[error("GitHub returned {status}: {message}")]
    GitHub { status: u16, message: String },
    #[error("GitHub limit reached, resumes at {}", reset_text(*reset_unix))]
    RateLimited { reset_unix: u64 },
    #[error("network error: {0}")]
    Network(String),
    #[error("someone else pushed first and the retries ran out")]
    PushRace,
    #[error("secret found in {path}:{line} ({pattern}); nothing was committed")]
    SecretFound {
        path: String,
        line: usize,
        pattern: String,
    },
    #[error("{path} is not one JSON object; nothing was merged")]
    InvalidJson { path: String },
    #[error("repo {0} is public; claude-sync only uses private repos")]
    PublicRepo(String),
    #[error("group file schema {0} is newer than this app; update the app")]
    SchemaTooNew(u32),
    /// A member tried to change the group; only the host's app writes to it.
    #[error("Only {host} can change {group}. Your change stays on this machine; ask {host}, or propose it as a pull request.")]
    HostOnly { group: String, host: String },
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// When the GitHub limit lifts, in the user's time zone: "5:24 PM" today, "Tue 5:24 PM" on
/// another day.
pub fn reset_text(reset_unix: u64) -> String {
    use chrono::{Local, TimeZone};
    let now = Local::now().fixed_offset();
    match Local.timestamp_opt(reset_unix as i64, 0).single() {
        Some(at) => format_reset(at.fixed_offset(), now),
        None => reset_unix.to_string(),
    }
}

fn format_reset(
    at: chrono::DateTime<chrono::FixedOffset>,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> String {
    let same_day = at.date_naive() == now.with_timezone(at.offset()).date_naive();
    at.format(if same_day {
        "%-I:%M %p"
    } else {
        "%a %-I:%M %p"
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, FixedOffset};

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn a_reset_today_shows_only_the_time() {
        let now = at("2026-10-27T09:00:00+02:00");
        assert_eq!(
            format_reset(at("2026-10-27T17:24:00+02:00"), now),
            "5:24 PM"
        );
    }

    #[test]
    fn a_reset_on_another_day_shows_the_weekday() {
        let now = at("2026-10-27T09:00:00+02:00");
        assert_eq!(
            format_reset(at("2026-10-28T08:05:00+02:00"), now),
            "Wed 8:05 AM"
        );
    }

    #[test]
    fn today_is_judged_in_the_zone_of_the_reset() {
        // 23:30 UTC is already the next day at +02:00.
        let now = at("2026-10-27T23:30:00+00:00");
        assert_eq!(
            format_reset(at("2026-10-28T09:00:00+02:00"), now),
            "9:00 AM"
        );
    }

    #[test]
    fn the_error_text_names_the_time() {
        let text = Error::RateLimited { reset_unix: 0 }.to_string();
        assert!(
            text.starts_with("GitHub limit reached, resumes at "),
            "{text}"
        );
    }
}
