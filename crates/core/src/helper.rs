//! Optional Claude helper (§19): explains a collision or a CLAUDE.md change and summarises
//! incoming group changes with the user's own Claude Code login: it runs the local `claude -p`
//! command. No API key is stored or sent anywhere by this app. Every call is on demand.
use crate::github::token_store::Store;
use crate::{claude_cli, secrets, Error, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The keychain account an older version saved a pasted API key under. It is only deleted.
const LEGACY_KEY_ACCOUNT: &str = "@anthropic-key";
pub const MODEL: &str = "sonnet";
/// Text longer than this is cut, with a note, before it is sent.
pub const MAX_INPUT: usize = 40 * 1024;
pub const TIMEOUT: Duration = Duration::from_secs(120);
pub const MISSING: &str =
    "Claude Code isn't installed or signed in. Install it and run `claude` once to sign in.";

const SYSTEM: &str = "You help a user of claude-sync, an app that shares Claude Code settings in groups. Explain plainly in a few short bullets what changed and what the user should decide. Do not repeat the text back. No preamble. Answer from the text below only; do not use tools.";

/// What the user is asking about; it only changes the first line of the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Task {
    /// Two rules or settings from different groups clash (§7b).
    Collision,
    /// Lines others changed in the group's CLAUDE.md wait for review (§7a).
    ClaudeMdReview,
    /// Incoming changes in the group's activity feed.
    Activity,
}

impl Task {
    fn intro(self) -> &'static str {
        match self {
            Task::Collision => "These two rules or settings from different groups clash. Say how they differ and which one to keep:",
            Task::ClaudeMdReview => "Other members changed the group's CLAUDE.md. Say what the changes do and whether to accept them:",
            Task::Activity => "These are recent changes other members made to the group. Summarise them:",
        }
    }

    /// Shown instead of asking Claude when there is no text to send.
    fn nothing(self) -> &'static str {
        match self {
            Task::Activity => {
                "No other member changed the group recently, so there is nothing to summarise."
            }
            Task::Collision | Task::ClaudeMdReview => "There is nothing to explain yet.",
        }
    }
}

/// Finds and runs `claude`. The app uses [`LocalClaude`]; tests use a fake, so they never
/// start the real CLI.
pub trait Runner: Send + Sync {
    /// The `claude` command, if installed.
    fn find(&self) -> Option<PathBuf>;
    /// Run it with `args`, `prompt` on stdin.
    fn run(
        &self,
        claude: &Path,
        args: &[String],
        prompt: &str,
    ) -> std::io::Result<claude_cli::Output>;
}

pub struct LocalClaude;

impl Runner for LocalClaude {
    fn find(&self) -> Option<PathBuf> {
        claude_cli::resolve(&std::env::home_dir()?)
    }
    fn run(
        &self,
        claude: &Path,
        args: &[String],
        prompt: &str,
    ) -> std::io::Result<claude_cli::Output> {
        let mut cmd = std::process::Command::new(claude);
        cmd.args(args)
            // Not a project folder, so no project CLAUDE.md or settings are read.
            .current_dir(std::env::temp_dir())
            .env("DISABLE_AUTOUPDATER", "1")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
        claude_cli::run_command_input(cmd, Some(prompt.to_string()), TIMEOUT)
    }
}

/// Whether the helper can run: `claude` is installed.
pub fn available(runner: &dyn Runner) -> bool {
    runner.find().is_some()
}

/// Delete the API key an older version saved, once. True when there was one.
pub fn remove_legacy_key(store: &impl Store) -> Result<bool> {
    let had = store.get(LEGACY_KEY_ACCOUNT)?.is_some();
    if had {
        store.delete(LEGACY_KEY_ACCOUNT)?;
    }
    Ok(had)
}

