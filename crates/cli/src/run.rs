//! The commands: each opens the core on the token file, sends the actions' commands and turns
//! the answer into a sentence (or the value for `--json`).
use crate::tree::*;
use crate::{adopt, daemon, service, Ctx, Fail};
use claude_sync_actions::{Command, Core, Env, Event, Snapshot};
use claude_sync_engine::traits::Store;
use claude_sync_github::Config;
use claude_sync_store::Db;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub struct Out {
    pub text: String,
    pub json: Value,
}

impl Out {
    fn new(text: impl Into<String>, json: Value) -> Self {
        Out {
            text: text.into(),
            json,
        }
    }
    fn done(text: impl Into<String>) -> Self {
        let text = text.into();
        Out {
            json: json!({ "ok": true, "message": text }),
            text,
        }
    }
}

type Res = Result<Out, Fail>;

/// Public by design (device flow), so `login` works in any build; the build variable wins.
const FALLBACK_CLIENT_ID: &str = "Ov23liM4fyK64GPQbGkC";
const APP_CLIENT_ID: &str = match option_env!("CLAUDE_SYNC_GITHUB_APP_CLIENT_ID") {
    Some(id) => id,
    None => "Iv23litHw8qKqwlLcKNV",
};

struct Where {
    home: PathBuf,
}

impl Where {
    // `CLAUDE_SYNC_HOME` and `CLAUDE_HOME` move these, as in v1.
    fn state(&self) -> PathBuf {
        claude_sync_fs::Home::from_env(&self.home).state
    }
    fn claude(&self) -> PathBuf {
        claude_sync_fs::Home::from_env(&self.home).claude
    }
}

fn place(ctx: &Ctx) -> Result<Where, Fail> {
    let home = ctx
        .home
        .clone()
        .or_else(std::env::home_dir)
        .ok_or("cannot find the home directory")?;
    Ok(Where { home })
}

/// A dev token wins over everything and is held in memory; else the app's keychain; else the file.
fn secrets_for(ctx: &Ctx, state: &std::path::Path, dev: bool) -> claude_sync_actions::Secrets {
    if dev {
        return claude_sync_actions::Secrets(Arc::new(crate::token::MemSecrets::default()));
    }
    ctx.secrets
        .clone()
        .unwrap_or_else(|| daemon::file_secrets(state))
}

pub(crate) fn env_in(ctx: &Ctx) -> Result<Env, Fail> {
    env_in_dev(ctx, crate::token::dev_token().is_some())
}

pub(crate) fn env_in_dev(ctx: &Ctx, dev: bool) -> Result<Env, Fail> {
    let w = place(ctx)?;
    let mut cfg: Config = ctx.cfg.clone().unwrap_or_else(Config::github);
    if cfg.client_id.is_empty() {
        cfg.client_id = FALLBACK_CLIENT_ID.into();
    }
    Ok(Env {
        cfg,
        app_client_id: APP_CLIENT_ID.into(),
        secrets: secrets_for(ctx, &w.state(), dev),
        now: Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64)
        }),
        device: sysinfo::System::host_name().unwrap_or_else(|_| "cli".into()),
        home: w.home,
        // a test's injected home never touches the real process list
        procs: if ctx.home.is_some() {
            Arc::new(claude_sync_actions::handover::NoProcs)
        } else {
            claude_sync_actions::handover::system()
        },
    })
}

/// The core's environment: the sign-in in `ctx.secrets` (the app's keychain), else the token file.
pub fn env(ctx: &Ctx) -> Result<Env, String> {
    env_in(ctx).map_err(|f| match f {
        Fail::Usage(m) | Fail::Run(m) => m,
    })
}

