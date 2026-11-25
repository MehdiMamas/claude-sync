//! A pack's check: a program from `pack.json` (`check.run`) run with no shell, in the pack
//! folder, with a timeout. Exit 0 passes. Only the last stderr line is kept, redacted.
use super::CheckSpec;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TAIL_MAX: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The exit code; `-1` when the check did not start, was killed or was cut short.
    pub exit: i32,
    pub tail: String,
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.exit == 0
    }
}

/// The last non-empty line of `text`, redacted and cut to 200 characters.
pub fn tail_of(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("");
    let line = crate::secrets::redact(line);
    if line.chars().count() > TAIL_MAX {
        let cut: String = line.chars().take(TAIL_MAX - 1).collect();
        format!("{cut}…")
    } else {
        line
    }
}

fn drain(mut r: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        // Keep the end: a long output is cut from the front.
        let mut chunk = [0u8; 4096];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > 64 * 1024 {
                buf.drain(..buf.len() - 16 * 1024);
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// Run the check in `folder`. Never panics and never waits longer than the timeout.
pub fn run(folder: &Path, spec: &CheckSpec) -> Outcome {
    let Some((program, args)) = spec.run.split_first() else {
        return Outcome {
            exit: -1,
            tail: "the pack has no check".into(),
        };
    };
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(crate::claude_cli::CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Outcome {
                exit: -1,
                tail: tail_of(&format!("could not start {program}: {e}")),
            }
        }
    };
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    let limit = Duration::from_secs(u64::from(spec.timeout_seconds));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => {
                return Outcome {
                    exit: -1,
                    tail: tail_of(&format!("could not wait for the check: {e}")),
                }
            }
        }
    };
    let join = |h: Option<std::thread::JoinHandle<String>>| {
        h.and_then(|h| h.join().ok()).unwrap_or_default()
    };
    let (stdout, stderr) = (join(out), join(err));
    match status {
        None => Outcome {
            exit: -1,
            tail: format!("timed out after {} s", spec.timeout_seconds),
        },
        Some(s) => {
            let code = s.code().unwrap_or(-1);
            let mut tail = tail_of(&stderr);
            if tail.is_empty() && code != 0 {
                tail = tail_of(&stdout);
            }
            Outcome { exit: code, tail }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(run: &[&str], secs: u32) -> CheckSpec {
        CheckSpec {
            run: run.iter().map(|s| s.to_string()).collect(),
            timeout_seconds: secs,
        }
    }

    #[test]
    fn the_tail_is_the_last_line_redacted_and_cut() {
        assert_eq!(tail_of("one\ntwo\n\n"), "two");
        assert_eq!(tail_of(""), "");
        let long = "x".repeat(500);
        assert_eq!(tail_of(&long).chars().count(), 200);
        assert!(tail_of("token ghp_abcdefghijklmnopqrstuvwxyz0123456789").contains("[redacted"));
    }

    #[cfg(unix)]
    #[test]
    fn exit_codes_and_stderr_come_back() {
        let d = tempfile::tempdir().unwrap();
        let pass = run(d.path(), &spec(&["sh", "-c", "exit 0"], 5));
        assert!(pass.passed());
        let fail = run(
            d.path(),
            &spec(&["sh", "-c", "echo noise; echo 'no agent' >&2; exit 3"], 5),
        );
        assert_eq!(
            fail,
            Outcome {
                exit: 3,
                tail: "no agent".into()
            }
        );
        // Without stderr the last stdout line explains a failure.
        let out = run(
            d.path(),
            &spec(&["sh", "-c", "echo only stdout; exit 1"], 5),
        );
        assert_eq!(out.tail, "only stdout");
    }

    #[cfg(unix)]
    #[test]
    fn the_check_runs_in_the_pack_folder_without_a_shell() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("marker"), b"x").unwrap();
        let ok = run(d.path(), &spec(&["test", "-f", "marker"], 5));
        assert!(ok.passed(), "{ok:?}");
        // `;` is an argument, not a second command.
        let o = run(d.path(), &spec(&["echo", "a;", "exit", "7"], 5));
        assert!(o.passed());
    }

    #[cfg(unix)]
    #[test]
    fn a_check_that_hangs_is_killed_at_its_timeout() {
        let d = tempfile::tempdir().unwrap();
        let t = Instant::now();
        let o = run(d.path(), &spec(&["sleep", "30"], 1));
        assert!(t.elapsed() < Duration::from_secs(10));
        assert_eq!(o.exit, -1);
        assert_eq!(o.tail, "timed out after 1 s");
    }

    #[test]
    fn a_missing_program_fails_with_a_reason() {
        let d = tempfile::tempdir().unwrap();
        let o = run(d.path(), &spec(&["definitely-not-a-program-xyz"], 5));
        assert_eq!(o.exit, -1);
        assert!(o.tail.starts_with("could not start"), "{}", o.tail);
    }
}
