#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo)
)]
//! claude-sync command line (v2-Plan step 6). The command tree and `run` live here, so the
//! headless binary and the app binary (step 7) answer the same words with the same exit codes:
//! 0 done, 1 failed, 2 wrong usage. Everything goes through `claude-sync-actions`.
pub mod adopt;
pub mod daemon;
mod refresh;
mod run;
pub mod service;
pub mod token;
pub mod tree;
pub mod update;

use clap::Parser;
use claude_sync_github::Config;
use std::path::PathBuf;
use std::process::ExitCode;

pub use run::{env, status_json, v1_daemon_message, Out};
pub use tree::{command_names, Cli, Cmd};

/// What differs between the binaries that embed the tree, and between a person and a test.
#[derive(Clone)]
pub struct Ctx {
    /// The program's name in messages.
    pub name: &'static str,
    pub version: &'static str,
    /// The person's home; `None`: the OS home directory.
    pub home: Option<PathBuf>,
    /// GitHub endpoints; `None`: github.com.
    pub cfg: Option<Config>,
    /// The app sets this: it alone answers the hidden `remove-jobs`.
    pub remove_jobs: bool,
    /// The app binary, for `--refresh` after a change the window shows; the app sets this.
    pub refresh: Option<PathBuf>,
    /// Where the sign-in lives; `None`: the token file `<state>/token` (the headless CLI, as v1).
    /// The app sets the OS keychain, so `claude-sync status` reads the app's own sign-in.
    pub secrets: Option<claude_sync_actions::Secrets>,
}

impl Ctx {
    pub fn cli() -> Ctx {
        Ctx {
            name: "claude-sync-cli",
            version: env!("CARGO_PKG_VERSION"),
            home: None,
            cfg: None,
            remove_jobs: false,
            refresh: None,
            secrets: None,
        }
    }
}

/// Why a command did not finish: 2 wrong usage, 1 anything else.
#[derive(Debug)]
pub enum Fail {
    Usage(String),
    Run(String),
}

impl From<String> for Fail {
    fn from(s: String) -> Self {
        Fail::Run(s)
    }
}

impl From<&str> for Fail {
    fn from(s: &str) -> Self {
        Fail::Run(s.into())
    }
}

impl From<claude_sync_actions::Error> for Fail {
    fn from(e: claude_sync_actions::Error) -> Self {
        Fail::Run(e.0)
    }
}

/// Messages name commands as `claude-sync-cli ...`; say the program that was started.
fn named(text: &str, ctx: &Ctx) -> String {
    text.replace("claude-sync-cli", ctx.name)
}

/// `args` as `std::env::args()` gives them: the program name first.
pub fn run(args: Vec<String>, ctx: &Ctx) -> ExitCode {
    ExitCode::from(run_status(args, ctx))
}

/// [`run`] with the exit code as a number (0 done, 1 failed, 2 wrong usage).
pub fn run_status(mut args: Vec<String>, ctx: &Ctx) -> u8 {
    // v1 took `--version` and `-V` as the version command.
    if matches!(args.get(1).map(String::as_str), Some("--version" | "-V")) {
        args[1] = "version".into();
    }
    let cli = match Cli::try_parse_from(&args) {
        Ok(c) => c,
        Err(e) => {
            let code = e.exit_code();
            let _ = e.print();
            return code as u8;
        }
    };
    if cli.cmd == Cmd::RemoveJobs && !ctx.remove_jobs {
        eprintln!("error: unrecognized subcommand 'remove-jobs'");
        return 2;
    }
    // The commands that run for long or from the OS scheduler write the log file (as v1).
    let _log = matches!(cli.cmd, Cmd::Daemon | Cmd::RunJob { .. } | Cmd::Sync)
        .then(|| {
            let home = ctx.home.clone().or_else(std::env::home_dir)?;
            claude_sync_fs::logging::init(&claude_sync_fs::Home::from_env(&home).state)
        })
        .flatten();
    match run::execute(ctx, cli.cmd) {
        Ok(out) => {
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&out.json).unwrap_or_default()
                );
            } else if !out.text.is_empty() {
                println!("{}", named(&out.text, ctx));
            }
            0
        }
        Err(f) => {
            let (code, msg) = match f {
                Fail::Usage(m) => (2, m),
                Fail::Run(m) => (1, m),
            };
            if cli.json && code == 1 {
                println!("{}", serde_json::json!({ "ok": false, "error": msg }));
            } else {
                eprintln!("{}: {}", ctx.name, named(&msg, ctx));
            }
            code
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_name_the_program_that_was_started() {
        let app = Ctx {
            name: "claude-sync",
            ..Ctx::cli()
        };
        let m = "not signed in: run `claude-sync-cli login`";
        assert_eq!(named(m, &app), "not signed in: run `claude-sync login`");
        assert_eq!(named(m, &Ctx::cli()), m);
    }
}
