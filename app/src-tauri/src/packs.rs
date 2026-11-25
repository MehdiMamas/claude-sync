//! Opt-in packs (docs/architecture.md §23) in the app: the commands behind the Optional packs
//! card on the group screen, the checks that follow a sync or the start of the app, and the
//! host the command line (`claude-sync packs`) shares with them. The rules are in
//! `claude_sync_core::packs`.
use crate::ctx::Ctx;
use crate::guard::{self, Error};
use crate::jobs::{blocking, with_jobs};
use claude_sync_core::packs::cli::Host;
use claude_sync_core::packs::launch::{self, Outcome};
use claude_sync_core::packs::ops::{self, CheckReport, Env, PacksOverview};
use claude_sync_core::state::AppState;
use claude_sync_core::{claude_cli, GroupId, Paths};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

type Res<T> = Result<T, Error>;
type CtxState<'a> = State<'a, Arc<Ctx>>;

fn home() -> Result<PathBuf, Error> {
    std::env::home_dir().ok_or_else(|| "cannot find the home directory".to_string())
}

/// A current-thread runtime for the one async call a blocking thread needs.
fn runtime() -> Result<tokio::runtime::Runtime, Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

/// What the app does to put a pack change into effect: apply the group again from its last
/// synced copy (no network), then accept the pack's jobs and reconcile the scheduler.
pub struct AppHost {
    pub ctx: Arc<Ctx>,
}

impl Host for AppHost {
    fn claude(&self) -> Option<PathBuf> {
        claude_cli::resolve(&std::env::home_dir()?)
    }

    fn apply(&self, group: &GroupId, jobs: &[(String, String)]) -> claude_sync_core::Result<()> {
        let invalid = |m: String| claude_sync_core::Error::Invalid(m);
        let engine = self.ctx.engine().map_err(invalid)?;
        let rt = runtime().map_err(invalid)?;
        rt.block_on(engine.reapply_group(group))?;
        with_jobs(&self.ctx.paths, "pack jobs", |j| {
            for (id, hash) in jobs {
                j.accept(group, id, hash)?;
            }
            j.reconcile(false)
        })
        .map(|_| ())
        .map_err(invalid)
    }

    fn group_name(&self, group: &GroupId) -> String {
        let name = self.ctx.engine().ok().and_then(|e| {
            let rt = runtime().ok()?;
            let groups = rt.block_on(e.groups()).ok()?;
            groups.into_iter().find(|g| &g.id == group).map(|g| g.name)
        });
        name.unwrap_or_else(|| group.to_string())
    }
}

