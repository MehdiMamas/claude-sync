//! claude-sync-cli: claude-sync without a desktop (issue #158). It uses the same core as
//! the app: sign in with `login`, `join` a group, `sync`, and `service install` to keep
//! syncing in the background. Exit codes: 0 done, 1 failed, 2 wrong usage.
mod adopt;
mod args;
mod auth;
mod cmds;
mod config;
mod daemon;
mod jobs;
mod review;
mod service;
mod store;
mod update;

use args::Cmd;
use claude_sync_core::Paths;
use std::sync::Arc;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&argv));
}

fn run(argv: &[String]) -> i32 {
    let (cmd, json) = match args::parse(argv) {
        Ok(v) => v,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };
    let paths = match Paths::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("claude-sync-cli: {e}");
            return 1;
        }
    };
    // Commands that run for long or from the scheduler log to the state folder.
    let _log = matches!(cmd, Cmd::Daemon | Cmd::RunJob(_) | Cmd::Sync)
        .then(|| claude_sync_core::logging::init(&paths).ok())
        .flatten();
    if let Cmd::RunJob(rest) = &cmd {
        return jobs::run_job_main(&paths, rest);
    }
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("claude-sync-cli: {e}");
            return 1;
        }
    };
    if cmd == Cmd::Daemon {
        let Some(_one) = daemon::single_instance(&paths) else {
            println!("claude-sync-cli daemon is already running.");
            return 0;
        };
        rt.block_on(daemon::run(
            &paths,
            Arc::new(jobs::RealHooks),
            daemon::stop_signal(),
        ));
        return 0;
    }
    match rt.block_on(cmds::execute(&paths, cmd)) {
        Ok(out) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&out.json).unwrap_or_default()
                );
            } else {
                println!("{}", out.text);
            }
            0
        }
        Err(e) => {
            if json {
                println!("{}", serde_json::json!({ "ok": false, "error": e }));
            } else {
                eprintln!("claude-sync-cli: {e}");
            }
            1
        }
    }
}
