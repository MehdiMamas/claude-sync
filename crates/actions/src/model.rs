//! Commands in, events out, snapshots for the window and tray (v2-Class: `handle(Command):
//! Result<Event>`). Snapshot types derive `ts_rs::TS` behind the `ts` feature.
use serde::{Deserialize, Serialize};

macro_rules! ts {
    ($($i:item)*) => {$(
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
        $i
    )*};
}

ts! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "camelCase")]
    pub enum SignInState {
        SignedOut,
        /// Asking GitHub for a code.
        Starting,
        Code { user_code: String, verification_uri: String, expires_in: u64 },
        Expired,
        Denied,
        Offline,
        /// The build has no client id.
        Unavailable,
        Failed { message: String },
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum GroupStatus {
        Synced,
        Syncing,
        Off,
        RateLimited,
        Offline,
        Failed,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct GroupView {
        pub repo: String,
        pub name: String,
        pub host: String,
        pub is_host: bool,
        pub status: GroupStatus,
        pub error: Option<String>,
        pub last_sync: Option<i64>,
        pub members: Vec<String>,
        pub proposal_mode: bool,
        pub auto_accept_claude_md: bool,
        /// Jobs of this group run without asking (this account's choice).
        pub jobs_auto_accept: bool,
        /// `on`, `unavailable` or unknown (not tried yet).
        pub host_lock: Option<String>,
        pub review_choice: crate::choices::ReviewChoice,
        pub pending_review: u32,
        /// Plugins that could not install and files the size cap kept out (Parity 26-28).
        pub plugin_problems: Vec<String>,
        /// This group is being synced right now.
        pub syncing: bool,
        /// When the last pass failed for this group; `last_sync` is untouched.
        pub failed_at: Option<i64>,
        pub off: bool,
        /// Logins invited to this group that GitHub has not seen accepted (host only).
        pub pending_invites: Vec<String>,
        /// Hand edits to this group's files, as the watcher last saw them (Parity 24).
        pub edits: Vec<EditView>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct InvitationView {
        pub id: u64,
        pub repo: String,
        pub inviter: String,
        /// Unix seconds.
        pub created_at: Option<i64>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DiscoverView {
        pub repo: String,
        /// Read from the group file; `None` when it could not be read.
        pub members: Option<u32>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct NotificationView {
        pub id: i64,
        pub kind: String,
        pub text: String,
        pub read: bool,
        pub time: i64,
        pub repo: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct JobView {
        pub repo: String,
        pub id: String,
        pub accepted: bool,
        pub enabled: bool,
        pub last_result: String,
        /// Unix seconds of the last run; 0 when it never ran.
        pub last_run: i64,
        pub title: String,
        pub cron: String,
        pub prompt: String,
        pub cwd: String,
        pub mode: String,
        pub timeout_minutes: u32,
        pub args: Vec<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DeviceView {
        pub id: String,
        pub name: String,
        pub os: String,
        pub last_seen: i64,
        pub this: bool,
    }

    /// Settings, Personal memory (Parity 43, 44, 45, 46, 54, 87).
    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PersonalView {
        pub on: bool,
        /// The other devices share personal memory and this one was not asked yet.
        pub offer: bool,
        pub repo: String,
        /// The next new device turns personal memory on by itself (the repo's `newDevicesJoin`).
        pub new_devices_join: bool,
        /// `claudeMd`, `memory`, `state`, `settings` switched off.
        pub kinds_off: Vec<String>,
        pub devices: Vec<DeviceView>,
        /// Files that stay on this device, with the reason.
        pub stays: Vec<String>,
        pub error: Option<String>,
        pub last_sync: Option<i64>,
    }

    /// Everything the window and the tray read; they never touch state themselves.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Snapshot {
        pub login: Option<String>,
        pub sign_in: SignInState,
        /// The login whose token GitHub no longer accepts; syncing is stopped.
        pub session_expired: Option<String>,
        pub app_mode: bool,
        pub paused: bool,
        pub interval_secs: u64,
        pub syncing: bool,
        /// GitHub rate limit lifts at this Unix time.
        pub rate_limited_until: Option<i64>,
        pub groups: Vec<GroupView>,
        pub invitations: Vec<InvitationView>,
        /// Groups on GitHub the person can read but has not joined.
        pub discoverable: Vec<DiscoverView>,
        pub personal_on: bool,
        pub personal: PersonalView,
        pub unread: u32,
        pub notifications: Vec<NotificationView>,
        pub jobs: Vec<JobView>,
        pub ssh_on: bool,
        pub activity_on: bool,
        pub update: String,
        /// When the update check last ran (Unix time); `None` before the first check.
        pub update_checked: Option<i64>,
        /// Groups the GitHub App is not installed on (App mode). Filled when the engine reads the
        /// app's installations; empty until then.
        pub app_uncovered: Vec<String>,
        /// The GitHub App's slug, for its install and manage pages.
        pub app_slug: String,
        /// The sign-in screen starts the GitHub App sign-in (a v1 `githubApp` setting that was on).
        pub github_app: bool,
        /// Desktop notifications switch (Settings); on until turned off.
        pub desktop_notifications: bool,
        /// Claude helper switch (Settings, Advanced); on until turned off.
        pub helper: bool,
        /// `None` until the first start decided it.
        pub start_at_login: Option<bool>,
        /// What the v1 import left to say on the sign-in screen; `None` when all went well.
        pub install: Option<InstallNote>,
    }

    /// A v1 import that needs a word (v2-Cutover): files skipped, or a v1 daemon that did not stop.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "camelCase")]
    pub enum InstallNote {
        /// Files that could not be read; `total` is how many v1 files there were.
        Skipped { files: Vec<String>, total: u32 },
        /// The v1 daemon was asked to stop and did not in 30 s: nothing was imported.
        Daemon { pid: u32, version: String },
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ConflictView {
        pub repo: String,
        pub path: String,
        pub saved_at: i64,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct EditView {
        pub path: String,
        pub kept: bool,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ProposalFile {
        pub path: String,
        pub patch: String,
    }

    /// An open proposal: a pull request from a `proposal/<login>-<stamp>` branch (Parity 36, 83-85).
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ProposalView {
        pub number: u64,
        pub branch: String,
        pub author: String,
        pub title: String,
        pub url: String,
        pub files: Vec<ProposalFile>,
        /// Members (never the author) whose approval holds on the current head.
        pub approvals: Vec<String>,
        /// Behind main: tagged "needs an update", cannot be merged.
        pub needs_update: bool,
        pub mine: bool,
        pub approved_by_me: bool,
        pub head: String,
        /// When the pull request was opened (ISO time).
        pub created: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CopyItem {
        pub name: String,
        pub in_target: bool,
        /// The target has the same bytes, so copying changes nothing.
        pub same: bool,
    }

    /// What copying to another group would do (Parity 33).
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CopyPreviewView {
        pub to: String,
        pub to_name: String,
        pub host: String,
        pub can_push: bool,
        pub syncs_claude_md: bool,
        pub syncs_skills: bool,
        pub syncs_agents: bool,
        /// The source group's CLAUDE.md, secrets redacted; empty when it has none.
        pub claude_md: String,
        pub skills: Vec<CopyItem>,
        pub agents: Vec<CopyItem>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RevertFile {
        pub path: String,
        /// `restore`, `delete` or `recreate`.
        pub action: String,
        /// Why this file cannot go back (it changed again); `None` when it can.
        pub blocked: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RevertPlan {
        pub sha: String,
        pub summary: String,
        pub files: Vec<RevertFile>,
    }

    /// One commit of the group's history (Parity 35).
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct HistoryItem {
        pub sha: String,
        pub summary: String,
        pub author: String,
        pub at: Option<String>,
        /// `#123` chips found in the commit text.
        pub issues: Vec<u64>,
        pub before: Option<String>,
        pub now: Option<String>,
    }

    /// What a group gives this device (Parity 35): `claudeMd`, `skill`, `agent`, `command`,
    /// `outputStyle`, `settings`, `plugin`, `jobs`.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ContentItem {
        pub kind: String,
        pub name: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ClashSide {
        /// The group repo; `None` for the person's own CLAUDE.md.
        pub source: Option<String>,
        pub name: String,
        pub text: String,
    }

    /// Two lines of CLAUDE.md that say the same thing (`duplicate`) or the opposite (`contradiction`).
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ClashView {
        pub kind: String,
        /// For a `setting` clash: the setting's JSON pointer.
        pub target: Option<String>,
        pub a: ClashSide,
        pub b: ClashSide,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ReviewLine {
        pub id: String,
        pub add: bool,
        pub lines: Vec<String>,
        pub author: String,
        /// Unix seconds of the commit.
        pub at: u64,
    }

    /// One opt-in pack of a group, as this machine sees it.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PackView {
        pub repo: String,
        pub id: String,
        pub title: String,
        pub summary: String,
        /// `off`, `ready` (the check passed), `on` or `failed`.
        pub state: String,
        /// The pack is not for this computer's system.
        pub unsupported: bool,
        /// The systems the pack runs on (`windows`, `macos`, `linux`).
        pub os: Vec<String>,
        /// When the last check ran (unix seconds); none before the first check.
        pub checked_at: Option<i64>,
        /// Last line of the last check, redacted.
        pub tail: String,
    }
}

/// What the helper is asked to explain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ExplainTask {
    Collision,
    ClaudeMdReview,
    Activity,
}

/// One user action.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Command {
    // account
    SignInStart {
        app: bool,
    },
    SignInCancel,
    /// Read which joined groups the GitHub App cannot reach; fills `appUncovered`.
    AppCoverage,
    /// Sent by the sign-in waiter itself, never by the window.
    #[serde(skip)]
    #[cfg_attr(feature = "ts", ts(skip))]
    SignInDone(crate::signin::Outcome),
    SignOut,
    /// Sent by a plugin install task when it ends, never by the window.
    #[serde(skip)]
    #[cfg_attr(feature = "ts", ts(skip))]
    PluginsDone(crate::plugins::Done),
    // groups
    ReorderGroups(Vec<String>),
    CreateGroup {
        name: String,
        repo: String,
    },
    Discover,
    Join {
        repo: String,
    },
    AcceptInvitation {
        id: u64,
    },
    AddMember {
        repo: String,
        login: String,
    },
    RemoveMember {
        repo: String,
        login: String,
    },
    Transfer {
        repo: String,
        new_owner: String,
    },
    Leave {
        repo: String,
    },
    SetGroupOff {
        repo: String,
        off: bool,
    },
    // sync
    /// Sync every group now (the tray's and the window's Sync now).
    SyncNow,
    /// Run the plugin installs of a group again, now.
    PluginsRetry {
        repo: String,
    },
    // CLAUDE.md review (Parity 37, 38, 82)
    ReviewList {
        repo: String,
    },
    /// `ids` None: all pending. Accept applies the lines, decline remembers them.
    ReviewDecide {
        repo: String,
        ids: Option<Vec<String>>,
        accept: bool,
    },
    /// Take one line out of a group's text for this user only.
    DropLine {
        repo: String,
        line: String,
    },
    /// Bring every dropped or declined line back.
    UseGroupVersion {
        repo: String,
    },
    /// Edit a clashing line (Parity 38). `source` is the group's repo (the line is replaced, or
    /// removed when `text` is empty, in the group's CLAUDE.md and committed to the group: a
    /// proposal when that mode is on, else every member gets it through review) or `None` for
    /// the person's own `~/.claude/CLAUDE.md` (backed up first).
    EditLine {
        source: Option<String>,
        line: String,
        text: String,
    },
    Clashes,
    /// This person's choice for a group's CLAUDE.md lines (follows them to every machine).
    SetReviewChoice {
        repo: String,
        choice: crate::choices::ReviewChoice,
    },
    // local edits to delivered files (Parity 39, 40, 41, 42)
    EditsList {
        repo: String,
    },
    /// Keep my version: it is not shown as an edit again until it changes.
    KeepMine {
        repo: String,
        path: String,
    },
    /// Put the group's version back (mine is saved under backups first).
    UndoEdit {
        repo: String,
        path: String,
    },
    /// What sharing this edit would send: the added lines (redacted) and the first secret hit.
    SharePreview {
        repo: String,
        path: String,
    },
    ShareEdit {
        repo: String,
        path: String,
    },
    ShareSkill {
        repo: String,
        name: String,
    },
    ShareClaudeMd {
        repo: String,
    },
    Conflicts,
    /// Where a saved conflict copy lies (`Event::Path`), for the window to open (Parity 39).
    ConflictOpen {
        repo: String,
        path: String,
        saved_at: i64,
    },
    /// Delete a saved conflict copy and its list entry (Parity 39).
    ConflictDelete {
        repo: String,
        path: String,
        saved_at: i64,
    },
    // jobs (Parity 64, 73, 74, 75)
    JobAccept {
        repo: String,
        id: String,
    },
    JobEnable {
        repo: String,
        id: String,
        on: bool,
    },
    JobsAutoAccept {
        repo: String,
        on: bool,
    },
    JobRunNow {
        repo: String,
        id: String,
    },
    /// The newest log, or the report the last run wrote.
    JobFile {
        repo: String,
        id: String,
        report: bool,
    },
    // personal memory (Parity 43, 44, 45, 46)
    PersonalOn,
    PersonalOff,
    /// "Not now" on the offer: the check stays quiet.
    PersonalDismiss,
    /// Whether new devices join automatically (Parity 43); written to the personal repo.
    PersonalJoinAuto {
        on: bool,
    },
    PersonalKind {
        kind: String,
        on: bool,
    },
    /// Pushed on the next pass.
    DeviceRename {
        id: String,
        name: String,
    },
    DeviceRemove {
        id: String,
    },
    // proposals (Parity 36, 83, 84, 85)
    /// Host only.
    SetProposalMode {
        repo: String,
        on: bool,
    },
    /// Host only: members get CLAUDE.md changes without review (the group default).
    SetAutoAccept {
        repo: String,
        on: bool,
    },
    Proposals {
        repo: String,
    },
    /// Never your own; the host's approval merges.
    ProposalApprove {
        repo: String,
        number: u64,
    },
    ProposalDecline {
        repo: String,
        number: u64,
    },
    ProposalsAcceptAll {
        repo: String,
    },
    ProposalsDeclineAll {
        repo: String,
    },
    // copy, revert, history (Parity 33, 34, 35)
    CopyPreview {
        from: String,
        to: String,
    },
    Copy {
        from: String,
        to: String,
        req: CopyRequest,
    },
    /// Packs of one group, or of every group.
    Packs {
        repo: Option<String>,
    },
    /// The setup text the person hands to Claude; nothing is run.
    PackSetup {
        repo: String,
        id: String,
    },
    /// Open a terminal that runs Claude Code in the pack's folder with the setup message. The
    /// answer is a message, or the command to type when no terminal opens.
    PackLaunch {
        repo: String,
        id: String,
    },
    PackCheck {
        repo: String,
        id: String,
    },
    PackOn {
        repo: String,
        id: String,
    },
    PackOff {
        repo: String,
        id: String,
    },
    SshSet {
        on: bool,
    },
    SshAlias {
        alias: String,
    },
    ActivitySet {
        on: bool,
    },
    /// The window reads the idle time (the OS call is its) and sends it here.
    ActivityTick {
        idle_secs: Option<u64>,
    },
    /// Explain text others changed with the local `claude`; empty text (only my own changes) is skipped.
    Explain {
        task: ExplainTask,
        text: String,
    },
    CheckUpdate,
    /// The shell records the updater's state: `available:<v>`, `ready:<v>`, `failed:<v>` or empty.
    SetUpdate {
        state: String,
    },
    MarkRead {
        id: i64,
    },
    MarkAllRead,
    ClearNotifications,
    SendFeedback(FeedbackInput),
    /// Send the saved reports once the relay answers `/api/health`.
    RetryFeedback,
    /// The tail of the newest log file, redacted, for the feedback form (`Event::Text`).
    RecentLogs,
    /// Host only; writes nothing.
    RevertPreview {
        repo: String,
        sha: String,
    },
    Revert {
        repo: String,
        sha: String,
    },
    History {
        repo: String,
        limit: u32,
    },
    Contents {
        repo: String,
    },
    // sync loop switches
    Pause,
    Resume,
    SetInterval {
        secs: u64,
    },
    /// The window opened: look for invitations now.
    WindowOpened,
    /// Settings: desktop notifications on or off.
    DesktopNotifications {
        on: bool,
    },
    /// Settings, Advanced: the Claude helper on or off.
    Helper {
        on: bool,
    },
    /// Settings: Test. Adds a notice; the shell shows it on the desktop like any new one.
    TestNotification,
    /// Settings: start at login (the shell turns the OS entry on or off).
    StartAtLogin {
        on: bool,
    },
    /// Publish a fresh snapshot (the shell got `--refresh`: another program changed the state).
    Refresh,
}

/// A feedback report as the window collects it; the logs are redacted and cut here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct FeedbackInput {
    /// `bug` or `idea`.
    pub kind: String,
    pub title: String,
    pub description: String,
    pub app_version: String,
    pub os: String,
    pub logs: String,
    /// PNG or JPEG bytes as base64, at most 3 MiB once decoded.
    pub screenshot: Option<String>,
}

/// What the person picked in the copy preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CopyRequest {
    /// CLAUDE.md lines to append, as the person left them; redacted again.
    pub claude_md: String,
    pub skills: Vec<String>,
    /// Agent file names (`reviewer.md`).
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Event {
    /// The change went out as proposal #n, not as a commit.
    Proposed(u64),
    Packs(Vec<PackView>),
    Text(String),
    /// Feedback was filed (`number`, `url`) or saved to send later (`queued`; `limited` when the
    /// reason is a 429 and not a lost connection).
    Feedback {
        queued: bool,
        limited: bool,
        number: Option<u64>,
        url: Option<String>,
    },
    Proposals(Vec<ProposalView>),
    CopyPreview(CopyPreviewView),
    Copied {
        files: usize,
        claude_md: bool,
        /// The target is in proposal mode: the proposal number.
        proposed: Option<u64>,
    },
    SharePreview {
        added: Vec<String>,
        /// The refusal text when a line looks like a secret; `None` when the scan is clean.
        secret: Option<String>,
    },
    RevertPlan(RevertPlan),
    History(Vec<HistoryItem>),
    Contents(Vec<ContentItem>),
    Done,
    Message(String),
    DeviceCode {
        user_code: String,
        verification_uri: String,
        expires_in: u64,
    },
    SignedIn {
        login: String,
    },
    SignedOut,
    Created {
        repo: String,
        lock: String,
    },
    Joined {
        repo: String,
        warning: String,
    },
    Discovered(Vec<DiscoverView>),
    /// A pass ran: files written or removed, groups that failed.
    Review(Vec<ReviewLine>),
    Edits(Vec<EditView>),
    Conflicts(Vec<ConflictView>),
    /// A file the window opens (a job's log or report); `None`: there is none yet.
    Path(Option<String>),
    Clashes(Vec<ClashView>),
    Synced {
        files: usize,
        failed: usize,
        /// Groups whose files changed.
        repos: Vec<String>,
    },
}
