//! What the engine needs from the outside world. The engine never does I/O itself; the sync
//! loop (a later step) passes real ones, tests pass fakes. Signatures follow v2-Class.
//! shortcut: synchronous methods, the async loop wraps them in `spawn_blocking`; switch to
//! async traits only if the loop needs concurrent calls.
//!
//! v2-Parity rows covered: (v2-Class).
use crate::plan::FileSet;
use std::collections::BTreeMap;

/// A short plain-language error from the outside world.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct IoError(pub String);

pub type IoResult<T> = Result<T, IoError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub sha: String,
    pub etag: Option<String>,
}

/// A repo on GitHub (group or personal).
pub trait Remote {
    /// `None` when the head did not move (ETag match).
    fn head(&self, etag: Option<&str>) -> IoResult<Option<Head>>;
    /// The files at `sha`, reusing `known` (path to blob sha) so only changed paths download.
    fn files(&self, sha: &str, known: &BTreeMap<String, String>) -> IoResult<FileSet>;
    /// One commit with `pushes` (`None`: delete); returns the new head sha.
    fn push(
        &self,
        parent: &str,
        pushes: &BTreeMap<String, Option<Vec<u8>>>,
        message: &str,
    ) -> IoResult<String>;
}

/// Where the engine keeps its state (the choices, base snapshots, review state).
pub trait Store {
    fn read(&self, key: &str) -> IoResult<Option<Vec<u8>>>;
    fn write(&self, key: &str, bytes: &[u8]) -> IoResult<()>;
}

/// The user's folders.
pub trait Disk {
    fn read(&self, rel: &str) -> IoResult<Option<Vec<u8>>>;
    fn write(&self, rel: &str, bytes: &[u8]) -> IoResult<()>;
    fn delete(&self, rel: &str) -> IoResult<()>;
    /// Repo paths that are links on disk.
    fn links(&self) -> IoResult<Vec<String>>;
}

pub trait Clock {
    fn now_unix(&self) -> i64;
}
