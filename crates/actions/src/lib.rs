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
//! Modules: groups, sync, edits, review, jobs, personal, proposals, packs, plugins, feedback,
//! extras (ssh, activity, helper, update state), history, copy, sign-in, the schedule, the
//! v1 hand-over and the snapshot (docs/architecture.md §24).
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
pub mod handover;
mod history;
pub mod jobs;
mod launch;
pub mod model;
pub mod msg;
mod packs;
mod personal;
mod plugins;
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
