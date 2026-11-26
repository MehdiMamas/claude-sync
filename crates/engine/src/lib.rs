//! The pure engine of claude-sync 2: no file or network access, bytes in and bytes out.
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo)
)]

pub mod formats;
