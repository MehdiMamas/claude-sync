//! User-facing texts that must stay as v1 had them (v1-MsgGroups, v1-MsgSignInSettings,
//! v1-MsgGithubHelper, v1-EngGroupLifecycle). The window shows these as they are.

pub const NO_CLIENT_ID: &str = "This copy of claude-sync was built without a GitHub client ID, so it cannot sign in. Install an official build, or rebuild with the client ID set.";
pub const NO_APP: &str = "This build has no GitHub App.";
pub const SESSION_EXPIRED: &str = "Your GitHub sign-in expired. Sign in again.";
pub const PLUGINS_WARNING: &str =
    "Plugins in this group run code on every member's machine; only join groups whose members you trust.";
pub const CLAUDE_NOT_FOUND: &str =
    "Claude Code not found (the claude command), so the group's plugins were not installed";
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

pub const PERSONAL_JOINED: &str =
    "Personal memory from your other devices is on. Turn off in Settings.";
pub const PERSONAL_OFFER: &str =
    "Your other devices share personal memory. Open claude-sync to turn it on here too.";
pub const PERSONAL_OFF_ELSEWHERE: &str =
    "Personal memory was turned off on another device, so it is off here too.";
pub const CANT_REMOVE_SELF: &str =
    "this device can't remove itself; turn personal memory off instead";
pub fn cant_reach(repo: &str, e: &str) -> String {
    format!("can't reach {repo}: {e}; turn personal memory off and on to start it again")
}
pub fn not_personal(repo: &str) -> String {
    format!("{repo} exists but is not a claude-sync personal memory repo; rename or delete it on GitHub")
}
pub fn personal_mod(repo: &str) -> String {
    format!("{repo} is used by the personal-rules mod, not by personal memory. Personal memory can't start until that mod moves to its own repo (an update is on the way).")
}
pub fn personal_foreign(repo: &str) -> String {
    format!("{repo} already exists on GitHub and wasn't made by claude-sync, so personal memory won't use it. Rename or delete that repo on GitHub, then turn this on again.")
}

pub fn own_proposal() -> &'static str {
    "you cannot approve your own proposal; another member has to"
}
pub fn needs_update(n: u64) -> String {
    format!("proposal #{n} needs an update before it can be merged")
}
pub fn not_open(n: u64) -> String {
    format!("proposal #{n} is not open")
}
pub fn declined_note(login: &str) -> String {
    format!("Declined by {login} in claude-sync.")
}
pub fn no_push(group: &str, host: &str) -> String {
    format!("You can't push to {group}: your GitHub account can read it but not write to it. Ask its host ({host}) for write access.")
}
pub fn copy_same_group() -> &'static str {
    "pick another group to copy to"
}
pub fn no_such_item(from: &str, what: &str, name: &str) -> String {
    format!("{from} has no {what} named {name}")
}
pub fn different_item(to: &str, what: &str, name: &str) -> String {
    format!("{to} already has a different {what} named {name}")
}
pub fn first_commit(sha: &str) -> String {
    format!("{sha} is the first commit and cannot be reverted")
}
pub fn nothing_reverted(why: &str) -> String {
    format!("Nothing was reverted. {why}")
}
pub fn nothing_to_revert(sha: &str) -> String {
    format!("Nothing to revert: the files of {sha} already match what they were before it")
}
pub fn changed_again(path: &str) -> String {
    format!("{path} changed again after this commit; revert that first.")
}

pub const TITLE_NEEDED: &str = "a title is needed";
pub const BAD_KIND: &str = "kind must be bug or idea";
pub const FEEDBACK_SIGN_IN: &str = "sign in to send feedback";
pub const SHOT_TOO_LARGE: &str = "screenshot is larger than 3 MB";
pub const SHOT_BAD: &str = "only PNG and JPEG images can be attached";

pub const TEST_NOTIFICATION: &str = "Notifications from claude-sync show up like this.";

/// v1 title of the notification for an edit made by hand to a shared file.
pub const LOCAL_EDIT_TITLE: &str = "Local change to a shared file";
