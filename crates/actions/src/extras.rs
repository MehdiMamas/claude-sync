//! Small switches and lookups: SSH between my machines (Parity 80), activity (81), the Claude
//! helper (40) and the update state (51). The OS calls (idle time, installing an update) stay
//! with the shell; this only decides and records.
use crate::core::Core;
use crate::error::{Error, Result};
use crate::model::{Event, ExplainTask};
use claude_sync_engine::traits::Store;
use claude_sync_fs::ssh;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const MISSING: &str =
    "Claude Code isn't installed or signed in. Install it and run `claude` once to sign in.";
const SYSTEM: &str = "You help a user of claude-sync, an app that shares Claude Code settings in groups. Explain plainly in a few short bullets what changed and what the user should decide. Do not repeat the text back. No preamble. Answer from the text below only; do not use tools.";
const MAX_INPUT: usize = 24 * 1024;
const IDLE_AFTER: u64 = 120;
const HEARTBEAT: i64 = 300;

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or(d.into())
}

/// `a.b.c` as numbers; anything else sorts as 0.
fn ver(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split(['.', '-', '+'])
        .map_while(|p| p.parse().ok())
        .collect()
}

impl Core {
    fn set_rest(&self, key: &str, v: Value) -> Result<()> {
        let mut c = self.choices();
        c.rest.insert(key.into(), v);
        Ok(c.save(&self.db)?)
    }

