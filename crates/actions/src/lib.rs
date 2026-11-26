//! Every user action of claude-sync 2 and the actor that runs them, so the CLI and the app
//! share one implementation (ADR-010). This crate is added to the v2-Plan file list for that
//! reason: the Plan names `crates/cli` and `app/src-tauri` as separate shells over the engine,
//! and both need the same commands, snapshot and background loop.
//!
//! One tokio task owns the engine, store, GitHub and filesystem handles (v2-EngRules: only the
//! actor mutates state; the window and tray read [`Snapshot`]s). [`Handle::send`] runs one
//! [`Command`] and returns its [`Event`]; the background loop (tick, invitations, renames,
//! rate-limit waits, session expiry) runs in the same task.
//!
//! Built so far: jobs (accept, on/off, auto-accept, run now, OS tasks, run-job, remove-jobs), sign-in (device flow with cancel, App mode, sign out, 401 stop), the
//! background schedule, the snapshot, and groups with members (create, discover, join, accept,
//! add, remove, transfer, leave, reorder, off/on). Sync, local edits, review, jobs, personal
//! memory, proposals, notifications, feedback, packs, ssh and activity come in later blocks.
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo)
)]

mod actor;
mod choices;
mod copy;
mod core;
mod edits;
mod error;
mod extras;
mod feedback;
mod groups;
mod history;
pub mod jobs;
pub mod model;
pub mod msg;
mod packs;
mod personal;
mod proposals;
mod review;
mod schedule;
mod signin;
mod sync;

pub use actor::{spawn, spawn_with, Handle};
pub use choices::{Choices, ReviewChoice};
pub use core::{Core, Env, Secrets};
pub use error::{Error, Result};
pub use model::*;
pub use schedule::{Due, Schedule};