/// The arguments of one call: no tools, one turn, JSON output.
pub fn build_args() -> Vec<String> {
    [
        "-p",
        "--model",
        MODEL,
        "--output-format",
        "json",
        "--max-turns",
        "1",
        "--disallowedTools",
        "Bash,Edit,Write,NotebookEdit,WebFetch,WebSearch,Task,Agent,Read,Grep,Glob",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// The answer text of `claude --output-format json`.
pub fn parse_output(stdout: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        Error::Invalid("Claude answered in a form this app does not understand.".into())
    })?;
    match v["result"].as_str() {
        Some(r) if v["is_error"] != true && !r.trim().is_empty() => Ok(r.trim().to_string()),
        _ => Err(Error::Invalid(
            "Claude could not answer. Open `claude` once to check you are signed in, then try again.".into(),
        )),
    }
}

/// Cut `text` to `MAX_INPUT` bytes on a char boundary, with a note.
fn cap(text: &str) -> String {
    if text.len() <= MAX_INPUT {
        return text.to_string();
    }
    let mut end = MAX_INPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[…cut: the text was longer than {} KB]",
        &text[..end],
        MAX_INPUT / 1024
    )
}

fn run_claude(runner: &dyn Runner, prompt: &str) -> Result<String> {
    let claude = runner
        .find()
        .ok_or_else(|| Error::Invalid(MISSING.into()))?;
    let out = runner
        .run(&claude, &build_args(), prompt)
        .map_err(|_| Error::Invalid(MISSING.into()))?;
    match out.code {
        None => Err(Error::Invalid(
            "Claude took too long to answer. Try again.".into(),
        )),
        // A non-zero exit still prints JSON when Claude itself failed; parse it for the reason.
        Some(_) => parse_output(&out.stdout),
    }
}

