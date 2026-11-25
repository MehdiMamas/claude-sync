//! `claude-sync packs [list | setup <id> | check <id> | on <id> | off <id>] [--group owner/repo]
//! [--json]`: the same operations as the Optional packs card, for WSL and other machines with no
//! window. The app binary and `claude-sync-cli` both call [`run`]; each supplies a [`Host`]
//! that applies a change (the engine and the job scheduler differ between them).
//!
//! Exit code: 0 done, 1 failed (stderr says why), 2 wrong usage.
use super::launch;
use super::ops::{self, Env, PackView};
use crate::state::AppState;
use crate::{GroupId, Paths};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub const USAGE: &str = "usage: claude-sync packs [list | setup <id> | check <id> | on <id> | off <id>] [--group owner/repo] [--json]";

/// What differs between the app binary and the headless CLI.
pub trait Host {
    /// Claude Code, if it is installed.
    fn claude(&self) -> Option<PathBuf>;
    /// Put a state change into effect: apply the group again (plugins, rule lines, files),
    /// accept `jobs` (`(id, consent hash)`) and reconcile the scheduler.
    fn apply(&self, group: &GroupId, jobs: &[(String, String)]) -> crate::Result<()>;
    /// The group's display name.
    fn group_name(&self, group: &GroupId) -> String {
        group.to_string()
    }
    /// Run Claude Code in this terminal, in `folder`, with `message`, and wait.
    fn chat(&self, claude: &Path, folder: &Path, message: &str) -> std::io::Result<i32> {
        let status = std::process::Command::new(claude)
            .arg(message)
            .current_dir(folder)
            .status()?;
        Ok(status.code().unwrap_or(-1))
    }
    /// Ask a yes/no question; anything but y or yes is no.
    fn confirm(&self, question: &str, out: &mut dyn Write) -> bool {
        let _ = write!(out, "{question} [y/N] ");
        let _ = out.flush();
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).is_err() {
            return false;
        }
        matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    List,
    Setup(String),
    Check(String),
    On(String),
    Off(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub cmd: Cmd,
    pub group: Option<GroupId>,
    pub json: bool,
}

/// `args` are what follows `packs`.
pub fn parse(args: &[String]) -> Result<Args, String> {
    let mut json = false;
    let mut group = None;
    let mut words: Vec<&str> = Vec::new();
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            "--json" => json = true,
            "--group" => {
                let g = it.next().ok_or("--group needs owner/repo")?;
                group = Some(GroupId::parse(g).ok_or_else(|| format!("{g:?} is not owner/repo"))?);
            }
            _ if a.starts_with("--") => return Err(format!("unknown option {a}")),
            _ => words.push(a),
        }
    }
    let id = |w: &[&str]| -> Result<String, String> {
        match w {
            [one] if super::valid_id(one) => Ok((*one).to_string()),
            [one] => Err(format!("{one:?} is not a pack id")),
            _ => Err("this command needs one pack id".into()),
        }
    };
    let cmd = match words.as_slice() {
        [] | ["list"] => Cmd::List,
        ["setup", rest @ ..] => Cmd::Setup(id(rest)?),
        ["check", rest @ ..] => Cmd::Check(id(rest)?),
        ["on", rest @ ..] => Cmd::On(id(rest)?),
        ["off", rest @ ..] => Cmd::Off(id(rest)?),
        [other, ..] => return Err(format!("unknown command {other:?}")),
    };
    Ok(Args { cmd, group, json })
}

fn state_word(v: &PackView) -> &'static str {
    match v.state {
        "available" | "tried" => "not set up",
        "ready" => "ready to turn on",
        "on" => "on",
        "failing" => "on, check fails",
        "off" => "off",
        _ => "not for this system",
    }
}

