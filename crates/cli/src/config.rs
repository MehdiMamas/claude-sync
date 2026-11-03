//! Compile-time configuration; the same variables the app is built with.

/// The relay (§12). A build without `CLAUDE_SYNC_RELAY_URL` never sends a token anywhere.
pub const RELAY_URL: &str = match option_env!("CLAUDE_SYNC_RELAY_URL") {
    Some(u) => u,
    None => "https://claude-sync-relay.invalid",
};

/// Public client ID of the GitHub OAuth App (device flow). Empty = not configured.
pub const GITHUB_CLIENT_ID: &str = match option_env!("CLAUDE_SYNC_GITHUB_CLIENT_ID") {
    Some(id) => id,
    None => "",
};

/// Public client ID of the GitHub App (per-repo access), used when the state has `githubApp`.
pub const GITHUB_APP_CLIENT_ID: &str = match option_env!("CLAUDE_SYNC_GITHUB_APP_CLIENT_ID") {
    Some(id) => id,
    None => "Iv23litHw8qKqwlLcKNV",
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_POLL_SECONDS: u64 = 60;
pub const MIN_POLL_SECONDS: u64 = 15;

/// The client ID sign-in and token refresh use for this device's sign-in method.
pub fn client_id(github_app: bool) -> &'static str {
    if github_app && !GITHUB_APP_CLIENT_ID.is_empty() {
        GITHUB_APP_CLIENT_ID
    } else {
        GITHUB_CLIENT_ID
    }
}

pub fn relay_configured() -> bool {
    !RELAY_URL.ends_with(".invalid")
}

pub fn relay(path: &str) -> String {
    format!("{}{}", RELAY_URL.trim_end_matches('/'), path)
}
