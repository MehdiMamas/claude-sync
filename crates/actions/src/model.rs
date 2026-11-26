//! Commands in, events out, snapshots for the window and tray (v2-Class: `handle(Command):
//! Result<Event>`). Snapshot types derive `ts_rs::TS` behind the `ts` feature.
use serde::{Deserialize, Serialize};

macro_rules! ts {
    ($($i:item)*) => {$(
        #[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
        /// `on`, `unavailable` or unknown (not tried yet).
        pub host_lock: Option<String>,
        pub review_choice: crate::choices::ReviewChoice,
        pub pending_review: u32,
        pub off: bool,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct InvitationView {
        pub id: u64,
        pub repo: String,
        pub inviter: String,
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
        pub discoverable: Vec<String>,
        pub personal_on: bool,
        pub unread: u32,
        pub notifications: Vec<NotificationView>,
        pub jobs: Vec<JobView>,
        pub ssh_on: bool,
        pub activity_on: bool,
        pub update: String,
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
    pub struct ReviewLine {
        pub id: String,
        pub add: bool,
        pub lines: Vec<String>,
        pub author: String,
    }
}

/// One user action.
#[derive(Debug, Clone)]
pub enum Command {
    // account
    SignInStart {
        app: bool,
    },
    SignInCancel,
    /// Sent by the sign-in waiter itself.
    SignInDone(crate::signin::Outcome),
    SignOut,
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
    Clashes,
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
    // sync loop switches
    Pause,
    Resume,
    SetInterval {
        secs: u64,
    },
    /// The window opened: look for invitations now.
    WindowOpened,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
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
    Discovered(Vec<String>),
    /// A pass ran: files written or removed, groups that failed.
    Review(Vec<ReviewLine>),
    Edits(Vec<EditView>),
    Conflicts(Vec<ConflictView>),
    Clashes(Vec<claude_sync_engine::clashes::Clash>),
    Synced {
        files: usize,
        failed: usize,
    },
}
