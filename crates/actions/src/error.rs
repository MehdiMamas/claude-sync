/// A plain-language message the window shows as it is (v1 texts where they exist).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(s: impl Into<String>) -> Self {
        Error(s.into())
    }
}

impl From<claude_sync_github::Error> for Error {
    fn from(e: claude_sync_github::Error) -> Self {
        Error(e.to_string())
    }
}
impl From<claude_sync_store::Error> for Error {
    fn from(e: claude_sync_store::Error) -> Self {
        Error(e.to_string())
    }
}
impl From<claude_sync_engine::traits::IoError> for Error {
    fn from(e: claude_sync_engine::traits::IoError) -> Self {
        Error(e.0)
    }
}
impl From<claude_sync_engine::formats::FormatError> for Error {
    fn from(e: claude_sync_engine::formats::FormatError) -> Self {
        Error(e.to_string())
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}
