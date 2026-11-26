//! User-facing texts that must stay as v1 had them (v1-MsgGroups, v1-MsgSignInSettings,
//! v1-MsgGithubHelper, v1-EngGroupLifecycle). The window shows these as they are.

pub const NO_CLIENT_ID: &str = "This copy of claude-sync was built without a GitHub client ID, so it cannot sign in. Install an official build, or rebuild with the client ID set.";
pub const NO_APP: &str = "This build has no GitHub App.";
pub const SESSION_EXPIRED: &str = "Your GitHub sign-in expired. Sign in again.";
pub const PLUGINS_WARNING: &str =
    "Plugins in this group run code on every member's machine; only join groups whose members you trust.";
pub const LOCK_UNAVAILABLE: &str =
    "GitHub can't lock this repo on your plan; the app still only sends changes from your machine.";

/// How the GitHub client words an unreachable network and a spent rate limit (v1 texts); a
/// group whose last pass failed with one of them is Offline or RateLimited, not Failed.
pub const OFFLINE_PREFIX: &str = "network error";
pub const RATE_LIMITED_PREFIX: &str = "GitHub limit reached";

pub fn session_expired_for(login: &str) -> String {
    format!("GitHub no longer accepts the sign-in of {login}, so syncing stopped. Sign in again to pick up where you left off; your groups and files on this computer are kept.")
}
pub fn public_repo(repo: &str) -> String {
    format!("repo {repo} is public; claude-sync only uses private repos")
}
pub fn not_a_group(id: &str) -> String {
    format!("{id} is not a claude-sync group (no claude-sync.json)")
}
pub fn host_only(host: &str, group: &str) -> String {
    format!("Only {host} can change {group}. Your change stays on this machine; ask {host}, or propose it as a pull request.")
}
pub fn not_a_login(login: &str) -> String {
    format!("{login:?} is not a GitHub login")
}
pub fn invited(who: &str) -> String {
    format!("Invited {who}. They get an invitation from GitHub.")
}
pub fn transfer_asked(who: &str) -> String {
    format!("Asked {who} to accept the group. Nothing changes until they do.")
}
pub fn left(name: &str) -> String {
    format!("Left {name}. Its files were removed from this device.")
}
pub fn host_cannot_be_removed(login: &str, id: &str) -> String {
    format!("{login} hosts {id} and cannot be removed")
}
pub fn host_cannot_leave(id: &str) -> String {
    format!("You host {id}. Hand it over to another member first, or the group would have no host.")
}
pub fn not_a_member(who: &str, id: &str) -> String {
    format!("{who} is not a member of {id}; add them first")
}
pub fn already_host(id: &str) -> String {
    format!("you already host {id}")
}
pub fn order_mismatch() -> &'static str {
    "the new order must list exactly the joined groups"
}
pub fn access_gone(id: &str) -> String {
    format!("access to {id} is gone; its files were removed from this machine")
}
pub fn secret_blocked(path: &str, pattern: &str) -> String {
    format!("{path} looks like it holds a secret ({pattern}) and was not shared")
}
pub fn push_retries() -> &'static str {
    "someone else pushed first and the retries ran out"
}
pub fn no_claude_md_to_review(id: &str) -> String {
    format!("{id} has no CLAUDE.md on this device to review")
}
pub fn own_line_gone() -> &'static str {
    "your CLAUDE.md no longer has that line"
}
pub fn group_line_gone(id: &str) -> String {
    format!("{id}'s CLAUDE.md no longer has that line")
}
pub fn secret_found(path: &str, line: usize, pattern: &str) -> String {
    format!("secret found in {path}:{line} ({pattern}); nothing was committed")
}
pub fn not_a_skill_name(name: &str) -> String {
    format!("{name:?} is not a skill name")
}
pub fn no_skill_md(name: &str) -> String {
    format!("{name} has no SKILL.md, so it is not a skill")
}
pub fn no_files_to_share(name: &str) -> String {
    format!("{name} has no files to share")
}
pub fn does_not_sync(id: &str, what: &str) -> String {
    format!("{id} does not sync {what}")
}
pub fn skill_exists(id: &str, name: &str) -> String {
    format!("{id} already has a skill named {name}")
}
pub fn no_text_to_share() -> &'static str {
    "there is no text to share"
}
pub fn not_an_edit(path: &str) -> String {
    format!("{path} has no local edit")
}
