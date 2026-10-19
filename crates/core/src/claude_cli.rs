//! Finding and running the `claude` CLI (§17, §18).
//!
//! Neither the tray app nor a scheduled task gets a login shell's `PATH` (launchd has almost
//! none), so the path is resolved here: `where.exe` on Windows (an `.exe` before a `.cmd`),
//! the usual install locations elsewhere, then the login shell. Every child runs without a
//! console window and is killed with its whole process tree when it runs too long. A job
//! session gets a full user `PATH` ([`session_path`]) so hooks find `node`, `gh` and `claude`.
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
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

/// The `PATH` for a scheduled session: the login shell's entries, then the current ones, then
/// `claude`'s own directory and the usual user bin directories that exist; first one wins.
pub fn session_path(
    home: &Path,
    claude: &Path,
    current: Option<&OsStr>,
    login: Option<&OsStr>,
    exists: &dyn Fn(&Path) -> bool,
) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for list in [login, current].into_iter().flatten() {
        dirs.extend(std::env::split_paths(list));
    }
    dirs.extend(claude.parent().map(Path::to_path_buf));
    dirs.extend(
        [
            home.join(".local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            home.join(".cargo/bin"),
            home.join(".bun/bin"),
            home.join(".npm-global/bin"),
        ]
        .into_iter()
        .filter(|d| exists(d)),
    );
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| !d.as_os_str().is_empty() && seen.insert(d.clone()));
    std::env::join_paths(dirs).unwrap_or_default()
}

/// The `PATH` an interactive login shell builds (nvm, asdf, brew, ...). Windows tasks already
/// run with the user's environment.
#[cfg(not(windows))]
pub fn login_shell_path() -> Option<OsString> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut cmd = Command::new(shell);
    cmd.args(["-ilc", "printf %s \"$PATH\""]);
    let out = run_command(cmd, Duration::from_secs(10)).ok()?;
    // Interactive rc files may print noise first; the PATH is the last line.
    let last = out.stdout.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    last.contains('/').then(|| OsString::from(last))
}

#[cfg(windows)]
pub fn login_shell_path() -> Option<OsString> {
    None
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
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
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

/// Kill `child` and everything it started. `child` must have been spawned through [`quiet`],
/// and not yet reaped, so that on unix its pid is still its process group's id.
pub fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = quiet(&mut cmd).status();
    }
    #[cfg(unix)]
    {
        // A syscall, not the `kill` command: procps 4.0.4 (Ubuntu 24.04) fails to parse
        // `kill -KILL -<pgid>`, which left the grandchildren running.
        if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
            // SAFETY: killpg takes no pointers; at worst the group is gone (ESRCH).
            unsafe { libc::killpg(pgid, libc::SIGKILL) };
        }
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

/// How long the output is still read after the child exits or is killed. A process that left
/// its group (or a daemon it started) can hold the pipes open for good.
const PIPE_GRACE: Duration = Duration::from_secs(2);

/// [`run`] for a prepared command (arguments, environment).
pub fn run_command(mut cmd: Command, timeout: Duration) -> std::io::Result<Output> {
    quiet(&mut cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let deadline = Instant::now() + timeout;
    let mut child = cmd.spawn()?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let code = wait_until(&mut child, deadline)?;
    let until = deadline.max(Instant::now()) + PIPE_GRACE;
    Ok(Output {
        code,
        stdout: out.collect(until),
        stderr: err.collect(until),
    })
}

/// A pipe read to its end on its own thread.
struct Drain {
    buf: Arc<Mutex<Vec<u8>>>,
    /// Disconnects when the reader is done.
    done: mpsc::Receiver<()>,
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> Drain {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let (tx, done) = mpsc::channel::<()>();
    let shared = Arc::clone(&buf);
    std::thread::spawn(move || {
        let _tx = tx;
        let Some(mut pipe) = pipe else { return };
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    Drain { buf, done }
}

impl Drain {
    /// What was read, waiting for the end of the pipe until `until` at most.
    fn collect(self, until: Instant) -> String {
        let _ = self
            .done
            .recv_timeout(until.saturating_duration_since(Instant::now()));
        let buf = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&buf).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    fn sp(login: Option<&str>, current: Option<&str>, have: &[&str]) -> String {
        let have: Vec<PathBuf> = have.iter().map(PathBuf::from).collect();
        let exists = |p: &Path| have.iter().any(|h| h == p);
        let out = session_path(
            Path::new("/h"),
            Path::new("/n/bin/claude"),
            current.map(OsStr::new),
            login.map(OsStr::new),
            &exists,
        );
        out.to_string_lossy().into_owned()
    }

    #[cfg(not(windows))]
    #[test]
    fn session_path_puts_login_entries_first() {
        let p = sp(Some("/l/bin:/x"), Some("/usr/bin:/bin"), &[]);
        assert!(p.starts_with("/l/bin:/x:/usr/bin:/bin"), "{p}");
    }

    #[cfg(not(windows))]
    #[test]
    fn session_path_adds_only_existing_standard_dirs() {
        let p = sp(
            None,
            Some("/usr/bin"),
            &["/h/.local/bin", "/opt/homebrew/bin"],
        );
        assert_eq!(p, "/usr/bin:/n/bin:/h/.local/bin:/opt/homebrew/bin");
    }

    #[cfg(not(windows))]
    #[test]
    fn session_path_has_no_duplicates() {
        let p = sp(
            Some("/n/bin:/usr/bin"),
            Some("/usr/bin:/n/bin"),
            &["/usr/bin"],
        );
        assert_eq!(p, "/n/bin:/usr/bin");
    }

    #[cfg(not(windows))]
    #[test]
    fn session_path_with_empty_inputs_keeps_claude_and_standard_dirs() {
        let p = sp(None, None, &["/usr/local/bin", "/h/.cargo/bin"]);
        assert_eq!(p, "/n/bin:/usr/local/bin:/h/.cargo/bin");
    }

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
            // Not the last command, so the shell cannot exec it: `sleep` is a grandchild that
            // holds the pipes until the whole group is killed.
            ("sh", "-c", "echo hi", "sleep 30; exit 0")
        };
        let out = run(Path::new(shell), &[flag, echo], Duration::from_secs(30)).unwrap();
        assert!(out.ok());
        assert_eq!(out.stdout.trim(), "hi");
        let start = Instant::now();
        let out = run(Path::new(shell), &[flag, slow], Duration::from_millis(500)).unwrap();
        assert_eq!(out.code, None);
        assert!(start.elapsed() < Duration::from_secs(20));
    }

    #[cfg(unix)]
    #[test]
    fn a_timeout_kills_the_grandchildren() {
        let out = run(
            Path::new("sh"),
            &["-c", "sleep 30 & echo $!; wait"],
            Duration::from_millis(500),
        )
        .unwrap();
        assert_eq!(out.code, None);
        let pid: libc::pid_t = out.stdout.trim().parse().unwrap();
        // Reparented to init once `sh` is gone, which reaps it shortly after.
        let deadline = Instant::now() + Duration::from_secs(10);
        // SAFETY: signal 0 only checks that the process exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(Instant::now() < deadline, "sleep {pid} is still running");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
