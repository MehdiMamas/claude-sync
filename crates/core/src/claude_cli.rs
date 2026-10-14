//! Finding and running the `claude` CLI (§16, §17).
//!
//! Neither the tray app nor a scheduled task gets a login shell's `PATH` (launchd has almost
//! none), so the path is resolved here: `where.exe` on Windows (an `.exe` before a `.cmd`),
//! the usual install locations elsewhere, then the login shell. Every child runs without a
//! console window and is killed with its whole process tree when it runs too long.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// `CREATE_NO_WINDOW`: a console child of a GUI app would otherwise open a window.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The `claude` binary on this machine, if any.
pub fn resolve(home: &Path) -> Option<PathBuf> {
    let found = search(home);
    pick(&found)
}

/// The best of the candidates that exist: on Windows an `.exe`, then a `.cmd`; elsewhere the
/// first one.
pub fn pick(found: &[PathBuf]) -> Option<PathBuf> {
    let ext = |p: &PathBuf, e: &str| {
        p.extension()
            .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case(e))
    };
    if cfg!(windows) {
        found
            .iter()
            .find(|p| ext(p, "exe"))
            .or_else(|| found.iter().find(|p| ext(p, "cmd")))
            .cloned()
    } else {
        found.first().cloned()
    }
}

#[cfg(windows)]
fn search(home: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut cmd = Command::new("where.exe");
    cmd.arg("claude");
    if let Ok(out) = run_command(cmd, Duration::from_secs(10)) {
        found.extend(
            out.stdout
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_file()),
        );
    }
    let mut fixed = vec![
        home.join(".local").join("bin").join("claude.exe"),
        home.join(".claude").join("local").join("claude.exe"),
    ];
    if let Some(appdata) = std::env::var_os("APPDATA") {
        fixed.push(PathBuf::from(appdata).join("npm").join("claude.cmd"));
    }
    found.extend(fixed.into_iter().filter(|p| p.is_file()));
    found
}

#[cfg(not(windows))]
fn search(home: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = [
        home.join(".local/bin/claude"),
        home.join(".claude/local/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
        home.join(".npm-global/bin/claude"),
    ]
    .into_iter()
    .chain(
        std::env::var_os("PATH")
            .map(|p| {
                std::env::split_paths(&p)
                    .map(|d| d.join("claude"))
                    .collect()
            })
            .unwrap_or_else(Vec::new),
    )
    .filter(|p| p.is_file())
    .collect();
    if found.is_empty() {
        // The login shell knows the PATH the user set up (nvm, asdf, ...).
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut cmd = Command::new(shell);
        cmd.args(["-lc", "command -v claude"]);
        if let Ok(out) = run_command(cmd, Duration::from_secs(10)) {
            found.extend(
                out.stdout
                    .lines()
                    .map(str::trim)
                    .filter(|l| l.starts_with('/'))
                    .map(PathBuf::from)
                    .filter(|p| p.is_file()),
            );
        }
    }
    found
}

/// No console window on Windows; its own process group elsewhere, so a timeout kills the
/// whole tree.
pub fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

/// Kill `child` and everything it started.
pub fn kill_tree(child: &mut Child) {
    let pid = child.id().to_string();
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &pid, "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = quiet(&mut cmd).status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// Wait for `child` until `deadline`; kill its tree after that. `None`: it timed out.
pub fn wait_until(child: &mut Child, deadline: Instant) -> std::io::Result<Option<i32>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status.code().unwrap_or(-1)));
        }
        if Instant::now() >= deadline {
            kill_tree(child);
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[derive(Debug, Clone, Default)]
pub struct Output {
    /// `None` when the command timed out and was killed.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Run `program args` without a window, capturing its output, killed after `timeout`.
pub fn run(program: &Path, args: &[&str], timeout: Duration) -> std::io::Result<Output> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    run_command(cmd, timeout)
}

/// [`run`] for a prepared command (arguments, environment).
pub fn run_command(mut cmd: Command, timeout: Duration) -> std::io::Result<Output> {
    quiet(&mut cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let code = wait_until(&mut child, Instant::now() + timeout)?;
    Ok(Output {
        code,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_an_exe_before_a_cmd_on_windows() {
        let found = [
            PathBuf::from("C:/npm/claude.cmd"),
            PathBuf::from("C:/bin/claude"),
            PathBuf::from("C:/x/claude.EXE"),
        ];
        let want = if cfg!(windows) {
            "C:/x/claude.EXE"
        } else {
            "C:/npm/claude.cmd"
        };
        assert_eq!(pick(&found), Some(PathBuf::from(want)));
        assert_eq!(pick(&[]), None);
        if cfg!(windows) {
            assert_eq!(
                pick(&[
                    PathBuf::from("C:/bin/claude"),
                    PathBuf::from("C:/n/claude.cmd")
                ]),
                Some(PathBuf::from("C:/n/claude.cmd"))
            );
        }
    }

    #[test]
    fn run_captures_output_and_times_out() {
        let (shell, flag, echo, slow) = if cfg!(windows) {
            ("cmd", "/C", "echo hi", "ping -n 30 127.0.0.1 >NUL")
        } else {
            ("sh", "-c", "echo hi", "sleep 30")
        };
        let out = run(Path::new(shell), &[flag, echo], Duration::from_secs(30)).unwrap();
        assert!(out.ok());
        assert_eq!(out.stdout.trim(), "hi");
        let start = Instant::now();
        let out = run(Path::new(shell), &[flag, slow], Duration::from_millis(500)).unwrap();
        assert_eq!(out.code, None);
        assert!(start.elapsed() < Duration::from_secs(20));
    }
}