/// The word for a v1 daemon that did not stop: its pid and the command that ends it here.
pub fn v1_daemon_message(pid: u32) -> String {
    let (who, cmd) = if pid == 0 {
        ("A v1 claude-sync-cli daemon".to_string(), None)
    } else {
        (
            format!("The v1 claude-sync-cli daemon (process {pid})"),
            Some(pid),
        )
    };
    let how = match (cmd, cfg!(windows)) {
        (Some(p), true) => format!("taskkill /PID {p} /F"),
        (Some(p), false) => {
            format!("kill {p}   (with systemd: systemctl --user restart claude-sync)")
        }
        (None, true) => "taskkill /IM claude-sync-cli.exe /F".to_string(),
        (None, false) => "systemctl --user restart claude-sync".to_string(),
    };
    format!("{who} is still running and did not stop in 30 s, so nothing was imported. Stop it with: {how}")
}

fn open_plain(ctx: &Ctx) -> Result<Core, Fail> {
    let core = Core::open(env_in(ctx)?)?;
    match core.snapshot().install {
        Some(claude_sync_actions::model::InstallNote::Daemon { pid, .. }) => {
            Err(Fail::Run(v1_daemon_message(pid)))
        }
        _ => Ok(core),
    }
}

/// Opens the core; with an expired sign-in (not after logout) tries `gh auth token` first (as v1).
/// A test's injected home never asks the real `gh`.
fn open(ctx: &Ctx) -> Result<Core, Fail> {
    let mut core = open_plain(ctx)?;
    crate::token::dev_sign_in(&mut core, crate::token::dev_token());
    if ctx.home.is_none() {
        crate::token::recover(&mut core, crate::token::gh_token);
    }
    Ok(core)
}

fn need_sign_in(core: &Core) -> Result<(), Fail> {
    if core.snapshot().login.is_none() {
        return Err("not signed in: run `claude-sync-cli login`".into());
    }
    Ok(())
}

