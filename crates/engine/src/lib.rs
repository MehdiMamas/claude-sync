//! The pure engine of claude-sync 2: no file or network access, bytes in and bytes out.
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo)
)]

pub mod clashes;
pub mod claude_md;
pub mod formats;
pub mod merge;
pub mod paths;
pub mod plan;
pub mod review;
pub mod secrets;
pub mod settings;
pub mod settings_edits;
pub mod traits;
