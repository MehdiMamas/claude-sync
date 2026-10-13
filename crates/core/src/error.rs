use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not signed in")]
    NotSignedIn,
    #[error("GitHub returned {status}: {message}")]
    GitHub { status: u16, message: String },
    #[error("rate limited by GitHub until {reset_unix}")]
    RateLimited { reset_unix: u64 },
    #[error("network error: {0}")]
    Network(String),
    #[error("someone else pushed first and the retries ran out")]
    PushRace,
    #[error("secret found in {path}:{line} ({pattern}); nothing was committed")]
    SecretFound { path: String, line: usize, pattern: String },
    #[error("{path} is not one JSON object; nothing was merged")]
    InvalidJson { path: String },
    #[error("repo {0} is public; claude-sync only uses private repos")]
    PublicRepo(String),
    #[error("group file schema {0} is newer than this app; update the app")]
    SchemaTooNew(u32),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
