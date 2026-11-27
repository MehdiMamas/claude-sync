//! The one GitHub client of claude-sync 2 (v2-Endpoints): REST with an ETag cache for GETs,
//! per-resource rate limits, request counts by caller, the OAuth device flow, token storage
//! and the engine's `Remote` on top. Blocking on purpose: the engine's traits are synchronous
//! and the async loop calls this through `spawn_blocking` (see the engine's `traits.rs`).
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo)
)]

pub mod api;
mod client;
pub mod device;
mod error;
pub mod remote;
pub mod secrets;

pub use client::{with_caller, Client, Config, Count, Response};
pub use error::{Error, Result};
pub use secrets::Tokens;

/// Public OAuth client id, set at build time (empty: sign-in not configured).
pub const CLIENT_ID: &str = match option_env!("CLAUDE_SYNC_GITHUB_CLIENT_ID") {
    Some(id) => id,
    None => "",
};
/// Slug of the GitHub App for `https://github.com/apps/<slug>`; the build variable wins (as v1).
pub const APP_SLUG: &str = match option_env!("CLAUDE_SYNC_GITHUB_APP_SLUG") {
    Some(s) => s,
    None => "claude-sync-app-github",
};
/// The relay, set at build time; the default is on the reserved `.invalid` TLD so a build
/// without it never sends a token anywhere.
pub const RELAY_URL: &str = match option_env!("CLAUDE_SYNC_RELAY_URL") {
    Some(u) => u,
    None => "https://claude-sync-relay.invalid",
};
