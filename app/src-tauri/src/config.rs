//! Compile-time configuration.

/// The relay (§12). Set `CLAUDE_SYNC_RELAY_URL` at build time. The default is a
/// placeholder on the reserved `.invalid` TLD, so a build without it never sends
/// the user's token anywhere.
pub const RELAY_URL: &str = match option_env!("CLAUDE_SYNC_RELAY_URL") {
    Some(u) => u,
    None => "https://claude-sync-relay.invalid",
};

/// Public client ID of the GitHub OAuth App (device flow, §3). Empty = not configured.
pub const GITHUB_CLIENT_ID: &str = match option_env!("CLAUDE_SYNC_GITHUB_CLIENT_ID") {
    Some(id) => id,
    None => "",
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const DEFAULT_POLL_SECONDS: u64 = 60;
pub const MIN_POLL_SECONDS: u64 = 15;
pub const INVITATIONS_EVERY_SECONDS: u64 = 5 * 60;
pub const UPDATES_EVERY_SECONDS: u64 = 6 * 60 * 60;
/// How much of the log goes with a feedback report.
pub const FEEDBACK_LOG_BYTES: usize = 200 * 1024;
/// Started by the login item: stay in the tray.
pub const BACKGROUND_ARG: &str = "--background";

pub fn relay_configured() -> bool {
    !RELAY_URL.ends_with(".invalid")
}

pub fn relay(path: &str) -> String {
    format!("{}{}", RELAY_URL.trim_end_matches('/'), path)
}

pub fn os_label() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}