fn repo_arg(s: &str) -> Result<String, Fail> {
    match s.split_once('/') {
        Some((o, n)) if !o.is_empty() && !n.is_empty() && !n.contains('/') => Ok(s.into()),
        _ => Err(Fail::Usage(format!(
            "`{s}` is not a group: write owner/repo"
        ))),
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

pub(crate) fn execute(ctx: &Ctx, cmd: Cmd) -> Res {
    match cmd {
        Cmd::Login => login(ctx),
        Cmd::Logout => {
            let mut core = open_plain(ctx)?;
            core.handle(Command::SignOut)?;
            Ok(Out::done(
                "Signed out. Your groups on this machine are kept.",
            ))
        }
        Cmd::Groups => groups(ctx),
        Cmd::Join { repo } => join(ctx, &repo_arg(&repo)?),
        Cmd::Leave { repo } => {
            let repo = repo_arg(&repo)?;
            let mut core = open(ctx)?;
            need_sign_in(&core)?;
            core.handle(Command::Leave { repo: repo.clone() })?;
            Ok(Out::done(format!("Left {repo}.")))
        }
        Cmd::Sync => sync(ctx),
        Cmd::Status => status(ctx),
        Cmd::Personal { cmd } => personal(ctx, cmd),
        Cmd::Ssh { cmd } => ssh(ctx, cmd),
        Cmd::Activity { cmd } => activity(ctx, cmd),
        Cmd::Packs { cmd, group } => packs(ctx, cmd.unwrap_or(PacksCmd::List), group),
        Cmd::Review { cmd } => review(ctx, cmd),
        Cmd::Daemon => {
            let w = place(ctx)?;
            let line = daemon::run(&w.state(), ctx.version, env_in(ctx)?)?;
            Ok(Out::done(line))
        }
        Cmd::Service { cmd } => Ok(Out::done(match cmd {
            ServiceCmd::Install => service::install()?,
            ServiceCmd::Remove => service::remove()?,
            ServiceCmd::Status => service::status()?,
        })),
        Cmd::Update { check } => update(ctx, check),
        Cmd::Version => Ok(Out::new(
            format!("{} {}", ctx.name, ctx.version),
            json!({ "version": ctx.version }),
        )),
        Cmd::RunJob { args, now } => run_job(ctx, &args, now),
        Cmd::RemoveJobs => {
            let w = place(ctx)?;
            let n = claude_sync_actions::jobs::remove_jobs(
                &w.home,
                &claude_sync_actions::jobs::JobsEnv::system(&w.home),
            )?;
            Ok(Out::new(
                format!("Removed {n} scheduled jobs."),
                json!({ "ok": true, "removed": n }),
            ))
        }
    }
}

fn login(ctx: &Ctx) -> Res {
    use claude_sync_actions::SignInState as S;
    let env = env_in(ctx)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let login: Result<String, Fail> = rt.block_on(async {
        let h = claude_sync_actions::spawn(env, Duration::from_secs(3600)).await?;
        let app = h.snapshot().app_mode;
        if let Event::DeviceCode {
            user_code,
            verification_uri,
            expires_in,
        } = h.send(Command::SignInStart { app }).await?
        {
            eprintln!(
                "Open {verification_uri} in any browser and enter the code {user_code}.\nWaiting for you to approve (the code expires in {} minutes)...",
                expires_in / 60
            );
        }
        let mut rx = h.watch();
        let mut seen_code = false;
        loop {
            let s = rx.borrow().clone();
            match s.sign_in {
                S::Code { .. } | S::Starting => seen_code = true,
                S::SignedOut if seen_code => {
                    if let Some(l) = s.login {
                        return Ok(l);
                    }
                }
                S::Expired => return Err("the code expired; run `login` again".into()),
                S::Denied => return Err("sign-in was denied on GitHub".into()),
                S::Offline => return Err("cannot reach GitHub; check the connection".into()),
                S::Unavailable => return Err("sign-in is not available in this build".into()),
                S::Failed { message } => return Err(message.into()),
                S::SignedOut => {}
            }
            rx.changed()
                .await
                .map_err(|_| Fail::from("sign-in stopped"))?;
        }
    });
    let login = login?;
    Ok(Out::new(
        format!(
            "Signed in as {login}. Next: `claude-sync-cli join owner/repo`, then `claude-sync-cli service install`."
        ),
        json!({ "ok": true, "login": login }),
    ))
}

fn groups(ctx: &Ctx) -> Res {
    let mut core = open(ctx)?;
    need_sign_in(&core)?;
    core.load_invitations();
    let s = core.snapshot();
    let mut text = String::from(if s.groups.is_empty() {
        "Joined: none\n"
    } else {
        "Joined:\n"
    });
    for g in &s.groups {
        text.push_str(&format!(
            "  {} ({}), {} members\n",
            g.repo,
            if g.off { "off here" } else { "on" },
            g.members.len()
        ));
    }
    if s.invitations.is_empty() {
        text.push_str("Invitations: none");
    } else {
        text.push_str("Invitations (join with `claude-sync-cli join owner/repo`):\n");
        for i in &s.invitations {
            text.push_str(&format!("  {} from {}\n", i.repo, i.inviter));
        }
    }
    Ok(Out::new(
        text.trim_end(),
        json!({ "joined": s.groups, "invitations": s.invitations }),
    ))
}

/// The group's CLAUDE.md as the review keeps it (the store's `review/<repo>`, `group`).
fn group_text(state: &std::path::Path, repo: &str) -> String {
    Db::open(&state.join("claude-sync.db"))
        .ok()
        .and_then(|db| Store::read(&db, &format!("review/{repo}")).ok().flatten())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["group"].as_str().map(String::from))
        .unwrap_or_default()
}

fn join(ctx: &Ctx, repo: &str) -> Res {
    let w = place(ctx)?;
    let mut core = open(ctx)?;
    need_sign_in(&core)?;
    // The file is saved first: the join, then the removal of pasted text, change it.
    let backup = adopt::backup(&w.claude(), &w.state())?;
    core.load_invitations();
    let invite = core
        .snapshot()
        .invitations
        .into_iter()
        .find(|i| i.repo.eq_ignore_ascii_case(repo));
    let joined = match invite {
        Some(i) => core
            .handle(Command::AcceptInvitation { id: i.id })
            .and_then(|_| core.handle(Command::Join { repo: repo.into() })),
        None => core.handle(Command::Join { repo: repo.into() }),
    };
    let warning = match joined {
        Ok(Event::Joined { warning, .. }) => warning,
        Ok(_) => String::new(),
        Err(e) => {
            if let Some(dir) = &backup {
                let _ = std::fs::remove_dir_all(dir);
            }
            return Err(e.into());
        }
    };
    // The first sync brings the group's CLAUDE.md in; its text is what was pasted by hand.
    let first_sync = core.handle(Command::SyncNow);
    let removed = adopt::remove_inlined(&w.claude(), &group_text(&w.state(), repo))?;
    let mut text = format!("Joined {repo}.");
    if let Err(e) = &first_sync {
        text.push_str(&format!("\nThe first sync did not finish: {e}"));
    }
    if removed > 0 {
        text.push_str(&format!(
            "\nTook {removed} lines of the group's text out of {}/CLAUDE.md; they come back inside the managed block.",
            w.claude().display()
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
    if !warning.is_empty() {
        text.push_str(&format!("\n{warning}"));
    }
    Ok(Out::new(
        text,
        json!({ "ok": true, "group": repo, "removedLines": removed, "backup": if removed > 0 { backup } else { None } }),
    ))
}

fn sync(ctx: &Ctx) -> Res {
    let mut core = open(ctx)?;
    need_sign_in(&core)?;
    let Event::Synced { files, failed, .. } = core.handle(Command::SyncNow)? else {
        return Err("the sync produced no report".into());
    };
    let s = core.snapshot();
    let mut text = String::new();
    for g in &s.groups {
        let state = match &g.error {
            Some(e) => format!("failed: {e}"),
            None => "ok".into(),
        };
        text.push_str(&format!("{}: {state}\n", g.repo));
        if g.pending_review > 0 {
            text.push_str(&format!(
                "  {} CLAUDE.md changes wait for you: `claude-sync-cli review list`\n",
                g.pending_review
            ));
        }
    }
    if s.groups.is_empty() {
        text.push_str("No groups joined: `claude-sync-cli join owner/repo`\n");
    }
    text.push_str(&format!("{files} files written or removed."));
    if failed > 0 {
        return Err(Fail::Run(format!("{text}\n{failed} group(s) failed")));
    }
    Ok(Out::new(
        text,
        json!({ "ok": true, "files": files, "failed": failed, "groups": s.groups }),
    ))
}

/// The one document `status --json` prints (Parity 61): exactly these keys.
pub fn status_json(
    requests: Vec<claude_sync_store::RequestRow>,
    s: &Snapshot,
    groups_error: Option<String>,
) -> Value {
    json!({
        "requests": requests.iter().map(|r| json!({
            "caller": r.caller, "endpoint": r.endpoint, "result": r.status, "count": r.n
        })).collect::<Vec<_>>(),
        "login": s.login,
        "expiredLogin": s.session_expired,
        "groups": s.groups,
        "groupsError": groups_error,
        "personal": s.personal,
    })
}

fn ago(unix: i64) -> String {
    let d = (now_unix() - unix).max(0);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86400),
    }
}

fn status(ctx: &Ctx) -> Res {
    let w = place(ctx)?;
    let core = open(ctx)?;
    let s = core.snapshot();
    let requests = Db::open(&w.state().join("claude-sync.db"))
        .and_then(|db| db.requests())
        .unwrap_or_default();
    let mut text = match (&s.login, &s.session_expired) {
        (_, Some(l)) => format!("Sign-in of {l} expired: run `claude-sync-cli login`."),
        (Some(l), None) => format!("Signed in as {l}."),
        _ => "Not signed in: run `claude-sync-cli login`.".to_string(),
    };
    text.push_str(if s.groups.is_empty() {
        "\nGroups: none"
    } else {
        "\nGroups:"
    });
    for g in &s.groups {
        let last = g.last_sync.map_or("never synced".into(), ago);
        text.push_str(&format!(
            "\n  {} ({}): {}",
            g.repo,
            if g.off { "off here" } else { "on" },
            g.error.clone().unwrap_or(last)
        ));
    }
    text.push_str(&format!(
        "\nPersonal memory: {}",
        if s.personal.on { "on" } else { "off" }
    ));
    let json = status_json(requests, &s, None);
    Ok(Out::new(text, json))
}

fn refresh(ctx: &Ctx) {
    if let Some(exe) = &ctx.refresh {
        crate::refresh::refresh_running_app(exe);
    }
}

fn personal(ctx: &Ctx, cmd: OnOff) -> Res {
    let mut core = open(ctx)?;
    need_sign_in(&core)?;
    match cmd {
        OnOff::Status => {}
        OnOff::Enable => {
            need_sign_in(&core)?;
            core.handle(Command::PersonalOn)?;
            refresh(ctx);
        }
        OnOff::Disable => {
            need_sign_in(&core)?;
            core.handle(Command::PersonalOff)?;
            refresh(ctx);
        }
    }
    let p = core.snapshot().personal;
    let mut text = format!("Personal memory is {}.", if p.on { "on" } else { "off" });
    if !p.repo.is_empty() {
        text.push_str(&format!(" Repo: {}.", p.repo));
    }
    if let Some(t) = p.last_sync {
        text.push_str(&format!(" Last sync {}.", ago(t)));
    }
    if let Some(e) = &p.error {
        text.push_str(&format!("\nProblem: {e}"));
    }
    for d in &p.devices {
        text.push_str(&format!(
            "\n  {}{} ({})",
            d.name,
            if d.this { " (this one)" } else { "" },
            d.os
        ));
    }
    Ok(Out::new(
        text,
        serde_json::to_value(&p).map_err(|e| e.to_string())?,
    ))
}

fn ssh(ctx: &Ctx, cmd: SshCmd) -> Res {
    let mut core = open(ctx)?;
    match &cmd {
        SshCmd::Status => {}
        SshCmd::Enable => {
            core.handle(Command::SshSet { on: true })?;
        }
        SshCmd::Disable => {
            core.handle(Command::SshSet { on: false })?;
        }
        SshCmd::Alias { name } => {
            core.handle(Command::SshAlias {
                alias: name.clone(),
            })?;
        }
    }
    let on = core.snapshot().ssh_on;
    let alias = match cmd {
        SshCmd::Alias { name } => Some(name),
        _ => None,
    };
    let mut text = format!(
        "SSH to your own machines is {}.",
        if on { "on" } else { "off" }
    );
    if let Some(a) = &alias {
        text.push_str(&format!(" This machine is called `{a}`."));
    }
    Ok(Out::new(text, json!({ "on": on, "alias": alias })))
}

fn activity(ctx: &Ctx, cmd: OnOff) -> Res {
    let mut core = open(ctx)?;
    match cmd {
        OnOff::Status => {}
        OnOff::Enable => {
            core.handle(Command::ActivitySet { on: true })?;
        }
        OnOff::Disable => {
            core.handle(Command::ActivitySet { on: false })?;
        }
    }
    let share = core.snapshot().activity_on;
    Ok(Out::new(
        format!(
            "Sharing your activity is {}.",
            if share { "on" } else { "off" }
        ),
        json!({ "share": share }),
    ))
}

fn packs(ctx: &Ctx, cmd: PacksCmd, group: Option<String>) -> Res {
    let group = group.map(|g| repo_arg(&g)).transpose()?;
    let mut core = open(ctx)?;
    need_sign_in(&core)?;
    let Event::Packs(all) = core.handle(Command::Packs {
        repo: group.clone(),
    })?
    else {
        return Err("no packs answer".into());
    };
    // The group to act on: --group, or the only joined group that has packs.
    let pick = |all: &[claude_sync_actions::PackView]| -> Result<String, Fail> {
        if let Some(g) = &group {
            return Ok(g.clone());
        }
        let mut repos: Vec<&str> = all.iter().map(|p| p.repo.as_str()).collect();
        repos.dedup();
        match repos.as_slice() {
            [one] => Ok((*one).into()),
            [] => Err("none of your groups has packs".into()),
            many => Err(Fail::Usage(format!(
                "several groups have packs ({}); add --group owner/repo",
                many.join(", ")
            ))),
        }
    };
    match cmd {
        PacksCmd::List => {
            if group.is_none() {
                pick(&all)?;
            }
            let mut text = String::new();
            for p in &all {
                text.push_str(&format!(
                    "{}  {}  [{}]{}\n    {}\n",
                    p.id,
                    p.title,
                    p.state,
                    if p.unsupported {
                        " (not for this system)"
                    } else {
                        ""
                    },
                    p.summary
                ));
            }
            if all.is_empty() {
                text.push_str("No packs in your groups.");
            }
            let json = all
                .iter()
                .map(|p| json!({ "repo": p.repo, "id": p.id, "title": p.title, "summary": p.summary, "state": p.state, "unsupported": p.unsupported, "os": p.os, "checkedAt": p.checked_at, "tail": p.tail }))
                .collect::<Vec<_>>();
            Ok(Out::new(text.trim_end(), json!({ "packs": json })))
        }
        PacksCmd::Setup { id } => {
            let repo = pick(&all)?;
            // as v1: Claude Code runs here, in the pack's folder; then the check runs
            let (dir, msg, claude) = core.pack_chat(&repo, &id)?;
            eprintln!("Starting Claude Code in {} ...", dir.display());
            std::process::Command::new(&claude)
                .arg(&msg)
                .current_dir(&dir)
                .status()
                .map_err(|e| format!("could not start Claude Code: {e}"))?;
            match core.handle(Command::PackCheck {
                repo: repo.clone(),
                id: id.clone(),
            }) {
                Ok(_) => Ok(Out::done(format!(
                    "check passed. Turn it on with: claude-sync packs on {id}"
                ))),
                Err(e) => Err(Fail::Run(format!(
                    "{e}\n{id} stays off on this machine. After you fix it, run:\n  claude-sync packs check {id}"
                ))),
            }
        }
        PacksCmd::Check { id } => {
            let repo = pick(&all)?;
            core.handle(Command::PackCheck {
                repo: repo.clone(),
                id: id.clone(),
            })?;
            Ok(Out::done(format!("The check of {id} passed in {repo}.")))
        }
        PacksCmd::On { id } => {
            let repo = pick(&all)?;
            core.handle(Command::PackOn {
                repo,
                id: id.clone(),
            })?;
            Ok(Out::done(format!("{id} is on.")))
        }
        PacksCmd::Off { id } => {
            let repo = pick(&all)?;
            core.handle(Command::PackOff {
                repo,
                id: id.clone(),
            })?;
            Ok(Out::done(format!("{id} is off.")))
        }
    }
}

fn review(ctx: &Ctx, cmd: ReviewCmd) -> Res {
    let mut core = open(ctx)?;
    need_sign_in(&core)?;
    match cmd {
        ReviewCmd::List => {
            let snap = core.snapshot();
            let mut entries: Vec<Value> = vec![];
            for g in snap.groups.iter().filter(|g| !g.off) {
                // A group with no CLAUDE.md to review is not an error in a list.
                if let Ok(Event::Review(lines)) = core.handle(Command::ReviewList {
                    repo: g.repo.clone(),
                }) {
                    for l in lines {
                        entries.push(json!({
                            "id": format!("md:{}:{}", g.repo, l.id),
                            "kind": "claude-md",
                            "group": g.repo,
                            "summary": format!("{} {} line(s) by {}: {}", if l.add { "adds" } else { "removes" }, l.lines.len(), l.author, l.lines.join(" / ")),
                            "accept": "put it in your CLAUDE.md copy",
                            "undo": "leave it out, and do not ask again",
                        }));
                    }
                }
                if let Ok(Event::Edits(list)) = core.handle(Command::EditsList {
                    repo: g.repo.clone(),
                }) {
                    for e in list.into_iter().filter(|e| !e.kept) {
                        entries.push(json!({
                            "id": format!("edit:{}:{}", g.repo, e.path),
                            "kind": "local-edit",
                            "group": g.repo,
                            "summary": format!("you edited {}", e.path),
                            "accept": "share it with the group",
                            "undo": "put the group's version back",
                        }));
                    }
                }
            }
            for j in snap.jobs.iter().filter(|j| !j.accepted) {
                entries.push(json!({
                    "id": format!("job:{}:{}", j.repo, j.id),
                    "kind": "job",
                    "group": j.repo,
                    "summary": format!("scheduled job {} waits for your yes", j.id),
                    "accept": "let it run on schedule",
                    "undo": "keep it off",
                }));
            }
            let text = if entries.is_empty() {
                "Nothing is waiting for you.".to_string()
            } else {
                entries
                    .iter()
                    .map(|e| {
                        format!(
                            "{}\n  {}\n  accept: {}; undo: {}",
                            e["id"].as_str().unwrap_or_default(),
                            e["summary"].as_str().unwrap_or_default(),
                            e["accept"].as_str().unwrap_or_default(),
                            e["undo"].as_str().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(Out::new(text, json!({ "items": entries })))
        }
        ReviewCmd::Accept { id } => decide(&mut core, &id, true),
        ReviewCmd::Undo { id } => decide(&mut core, &id, false),
    }
}

/// `md:<group>:<item>`, `edit:<group>:<path>` or `job:<group>:<job>`.
fn decide(core: &mut Core, id: &str, accept: bool) -> Res {
    let bad = || {
        Fail::Usage(format!(
            "`{id}` is not a review id; take one from `claude-sync-cli review list`"
        ))
    };
    let mut parts = id.splitn(3, ':');
    let (Some(kind), Some(group), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad());
    };
    let repo = repo_arg(group).map_err(|_| bad())?;
    if rest.is_empty() {
        return Err(bad());
    }
    let rest = rest.to_string();
    let said = match (kind, accept) {
        ("md", a) => {
            core.handle(Command::ReviewDecide {
                repo,
                ids: Some(vec![rest]),
                accept: a,
            })?;
            if a {
                "Accepted: it is in your CLAUDE.md copy."
            } else {
                "Declined: it stays out and will not be asked again."
            }
        }
        ("edit", true) => {
            core.handle(Command::ShareEdit { repo, path: rest })?;
            "Shared with the group."
        }
        ("edit", false) => {
            core.handle(Command::UndoEdit { repo, path: rest })?;
            "The group's version is back."
        }
        ("job", true) => {
            core.handle(Command::JobAccept { repo, id: rest })?;
            "The job is accepted and runs on schedule."
        }
        ("job", false) => "The job stays off.",
        _ => return Err(bad()),
    };
    Ok(Out::done(said))
}

fn update(ctx: &Ctx, check: bool) -> Res {
    let mut core = open(ctx)?;
    let Event::Text(t) = core.handle(Command::CheckUpdate)? else {
        return Err("no update answer".into());
    };
    let Some(version) = t.strip_prefix("available:").map(String::from) else {
        return Ok(Out::new(
            format!("{} {} is the newest version.", ctx.name, ctx.version),
            json!({ "current": ctx.version, "available": null }),
        ));
    };
    if check {
        return Ok(Out::new(
            format!(
                "Version {version} is available (this is {}). Run `{} update`.",
                ctx.version, ctx.name
            ),
            json!({ "current": ctx.version, "available": version }),
        ));
    }
    // The app binary updates itself (Settings, Updates); only the headless program swaps itself.
    if ctx.name != "claude-sync-cli" {
        return Err(format!(
            "Version {version} is available. The app updates itself: open Settings, Updates."
        )
        .into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let name = crate::update::asset_name(crate::update::target_triple()?);
    let client = claude_sync_github::Client::new(ctx.cfg.clone().unwrap_or_else(Config::github))
        .map_err(|e| e.to_string())?;
    crate::update::install(&client, &version, &name, &exe)?;
    Ok(Out::new(
        format!("Updated to {version}. A running daemon switches over at its next restart (`systemctl --user restart claude-sync`)."),
        json!({ "ok": true, "from": ctx.version, "to": version }),
    ))
}

/// `run-job <owner/repo> <id>` (what v1 tasks call) or `run-job <id>` (when one group has it).
fn run_job(ctx: &Ctx, args: &[String], now: bool) -> Res {
    let core = open(ctx)?;
    let (repo, id) = match args {
        [id] => match core.job_repos(id).as_slice() {
            [_] => (None, id),
            [] => {
                return Err(Fail::Usage(format!(
                    "no job {id:?} in any group on this device"
                )))
            }
            _ => {
                return Err(Fail::Usage(format!(
                    "job {id:?} is in more than one group: run-job <owner/repo> {id}"
                )))
            }
        },
        [repo, id] => (Some(repo_arg(repo)?), id),
        _ => {
            return Err(Fail::Usage(
                "run-job <owner/repo> <id> | run-job <id>".into(),
            ))
        }
    };
    let status = core.run_job(repo.as_deref(), id, now)?;
    if status.starts_with("error") || status.starts_with("failed") {
        return Err(Fail::Run(status));
    }
    Ok(Out::new(
        status.clone(),
        json!({ "ok": true, "status": status }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A v1 `~/.claude-sync/token` (login + tokens), as v1 wrote it.
    const V1_TOKEN: &str = r#"{"login":"ann","tokens":{"access_token":"gho_sample"}}"#;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_v1_token_file_is_a_sign_in_and_its_token_reaches_github() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .and(header("authorization", "Bearer gho_sample"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        let h = tempfile::tempdir().unwrap();
        let state = h.path().join(".claude-sync");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("token"), V1_TOKEN).unwrap();
        let mut cfg = Config::github();
        cfg.api = server.uri();
        let ctx = Ctx {
            home: Some(h.path().to_path_buf()),
            cfg: Some(cfg),
            ..Ctx::cli()
        };
        let signed_in = tokio::task::spawn_blocking(move || {
            let core = open(&ctx).map_err(|_| "open")?;
            let l = core.snapshot().login;
            Ok::<_, &str>((l, execute(&ctx, Cmd::Groups).is_ok()))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(signed_in.0.as_deref(), Some("ann"));
        let seen = server.received_requests().await.unwrap();
        assert!(
            !seen.is_empty(),
            "groups asked GitHub with the file's token"
        );
    }

    #[test]
    fn the_app_ctx_reads_its_own_store_not_the_token_file_and_the_file_is_v1_json() {
        use claude_sync_github::secrets::{save, Tokens};
        let h = tempfile::tempdir().unwrap();
        let state = h.path().join(".claude-sync");
        let mem = crate::token::MemSecrets::default();
        save(&mem, "bob", &Tokens::plain("gho_app")).unwrap();
        let ctx = Ctx {
            home: Some(h.path().to_path_buf()),
            secrets: Some(claude_sync_actions::Secrets(Arc::new(mem))),
            ..Ctx::cli()
        };
        assert_eq!(
            open_plain(&ctx).unwrap().snapshot().login.as_deref(),
            Some("bob")
        );
        assert!(!state.join("token").exists(), "the store is not the file");
        // the file the headless CLI writes carries no null keys, as v1
        let file = daemon::file_secrets(&state);
        save(&file, "ann", &Tokens::plain("gho_x")).unwrap();
        assert_eq!(
            std::fs::read_to_string(state.join("token"))
                .unwrap()
                .replace(char::is_whitespace, ""),
            r#"{"login":"ann","tokens":{"access_token":"gho_x"}}"#
        );
    }

    #[test]
    fn packs_list_signed_out_says_not_signed_in_like_every_other_command() {
        let h = tempfile::tempdir().unwrap();
        let ctx = Ctx {
            home: Some(h.path().to_path_buf()),
            ..Ctx::cli()
        };
        for cmd in [
            Cmd::Packs {
                cmd: None,
                group: None,
            },
            Cmd::Packs {
                cmd: Some(PacksCmd::List),
                group: None,
            },
            Cmd::Groups,
        ] {
            match execute(&ctx, cmd) {
                Err(Fail::Run(m)) => assert_eq!(m, "not signed in: run `claude-sync-cli login`"),
                _ => panic!("expected the not signed in failure"),
            }
        }
    }
}
