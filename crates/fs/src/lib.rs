//! The machine side of claude-sync 2: the real filesystem behind the engine's `Disk`, the
//! executor with its journal, applying to `~/.claude`, the file watcher, the OS task writers
//! and the sync pass. Everything takes an injectable home folder.
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo)
)]

pub mod apply;
pub mod atomic;
pub mod disk;
pub mod exec;
pub mod pass;
pub mod redact;
pub mod ssh;
pub mod tasks;
pub mod watch;

pub use disk::{FsDisk, Home, Scope};
pub use exec::{Applied, Error, Exec, Result};

/// The text v1 shows when settings.json cannot be merged into.
pub const NOT_OBJECT_LOCAL: &str =
    "settings.json is not one JSON object, so nothing was merged; settings.json was not changed";

/// No console window flashes for a child process on Windows (CREATE_NO_WINDOW).
pub fn quiet(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}