    fn ssh_env(&self) -> ssh::Env<'_> {
        ssh::Env {
            home: &self.home,
            now: self.now(),
            hostname: env_or("HOSTNAME", &env_or("COMPUTERNAME", "this-computer")),
            user: env_or("USER", &env_or("USERNAME", "user")),
        }
    }

    pub(crate) fn ssh_on(&self) -> bool {
        self.choices()
            .rest
            .get("sshPeers")
            .and_then(|p| p["on"].as_bool())
            .unwrap_or(false)
    }

    pub(crate) fn ssh_set(&mut self, on: bool) -> Result<Event> {
        self.set_rest("sshPeers", json!({"on": on}))?;
        ssh::pass(&self.ssh_env(), &ssh::RealProbe, on).map_err(Error::new)?;
        Ok(Event::Done)
    }

    pub(crate) fn ssh_alias(&mut self, alias: &str) -> Result<Event> {
        ssh::set_alias(&self.ssh_env(), alias).map_err(Error::new)?;
        Ok(Event::Done)
    }

    /// After a personal sync: keep this machine's record and the others' keys current.
    pub(crate) fn ssh_pass(&self) {
        if self.ssh_on() {
            let _ = ssh::pass(&self.ssh_env(), &ssh::RealProbe, true);
        }
    }

    pub(crate) fn activity_set(&mut self, on: bool) -> Result<Event> {
        self.set_rest("activity", json!({"share": on}))?;
        if !on {
            // data kept elsewhere; only the status file this switch made goes
            let _ = std::fs::remove_file(self.home.state.join("activity.json"));
        }
        Ok(Event::Done)
    }

    /// Writes `<state>/activity.json` when the state changed or the last write is 5 minutes old.
    pub(crate) fn activity_tick(&mut self, idle: Option<u64>) -> Result<Event> {
        if !self.snapshot_activity_on() {
            return Ok(Event::Done);
        }
        let state = match idle {
            Some(s) if s >= IDLE_AFTER => "idle",
            Some(_) => "active",
            None => "unknown",
        };
        let file = self.home.state.join("activity.json");
        let old: Value = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let now = self.now();
        let fresh = old["state"] == state && now - old["ts"].as_i64().unwrap_or(0) < HEARTBEAT;
        if !fresh {
            std::fs::create_dir_all(&self.home.state)?;
            std::fs::write(&file, json!({"state": state, "ts": now}).to_string())?;
        }
        Ok(Event::Done)
    }

    fn snapshot_activity_on(&self) -> bool {
        self.choices()
            .rest
            .get("activity")
            .and_then(|p| p["share"].as_bool())
            .unwrap_or(false)
    }

    pub(crate) fn explain(&mut self, task: ExplainTask, text: &str) -> Result<Event> {
        if self.choices().app.helper == Some(false) {
            return Err(Error::new(
                "The Claude helper is off. Turn it on in Settings, Advanced.",
            ));
        }
        if text.trim().is_empty() {
            return Err(Error::new(match task {
                ExplainTask::Activity => {
                    "No other member changed the group recently, so there is nothing to summarise."
                }
                _ => "There is nothing to explain yet.",
            }));
        }
        if let Some(p) = claude_sync_engine::secrets::first_hit(text) {
            return Err(Error::new(format!("This text looks like it contains a secret ({p}), so it was not sent to Claude. Remove the secret first.")));
        }
        let intro = match task {
            ExplainTask::Collision => "These two rules or settings from different groups clash. Say how they differ and which one to keep:",
            ExplainTask::ClaudeMdReview => "Other members changed the group's CLAUDE.md. Say what the changes do and whether to accept them:",
            ExplainTask::Activity => "These are recent changes other members made to the group. Summarise them:",
        };
        let mut cut = text.len().min(MAX_INPUT);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let prompt = format!("{SYSTEM}\n\n{intro}\n\n{}", &text[..cut]);
        let claude = (self.jobs_env.claude)().ok_or_else(|| Error::new(MISSING))?;
        let out = run_claude(&claude, &prompt)
            .ok_or_else(|| Error::new("Claude took too long to answer. Try again."))?;
        let v: Value = serde_json::from_str(out.trim())
            .map_err(|_| Error::new("Claude answered in a form this app does not understand."))?;
        match v["result"].as_str() {
            Some(r) if v["is_error"] != true && !r.trim().is_empty() => Ok(Event::Text(r.trim().into())),
            _ => Err(Error::new("Claude could not answer. Open `claude` once to check you are signed in, then try again.")),
        }
    }

    /// Reads `latest.json` of the newest release and records whether it is newer than this build.
    pub(crate) fn check_update(&mut self) -> Result<Event> {
        let latest = self.client.latest()?;
        let newer = ver(&latest.version) > ver(env!("CARGO_PKG_VERSION"));
        let mut v = if newer {
            format!("available:{}", latest.version)
        } else {
            String::new()
        };
        // A download the shell already verified stays ready.
        if self.update_state() == format!("ready:{}", latest.version) {
            v = self.update_state();
        }
        Store::write(&self.db, "update", v.as_bytes())?;
        Store::write(
            &self.db,
            "update-checked",
            self.now().to_string().as_bytes(),
        )?;
        Ok(Event::Text(v))
    }

    /// The shell (the updater plugin) reports `available:<v>` (downloading), `ready:<v>`
    /// (verified) or `failed:<v>`, or `` when up to date; the check time is stamped here.
    pub(crate) fn set_update(&mut self, state: &str) -> Result<Event> {
        // "Updated to <v>" stays until the next update shows up.
        let kept = self.update_state();
        let state = if state.is_empty() && kept.starts_with("updated:") {
            kept.as_str()
        } else {
            state
        };
        Store::write(&self.db, "update", state.as_bytes())?;
        Store::write(
            &self.db,
            "update-checked",
            self.now().to_string().as_bytes(),
        )?;
        Ok(Event::Done)
    }

    /// `` (up to date or unchecked), `available:<v>`, `ready:<v>`, `failed:<v>` or
    /// `updated:<v>` (a verified download that is now the running version).
    pub(crate) fn update_state(&self) -> String {
        let stored = Store::read(&self.db, "update")
            .ok()
            .flatten()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let Some((kind, v)) = stored.split_once(':') else {
            return stored;
        };
        if ver(v) > ver(env!("CARGO_PKG_VERSION")) {
            return stored;
        }
        match kind {
            "ready" | "updated" => format!("updated:{v}"),
            _ => String::new(),
        }
    }

    pub(crate) fn update_checked(&self) -> Option<i64> {
        let b = Store::read(&self.db, "update-checked").ok().flatten()?;
        String::from_utf8_lossy(&b).parse().ok()
    }

    pub(crate) fn host_lock(&self, repo: &str, host: bool) -> Option<String> {
        if !host {
            return None;
        }
        Store::read(&self.db, &format!("host-lock/{repo}"))
            .ok()
            .flatten()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

/// `claude -p` with the prompt on stdin, no tools, one turn; `None` after 120 s.
fn run_claude(claude: &std::path::Path, prompt: &str) -> Option<String> {
    let mut c = Command::new(claude);
    claude_sync_fs::quiet(&mut c);
    c.args([
        "-p",
        "--output-format",
        "json",
        "--max-turns",
        "1",
        "--disallowedTools",
        "Bash,Edit,Write,NotebookEdit,WebFetch,WebSearch,Task,Agent,Read,Grep,Glob",
    ])
    .current_dir(std::env::temp_dir())
    .env("DISABLE_AUTOUPDATER", "1")
    .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    let mut child = c.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let p = prompt.to_string();
    std::thread::spawn(move || {
        let _ = stdin.write_all(p.as_bytes());
    });
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    while child.try_wait().ok()?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    reader.join().ok()
}

#[cfg(test)]
mod tests {
    use super::ver;

    #[test]
    fn versions_compare_by_number_not_text() {
        assert!(ver("2.0.0") > ver("0.1.27"));
        assert!(ver("v10.0.0") > ver("9.9.9"));
        assert!(ver("2.0.1") > ver("2.0.0"));
        assert!(ver("2.0.0") == ver("v2.0.0"));
        assert!(ver("2.0.0-rc1") <= ver("2.0.0"));
    }
}
