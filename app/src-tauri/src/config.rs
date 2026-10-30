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

/// Public client ID of the GitHub App (per-repo access, issue #39), used when the
/// "GitHub App" setting is on. `CLAUDE_SYNC_GITHUB_APP_CLIENT_ID` overrides it.
pub const GITHUB_APP_CLIENT_ID: &str = match option_env!("CLAUDE_SYNC_GITHUB_APP_CLIENT_ID") {
    Some(id) => id,
    None => "Iv23litHw8qKqwlLcKNV",
};

/// Slug of the GitHub App, for `https://github.com/apps/<slug>`.
pub const GITHUB_APP_SLUG: &str = match option_env!("CLAUDE_SYNC_GITHUB_APP_SLUG") {
    Some(s) => s,
    None => "claude-sync-app-github",
};

/// The client ID sign-in and token refresh use: the App's when `github_app` is on and the
/// build has one, else the OAuth App's (the default).
pub fn client_id(github_app: bool) -> &'static str {
    if github_app && !GITHUB_APP_CLIENT_ID.is_empty() {
        GITHUB_APP_CLIENT_ID
    } else {
        GITHUB_CLIENT_ID
    }
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `identifier` in tauri.conf.json: the app id of notifications sent outside Tauri (the job
/// runner). Linux notifications carry no app id.
#[cfg(any(windows, target_os = "macos", test))]
pub const IDENTIFIER: &str = "com.mehdimamas.claudesync";

pub const DEFAULT_POLL_SECONDS: u64 = 60;
pub const MIN_POLL_SECONDS: u64 = 15;
pub const INVITATIONS_EVERY_SECONDS: u64 = 15 * 60;
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

#[cfg(test)]
mod tests {
    #[test]
    fn identifier_matches_the_tauri_config() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], super::IDENTIFIER);
    }
}