/// `claude-sync packs ...` (the app binary's command line, handled before Tauri starts).
pub fn cli_main(paths: &Paths, args: &[String]) -> i32 {
    let Some(home) = std::env::home_dir() else {
        eprintln!("claude-sync: cannot find the home directory");
        return 1;
    };
    let host = AppHost {
        ctx: Arc::new(Ctx::load(paths.clone())),
    };
    claude_sync_core::packs::cli::run(
        paths,
        &host,
        &Env::real(&home),
        args,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

fn overview(paths: &Paths, group: &GroupId) -> Res<PacksOverview> {
    let home = home()?;
    guard::call("list packs", || ops::list(paths, group, &Env::real(&home)))
}

/// The packs of a group, their states and what they need.
#[tauri::command]
pub async fn list_packs(ctx: CtxState<'_>, group: GroupId) -> Res<PacksOverview> {
    let paths = ctx.paths.clone();
    blocking(move || overview(&paths, &group)).await
}

/// What `start_pack_setup` tells the screen.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupLaunch {
    pub started: bool,
    /// `Claude Code isn't installed or signed in`, or `No terminal found`.
    pub problem: Option<String>,
    /// What to run by hand when no terminal could be opened.
    pub command: Option<String>,
}

/// "Set up with Claude": open a terminal that runs Claude Code in the pack's folder.
#[tauri::command]
pub async fn start_pack_setup(ctx: CtxState<'_>, group: GroupId, id: String) -> Res<SetupLaunch> {
    let ctx = ctx.inner().clone();
    blocking(move || {
        let home = home()?;
        let env = Env::real(&home);
        let plan = guard::call("start pack setup", || {
            ops::start_setup(&ctx.paths, &group, &id, &env)
        })?;
        let host = AppHost { ctx: ctx.clone() };
        let name = host.group_name(&group);
        let claude = host.claude();
        let outcome = guard::call("open the setup terminal", || {
            launch::start(&ctx.paths, &group, &plan, &name, env.os, claude.as_deref())
        })?;
        Ok(match outcome {
            Outcome::Started => SetupLaunch {
                started: true,
                problem: None,
                command: None,
            },
            Outcome::NoClaude => SetupLaunch {
                started: false,
                problem: Some("Claude Code isn't installed or signed in".into()),
                command: None,
            },
            Outcome::NoTerminal { command } => SetupLaunch {
                started: false,
                problem: Some("No terminal found".into()),
                command: Some(command),
            },
        })
    })
    .await
}

/// Run the pack's check (after the setup terminal closed, "Check again"). Takes up to the
/// check's timeout.
#[tauri::command]
pub async fn check_pack(ctx: CtxState<'_>, group: GroupId, id: String) -> Res<CheckReport> {
    let paths = ctx.paths.clone();
    blocking(move || {
        let home = home()?;
        guard::call("check pack", || {
            ops::check_pack(&paths, &group, &id, &Env::real(&home))
        })
    })
    .await
}

/// Turn a ready pack on for this computer. `hashes`: the consent hash of each of the pack's
/// jobs as the screen showed it.
#[tauri::command]
pub async fn turn_on_pack(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
    id: String,
    hashes: BTreeMap<String, String>,
) -> Res<PacksOverview> {
    let ctx = ctx.inner().clone();
    blocking(move || {
        let home = home()?;
        let env = Env::real(&home);
        let on = guard::call("turn on pack", || {
            ops::turn_on(&ctx.paths, &group, &id, &hashes, &env)
        })?;
        let host = AppHost { ctx: ctx.clone() };
        guard::call("apply pack", || host.apply(&group, &on.jobs))?;
        if on.plugins_changed {
            let _ = app.emit("plugins-changed", ());
        }
        crate::background::refresh_status(&app);
        overview(&ctx.paths, &group)
    })
    .await
}

/// Turn a pack off for this computer.
#[tauri::command]
pub async fn turn_off_pack(
    app: AppHandle,
    ctx: CtxState<'_>,
    group: GroupId,
    id: String,
) -> Res<PacksOverview> {
    let ctx = ctx.inner().clone();
    blocking(move || {
        let home = home()?;
        let env = Env::real(&home);
        let changed = guard::call("turn off pack", || {
            ops::turn_off(&ctx.paths, &group, &id, &env)
        })?;
        let host = AppHost { ctx: ctx.clone() };
        guard::call("apply pack", || host.apply(&group, &[]))?;
        if changed {
            let _ = app.emit("plugins-changed", ());
        }
        crate::background::refresh_status(&app);
        overview(&ctx.paths, &group)
    })
    .await
}

/// Checks that run by themselves, and only these (D8): packs that are on and changed upstream,
/// and (`tried`, once per app start) packs a member set up before that never passed. Packs
/// that are not set up, off or ready never run anything. Tells the screens when one ran.
pub fn recheck_soon(app: &AppHandle, tried: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = app.state::<Arc<Ctx>>().inner().clone();
        let (Some(home), Ok(state)) = (std::env::home_dir(), AppState::load(&ctx.paths)) else {
            return;
        };
        let mut ran = false;
        for g in &state.groups {
            match guard::call("recheck packs", || {
                ops::recheck_due(&ctx.paths, g, &Env::real(&home), tried)
            }) {
                Ok(done) => ran |= !done.is_empty(),
                Err(e) => tracing::warn!("recheck packs of {g}: {e}"),
            }
        }
        if ran {
            let _ = app.emit("packs-changed", ());
        }
    });
}