/// The group to act on: `--group`, or the only joined group that has packs.
fn pick_group(paths: &Paths, wanted: Option<GroupId>) -> Result<GroupId, (i32, String)> {
    let joined = AppState::load(paths)
        .map_err(|e| (1, e.to_string()))?
        .groups;
    if let Some(g) = wanted {
        return if joined.contains(&g) {
            Ok(g)
        } else {
            Err((1, format!("you are not in {g} on this machine")))
        };
    }
    let with: Vec<&GroupId> = joined
        .iter()
        .filter(|g| {
            super::load_catalog(paths, g)
                .is_ok_and(|c| !c.packs.is_empty() || !c.problems.is_empty())
        })
        .collect();
    match with.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err((1, "none of your groups has packs".into())),
        many => Err((
            2,
            format!(
                "several groups have packs ({}); add --group owner/repo",
                many.iter()
                    .map(|g| g.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn view_of(paths: &Paths, group: &GroupId, id: &str, env: &Env) -> crate::Result<PackView> {
    ops::list(paths, group, env)?
        .packs
        .into_iter()
        .find(|v| v.id == id)
        .ok_or_else(|| crate::Error::Invalid(format!("this group has no pack {id:?}")))
}

fn job_lines(v: &PackView) -> Vec<String> {
    v.jobs
        .iter()
        .map(|j| {
            format!(
                "  scheduled job {}: {} (your time), at most {} min, in {}, permission mode {}\n    prompt: {}",
                j.title, j.schedule, j.timeout_minutes, j.cwd, j.permission_mode, j.prompt
            )
        })
        .collect()
}

/// What turning the pack on changes, for the question.
fn what_it_does(v: &PackView) -> String {
    let mut parts = Vec::new();
    let mods: Vec<&str> = v
        .plugins
        .iter()
        .map(|p| p.split('@').next().unwrap_or(p))
        .collect();
    if !mods.is_empty() {
        parts.push(format!(
            "turns on the mod{} {}",
            if mods.len() == 1 { "" } else { "s" },
            mods.join(", ")
        ));
    }
    if v.rule_lines > 0 {
        parts.push(format!(
            "adds {} rule line{}",
            v.rule_lines,
            if v.rule_lines == 1 { "" } else { "s" }
        ));
    }
    if !v.jobs.is_empty() {
        parts.push(format!(
            "registers {} scheduled job{}",
            v.jobs.len(),
            if v.jobs.len() == 1 { "" } else { "s" }
        ));
    }
    if v.has_files {
        parts.push("places the pack's skills or agents".into());
    }
    if parts.is_empty() {
        "It changes nothing else".into()
    } else {
        format!("It {}", parts.join(" and "))
    }
}

/// Run `claude-sync packs ...` with `args` (what follows `packs`). Output goes to `out` and
/// `err`; the exit code is returned.
pub fn run(
    paths: &Paths,
    host: &dyn Host,
    env: &Env,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let parsed = match parse(args) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(err, "{e}\n{USAGE}");
            return 2;
        }
    };
    let group = match pick_group(paths, parsed.group.clone()) {
        Ok(g) => g,
        Err((code, msg)) => {
            let _ = writeln!(err, "claude-sync: {msg}");
            return code;
        }
    };
    match go(paths, host, env, &group, &parsed, out) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(err, "claude-sync: {e}");
            1
        }
    }
}

fn go(
    paths: &Paths,
    host: &dyn Host,
    env: &Env,
    group: &GroupId,
    a: &Args,
    out: &mut dyn Write,
) -> crate::Result<i32> {
    let say = |out: &mut dyn Write, s: String| {
        let _ = writeln!(out, "{s}");
    };
    match &a.cmd {
        Cmd::List => {
            let o = ops::list(paths, group, env)?;
            if a.json {
                say(
                    out,
                    serde_json::to_string_pretty(&serde_json::json!({
                        "group": group.to_string(),
                        "host": o.host,
                        "packs": o.packs,
                        "problems": o.problems,
                    }))?,
                );
                return Ok(0);
            }
            if o.packs.is_empty() && o.problems.is_empty() {
                say(out, format!("{} has no packs.", host.group_name(group)));
                return Ok(0);
            }
            say(
                out,
                format!("{} ({group}), on this machine:", host.group_name(group)),
            );
            let width = o.packs.iter().map(|p| p.id.len()).max().unwrap_or(0) + 2;
            for p in &o.packs {
                let mut line = format!("  {:<width$}{:<20}{}", p.id, state_word(p), p.title);
                if !p.jobs.is_empty() {
                    line.push_str(" (scheduled job)");
                }
                if p.state == "unavailable" {
                    line.push_str(&format!(" (works on {})", p.os.join(", ")));
                }
                say(out, line);
                if matches!(p.state, "tried" | "available") {
                    if let Some(c) = p.last_check.as_ref().filter(|c| !c.passed()) {
                        say(
                            out,
                            format!("  {:<width$}  last check failed: {}", "", c.tail),
                        );
                    }
                }
            }
            for p in &o.problems {
                say(
                    out,
                    format!(
                        "  skipped {}: {}",
                        p.id.as_deref().unwrap_or("(packs)"),
                        p.message
                    ),
                );
            }
            say(out, "Set one up: claude-sync packs setup <id>".into());
            Ok(0)
        }
        Cmd::Setup(id) => {
            let plan = ops::start_setup(paths, group, id, env)?;
            let Some(claude) = host.claude() else {
                say(
                    out,
                    "Claude Code isn't installed or signed in on this machine. Install it, then run this again.".into(),
                );
                return Ok(1);
            };
            say(
                out,
                format!("Starting Claude Code in {} ...", plan.folder.display()),
            );
            let msg = launch::message(&plan.title, &host.group_name(group));
            if let Err(e) = host.chat(&claude, &plan.folder, &msg) {
                return Err(crate::Error::Invalid(format!(
                    "could not start Claude Code: {e}"
                )));
            }
            say(out, "(the setup chat ran here; Claude Code exited)".into());
            say(out, "Running the pack's check ...".into());
            let r = ops::check_pack(paths, group, id, env)?;
            if !r.passed {
                say(out, format!("check failed (exit {}): {}", r.exit, r.tail));
                say(
                    out,
                    format!(
                        "{id} stays off on this machine. After you fix it, run:\n  claude-sync packs check {id}"
                    ),
                );
                return Ok(1);
            }
            say(out, "check passed".into());
            offer(paths, host, env, group, id, out)
        }
        Cmd::Check(id) => {
            let r = ops::check_pack(paths, group, id, env)?;
            if a.json {
                say(out, serde_json::to_string_pretty(&r)?);
            } else if r.passed {
                say(out, "check passed".into());
                let v = view_of(paths, group, id, env)?;
                if v.state == "ready" {
                    say(out, format!("Turn it on with: claude-sync packs on {id}"));
                }
            } else {
                say(out, format!("check failed (exit {}): {}", r.exit, r.tail));
            }
            Ok(i32::from(!r.passed))
        }
        Cmd::On(id) => {
            let mut v = view_of(paths, group, id, env)?;
            if v.state == "on" {
                say(out, format!("{id} is on already."));
                return Ok(0);
            }
            // Turn on again runs the check first (the check only counts for the pack as it is).
            let fresh = v
                .last_check
                .as_ref()
                .is_some_and(|c| c.passed() && c.at + 5 * 60_000 > env.now_ms);
            if v.state != "ready" || !fresh {
                let r = ops::check_pack(paths, group, id, env)?;
                if !r.passed {
                    say(out, format!("check failed (exit {}): {}", r.exit, r.tail));
                    return Ok(1);
                }
                v = view_of(paths, group, id, env)?;
            }
            turn_on(paths, host, env, group, &v, out)?;
            if a.json {
                let v = view_of(paths, group, id, env)?;
                say(out, serde_json::to_string_pretty(&v)?);
            }
            Ok(0)
        }
        Cmd::Off(id) => {
            ops::turn_off(paths, group, id, env)?;
            host.apply(group, &[])?;
            if a.json {
                let v = view_of(paths, group, id, env)?;
                say(out, serde_json::to_string_pretty(&v)?);
            } else {
                say(out, format!("{id} is off for this machine."));
            }
            Ok(0)
        }
    }
}

/// After a passing check in `setup`: show what turning on changes and ask.
fn offer(
    paths: &Paths,
    host: &dyn Host,
    env: &Env,
    group: &GroupId,
    id: &str,
    out: &mut dyn Write,
) -> crate::Result<i32> {
    let v = view_of(paths, group, id, env)?;
    for l in job_lines(&v) {
        let _ = writeln!(out, "{l}");
    }
    let q = format!("Turn on {id} on this machine? {}.", what_it_does(&v));
    if !host.confirm(&q, out) {
        let _ = writeln!(
            out,
            "{id} stays off. Turn it on later with: claude-sync packs on {id}"
        );
        return Ok(0);
    }
    turn_on(paths, host, env, group, &v, out)?;
    Ok(0)
}

fn turn_on(
    paths: &Paths,
    host: &dyn Host,
    env: &Env,
    group: &GroupId,
    v: &PackView,
    out: &mut dyn Write,
) -> crate::Result<()> {
    let hashes: BTreeMap<String, String> = v
        .jobs
        .iter()
        .map(|j| (j.id.clone(), j.hash.clone()))
        .collect();
    let on = ops::turn_on(paths, group, &v.id, &hashes, env)?;
    host.apply(group, &on.jobs)?;
    let _ = writeln!(out, "{} is on for this machine.", v.id);
    if on.plugins_changed {
        let _ = writeln!(
            out,
            "Run /reload-plugins in open Claude Code sessions, or restart them."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_every_command_and_flag() {
        let p = |s: &str| parse(&args(s));
        assert_eq!(p("").unwrap().cmd, Cmd::List);
        assert!(p("list --json").unwrap().json);
        assert_eq!(
            p("setup busy-switch").unwrap().cmd,
            Cmd::Setup("busy-switch".into())
        );
        assert_eq!(p("check a").unwrap().cmd, Cmd::Check("a".into()));
        assert_eq!(p("on a").unwrap().cmd, Cmd::On("a".into()));
        assert_eq!(p("off a --group o/r").unwrap().group, GroupId::parse("o/r"));
        assert!(p("on").is_err());
        assert!(p("on A").is_err());
        assert!(p("on a b").is_err());
        assert!(p("frobnicate").is_err());
        assert!(p("list --nope").is_err());
        assert!(p("list --group").is_err());
        assert!(p("list --group nope").is_err());
    }
}
