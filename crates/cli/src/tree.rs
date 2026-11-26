//! The command tree: one clap definition compiled into `claude-sync-cli` and, in step 7, into
//! the app binary (ADR-010). Wrong usage is clap's exit code 2.
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "claude-sync-cli",
    about = "claude-sync without a desktop: sign in, join a group, sync, run as a service",
    disable_version_flag = true,
    disable_help_subcommand = true,
    arg_required_else_help = true,
    // A person who types a command runs it; scripts get exit codes 0, 1 and 2.
    after_help = "Exit codes: 0 done, 1 failed, 2 wrong usage. Add --json for machine output."
)]
pub struct Cli {
    /// Machine output on stdout.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Cmd {
    /// Sign in with GitHub (prints a code to enter in any browser)
    Login,
    /// Remove the sign-in from this machine
    Logout,
    /// Groups joined here and invitations waiting
    Groups,
    /// Accept an invitation if there is one, and join
    Join {
        /// owner/repo
        repo: String,
    },
    /// Leave a group
    Leave {
        /// owner/repo
        repo: String,
    },
    /// One full sync now
    Sync,
    /// Sign-in, groups, personal memory
    Status,
    /// Your own settings, CLAUDE.md and memory through a private repo
    Personal {
        #[command(subcommand)]
        cmd: OnOff,
    },
    /// Reach your own machines over SSH (ssh <alias>)
    Ssh {
        #[command(subcommand)]
        cmd: SshCmd,
    },
    /// Share whether you are at the computer (off until you turn it on)
    Activity {
        #[command(subcommand)]
        cmd: OnOff,
    },
    /// Optional packs of a group: set one up with Claude, then turn it on here
    Packs {
        #[command(subcommand)]
        cmd: Option<PacksCmd>,
        /// The group, when you are in several that have packs
        #[arg(long, global = true, value_name = "OWNER/REPO")]
        group: Option<String>,
    },
    /// Changes waiting for you (CLAUDE.md, local edits, jobs)
    Review {
        #[command(subcommand)]
        cmd: ReviewCmd,
    },
    /// Sync in a loop (run it with `service install`)
    Daemon,
    /// Run the daemon in the background at start
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
    /// Check for the newest release
    Update {
        /// Only look; the install path comes with the updater
        #[arg(long)]
        check: bool,
    },
    /// Print the version
    Version,
    /// Started by the OS scheduler, not typed
    #[command(hide = true)]
    RunJob {
        /// `<owner/repo> <id>` (the form v1 tasks use) or just `<id>`
        #[arg(num_args = 1..=2, required = true)]
        args: Vec<String>,
        /// Run now, even when the job is off
        #[arg(long)]
        now: bool,
    },
    /// The app only: remove every OS task this device registered
    #[command(hide = true)]
    RemoveJobs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum OnOff {
    Status,
    Enable,
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum SshCmd {
    Status,
    Enable,
    Disable,
    /// The name your other machines use for this one
    Alias {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum PacksCmd {
    List,
    Setup { id: String },
    Check { id: String },
    On { id: String },
    Off { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ReviewCmd {
    /// What waits for you, with one id per item
    List,
    /// Accept one (ids come from `review list`)
    Accept { id: String },
    /// Decline or undo one
    Undo { id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum ServiceCmd {
    Install,
    Remove,
    Status,
}

/// The visible command names, in order (the parity script compares them with v1).
pub fn command_names() -> Vec<String> {
    use clap::CommandFactory;
    Cli::command()
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| c.get_name().to_string())
        .collect()
}
