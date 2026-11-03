//! The commands. Each returns the text for a person and the value for `--json`.
use crate::args::{Cmd, Personal, Review, Service};
use crate::auth::{self, Hosts, Standing};
use crate::daemon;
use crate::jobs::{Hooks, RealHooks};
use crate::{adopt, config, review, service, update};
use claude_sync_core::report::StatusReport;
use claude_sync_core::state::AppState;
use claude_sync_core::{GroupId, Paths};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

pub struct Out {
    pub text: String,
    pub json: Value,
}

impl Out {
    fn new(text: impl Into<String>, json: Value) -> Self {
        Self {
            text: text.into(),
            json,
        }
    }
    fn done(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            json: json!({ "ok": true, "message": text }),
            text,
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub async fn execute(paths: &Paths, cmd: Cmd) -> Result<Out, String> {
    match cmd {
        Cmd::Login => login(paths).await,
        Cmd::Logout => {
            auth::logout(paths)?;
            Ok(Out::done(
                "Signed out. Your groups on this machine are kept.",
            ))
        }
        Cmd::Groups => groups(paths).await,
        Cmd::Join(g) => join(paths, &g).await,
        Cmd::Leave(g) => {
            let (_, engine) = auth::require(paths)?;
            engine.leave(&g).await.map_err(|e| e.to_string())?;
            Ok(Out::done(format!(
                "Left {g}. What it put in {} is removed.",
                paths.claude_home.display()
            )))
        }
        Cmd::Sync => sync(paths).await,
        Cmd::Status => status(paths).await,
        Cmd::Personal(p) => personal(paths, p).await,
        Cmd::Review(r) => review_cmd(paths, r).await,
        Cmd::Service(s) => {
            let text = match s {
                Service::Install => service::install()?,
                Service::Remove => service::remove()?,
                Service::Status => service::status()?,
            };
            Ok(Out::done(text))
        }
        Cmd::Update { check } => update_cmd(paths, check).await,
        Cmd::Version => Ok(Out::new(
            format!("claude-sync-cli {}", config::VERSION),
            json!({ "version": config::VERSION }),
        )),
        // Handled before the runtime starts: see main.
        Cmd::Daemon | Cmd::RunJob(_) => Err("this command is not run through here".into()),
    }
}

async fn login(paths: &Paths) -> Result<Out, String> {
    let state = AppState::load(paths).map_err(|e| e.to_string())?;
    let client_id = config::client_id(state.github_app);
    let login = auth::login(
        paths,
        &Hosts::github(),
        client_id,
        |c| {
            eprintln!(
                "Open {} in any browser and enter the code {}.\nWaiting for you to approve (the code expires in {} minutes)...",
                c.verification_uri,
                c.user_code,
                c.expires_in / 60
            );
        },
        async |d| tokio::time::sleep(d).await,
    )
    .await?;
    Ok(Out::new(
        format!(
            "Signed in as {login}. Next: `claude-sync-cli join owner/repo`, then `claude-sync-cli service install`."
        ),
        json!({ "ok": true, "login": login }),
    ))
}

async fn groups(paths: &Paths) -> Result<Out, String> {
    let (_, engine) = auth::require(paths)?;
    let joined = engine.groups().await.map_err(|e| e.to_string())?;
    let invites = engine.invitations().await.map_err(|e| e.to_string())?;
    let mut text = String::new();
    if joined.is_empty() {
        text.push_str("Joined: none\n");
    } else {
        text.push_str("Joined:\n");
    }
    for g in &joined {
        text.push_str(&format!(
            "  {} ({}), {} members\n",
            g.id,
            if g.disabled { "off here" } else { "on" },
            g.members.len()
        ));
    }
    if invites.is_empty() {
        text.push_str("Invitations: none");
    } else {
        text.push_str("Invitations (join with `claude-sync-cli join owner/repo`):\n");
        for i in &invites {
            text.push_str(&format!("  {} from {}\n", i.repo, i.inviter));
        }
    }
    Ok(Out::new(
        text.trim_end(),
        json!({ "joined": joined, "invitations": invites }),
    ))
}

async fn join(paths: &Paths, group: &GroupId) -> Result<Out, String> {
    let (_, engine) = auth::require(paths)?;
    // The file is saved first: the join, then the removal of pasted text, change it.
    let backup = adopt::backup(paths)?;
    let invite = engine
        .invitations()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|i| i.repo == *group);
    let joined = match invite {
        Some(i) => engine.accept_invitation(i.id).await,
        None => engine.join(group).await,
    };
    let status = match joined {
        Ok(s) => s,
        Err(e) => {
            if let Some(dir) = &backup {
                let _ = std::fs::remove_dir_all(dir);
            }
            return Err(e.to_string());
        }
    };
    let removed = adopt::remove_inlined(paths, group)?;
    let mut text = format!("Joined {group}.");
    if removed > 0 {
        text.push_str(&format!(
            "\nTook {removed} lines of the group's text out of {}/CLAUDE.md; they come back inside the managed block.",
            paths.claude_home.display()
        ));
    }
    match (&backup, removed) {
        (Some(dir), r) if r > 0 => {
            text.push_str(&format!("\nYour CLAUDE.md before this: {}", dir.display()))
        }
        (Some(dir), _) => {
            // Nothing was changed, so the copy only adds clutter.
            let _ = std::fs::remove_dir_all(dir);
        }
        _ => {}
    }
    if status.plugins_changed {
        text.push_str("\nPlugins changed: restart Claude Code sessions.");
    }
    Ok(Out::new(
        text,
        json!({ "ok": true, "group": group, "removedLines": removed, "backup": if removed > 0 { backup } else { None } }),
    ))
}

async fn sync(paths: &Paths) -> Result<Out, String> {
    let (_, engine) = auth::require(paths)?;
    let hooks: Arc<dyn Hooks> = Arc::new(RealHooks);
    let mut checked = false;
    let o = daemon::tick(
        &engine,
        paths,
        true,
        Duration::from_secs(60),
        &mut checked,
        &hooks,
    )
    .await;
    if let Some(s) = o.skipped {
        return Err(s);
    }
    if let Some(e) = o.error {
        return Err(e);
    }
    let report = o.report.ok_or("the sync produced no report")?;
    let mut text = String::new();
    let mut failed = 0;
    for g in &report.groups {
        let state = match &g.error {
            Some(e) => {
                failed += 1;
                format!("failed: {e}")
            }
            None => format!(
                "ok{}{}",
                if g.pulled { ", pulled" } else { "" },
                if g.pushed { ", pushed" } else { "" }
            ),
        };
        text.push_str(&format!("{}: {state}\n", g.id));
        if !g.review.is_empty() {
            text.push_str(&format!(
                "  {} CLAUDE.md changes wait for you: `claude-sync-cli review list`\n",
                g.review.len()
            ));
        }
        if !g.conflicts.is_empty() {
            text.push_str(&format!("  {} conflicts\n", g.conflicts.len()));
        }
    }
    if report.groups.is_empty() {
        text.push_str("No groups joined: `claude-sync-cli join owner/repo`\n");
    }
    let json = json!({ "ok": failed == 0, "report": report, "personalOffer": o.personal_offer });
    if failed > 0 {
        return Err(format!("{}{failed} group(s) failed", text));
    }
    Ok(Out::new(text.trim_end(), json))
}

async fn status(paths: &Paths) -> Result<Out, String> {
    let (login, expired, engine) = match auth::standing(paths)? {
        Standing::In(s) => {
            let e = auth::engine(paths, &s)?;
            (Some(s.login), None, Some(e))
        }
        Standing::Expired(l) => (None, Some(l), None),
        Standing::Out => (None, None, None),
    };
    let report = StatusReport::gather(paths, login, expired, engine.as_ref()).await;
    let mut text = report.text(now_unix());
    if report.expired_login.is_some() {
        text = text.replace(
            "open the app to sign in again",
            "run `claude-sync-cli login`",
        );
    }
    Ok(Out::new(text, report.json()))
}

async fn personal(paths: &Paths, p: Personal) -> Result<Out, String> {
    let (_, engine) = auth::require(paths)?;
    let st = match p {
        Personal::Status => engine.personal_status(),
        Personal::Enable => engine.personal_enable().await.map_err(|e| e.to_string())?,
        Personal::Disable => engine.personal_disable().await.map_err(|e| e.to_string())?,
    };
    Ok(Out::new(
        claude_sync_core::report::personal_text(&st, now_unix()),
        serde_json::to_value(&st).map_err(|e| e.to_string())?,
    ))
}

async fn review_cmd(paths: &Paths, r: Review) -> Result<Out, String> {
    let (_, engine) = auth::require(paths)?;
    match r {
        Review::List => {
            let entries = review::list(&engine, paths).await?;
            let text = if entries.is_empty() {
                "Nothing is waiting for you.".to_string()
            } else {
                entries
                    .iter()
                    .map(|e| {
                        format!(
                            "{}\n  {}\n  accept: {}; undo: {}",
                            e.id, e.summary, e.accept, e.undo
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(Out::new(text, json!({ "items": entries })))
        }
        Review::Accept(id) => {
            let text = review::decide(&engine, paths, &review::Id::parse(&id)?, true).await?;
            Ok(Out::done(text))
        }
        Review::Undo(id) => {
            let text = review::decide(&engine, paths, &review::Id::parse(&id)?, false).await?;
            Ok(Out::done(text))
        }
    }
}

async fn update_cmd(paths: &Paths, check: bool) -> Result<Out, String> {
    let Some(version) = update::check(paths).await? else {
        return Ok(Out::new(
            format!("claude-sync-cli {} is the newest version.", config::VERSION),
            json!({ "current": config::VERSION, "available": null }),
        ));
    };
    if check {
        return Ok(Out::new(
            format!(
                "Version {version} is available (this is {}). Run `claude-sync-cli update`.",
                config::VERSION
            ),
            json!({ "current": config::VERSION, "available": version }),
        ));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    update::install(&version, &exe).await?;
    Ok(Out::new(
        format!("Updated to {version}. A running daemon switches over at its next restart (`systemctl --user restart claude-sync`)."),
        json!({ "ok": true, "from": config::VERSION, "to": version }),
    ))
}