/// Explain or summarise `text`. Refuses, without sending, when the text is empty or holds a
/// secret (§9).
pub fn explain(runner: &dyn Runner, task: Task, text: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Err(Error::Invalid(task.nothing().into()));
    }
    if let Some(pattern) = secrets::first_hit(text) {
        return Err(Error::Invalid(format!(
            "This text looks like it contains a secret ({pattern}), so it was not sent to Claude. Remove the secret first."
        )));
    }
    let prompt = format!("{SYSTEM}\n\n{}\n\n{}", task.intro(), cap(text));
    run_claude(runner, &prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemStore(RefCell<HashMap<String, String>>);
    impl Store for MemStore {
        fn get(&self, a: &str) -> Result<Option<String>> {
            Ok(self.0.borrow().get(a).cloned())
        }
        fn set(&self, a: &str, s: &str) -> Result<()> {
            self.0.borrow_mut().insert(a.into(), s.into());
            Ok(())
        }
        fn delete(&self, a: &str) -> Result<()> {
            self.0.borrow_mut().remove(a);
            Ok(())
        }
    }

    struct Fake {
        installed: bool,
        out: Option<(Option<i32>, String)>,
        seen: Mutex<Vec<(Vec<String>, String)>>,
    }
    impl Fake {
        fn new(code: i32, stdout: &str) -> Self {
            Fake {
                installed: true,
                out: Some((Some(code), stdout.into())),
                seen: Mutex::default(),
            }
        }
        fn missing() -> Self {
            Fake {
                installed: false,
                out: None,
                seen: Mutex::default(),
            }
        }
    }
    impl Runner for Fake {
        fn find(&self) -> Option<PathBuf> {
            self.installed.then(|| PathBuf::from("/fake/claude"))
        }
        fn run(
            &self,
            _: &Path,
            args: &[String],
            prompt: &str,
        ) -> std::io::Result<claude_cli::Output> {
            self.seen
                .lock()
                .unwrap()
                .push((args.to_vec(), prompt.into()));
            let (code, stdout) = self.out.clone().unwrap();
            Ok(claude_cli::Output {
                code,
                stdout,
                stderr: String::new(),
            })
        }
    }

    const OK: &str = r#"{"type":"result","is_error":false,"result":"- one\n- two\n"}"#;

    #[test]
    fn args_use_sonnet_json_and_no_tools() {
        let a = build_args();
        assert_eq!(
            &a[..5],
            ["-p", "--model", "sonnet", "--output-format", "json"]
        );
        let dis = &a[a.iter().position(|x| x == "--disallowedTools").unwrap() + 1];
        assert!(dis.contains("Bash") && dis.contains("Write") && dis.contains("WebFetch"));
    }

    #[test]
    fn output_is_parsed() {
        assert_eq!(parse_output(OK).unwrap(), "- one\n- two");
        assert!(parse_output("not json").is_err());
        assert!(parse_output(r#"{"is_error":true,"result":"x"}"#).is_err());
        assert!(parse_output(r#"{"subtype":"error_max_turns"}"#).is_err());
        assert!(parse_output(r#"{"result":"  "}"#).is_err());
    }

    #[test]
    fn explain_sends_the_prompt_on_stdin_to_the_runner() {
        let f = Fake::new(0, OK);
        let out = explain(&f, Task::Collision, "use tabs\nuse spaces").unwrap();
        assert_eq!(out, "- one\n- two");
        let seen = f.seen.lock().unwrap();
        assert!(seen[0].1.contains("use spaces") && seen[0].1.contains("clash"));
        assert_eq!(seen[0].0, build_args());
    }

    #[test]
    fn missing_claude_gives_the_plain_error() {
        let f = Fake::missing();
        assert!(!available(&f));
        let err = explain(&f, Task::Activity, "x").unwrap_err().to_string();
        assert!(err.contains("isn't installed or signed in"), "{err}");
        assert!(f.seen.lock().unwrap().is_empty());
        assert!(available(&Fake::new(0, OK)));
    }

    #[test]
    fn timeout_and_failure_are_plain_words() {
        let mut f = Fake::new(0, "");
        f.out = Some((None, String::new()));
        let err = explain(&f, Task::Activity, "x").unwrap_err().to_string();
        assert!(err.contains("too long"), "{err}");
        let err = explain(&Fake::new(1, "boom"), Task::Activity, "x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not understand"), "{err}");
        let err = explain(
            &Fake::new(1, r#"{"is_error":true,"result":"Not logged in"}"#),
            Task::Activity,
            "x",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("signed in"), "{err}");
    }

    #[test]
    fn text_with_a_secret_is_not_sent() {
        let f = Fake::new(0, OK);
        let text = format!("token: ghp_{}", "a".repeat(36));
        let err = explain(&f, Task::Activity, &text).unwrap_err().to_string();
        assert!(err.contains("not sent"), "{err}");
        assert!(!err.contains("ghp_aaa"));
        assert!(f.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn empty_text_is_not_sent() {
        let f = Fake::new(0, OK);
        let err = explain(&f, Task::Activity, " \n\n ")
            .unwrap_err()
            .to_string();
        assert!(err.contains("nothing to summarise"), "{err}");
        assert!(explain(&f, Task::Collision, "").is_err());
        assert!(f.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn long_text_is_cut_with_a_note() {
        let f = Fake::new(0, OK);
        explain(&f, Task::Activity, &"é".repeat(MAX_INPUT)).unwrap();
        let prompt = f.seen.lock().unwrap()[0].1.clone();
        assert!(prompt.contains("cut"));
        assert!(prompt.len() < MAX_INPUT + 2048);
    }

    #[test]
    fn an_old_api_key_is_deleted_once() {
        let s = MemStore::default();
        s.set(LEGACY_KEY_ACCOUNT, "old-key").unwrap();
        s.set("@current", "gh-token").unwrap();
        assert!(remove_legacy_key(&s).unwrap());
        assert_eq!(s.get(LEGACY_KEY_ACCOUNT).unwrap(), None);
        assert_eq!(s.get("@current").unwrap().as_deref(), Some("gh-token"));
        assert!(!remove_legacy_key(&s).unwrap());
    }
}
