//! "Set up with Claude": open a terminal that runs Claude Code in the pack's folder with a
//! short first message (D4: an interactive chat, normal permission mode, so the member can pair
//! a phone or sign in).
//!
//! The terminal runs a small script claude-sync writes into `<state>/packs-setup/`. It starts
//! `claude`, and when Claude Code exits it writes a `.done` marker. That marker, not a process
//! id, tells the app the chat is over (a terminal app hands its window to another process, so
//! there is no id to wait for); the group screen then runs the pack's check. Nothing to quote
//! on a command line: the message and paths live in the script, each in its own shell quoting.
use crate::{GroupId, Paths};
use std::path::{Path, PathBuf};

/// A setup chat that was started and never closed is forgotten after this long.
const STALE: std::time::Duration = std::time::Duration::from_secs(12 * 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    None,
    /// The terminal was opened and Claude Code has not exited.
    Open,
    /// Claude Code exited; the check is due.
    Closed,
}

fn dir(paths: &Paths) -> PathBuf {
    paths.state_dir.join("packs-setup")
}

fn stem(group: &GroupId, id: &str) -> String {
    format!("{}__{id}", group.dir_name())
}

fn marker(paths: &Paths, group: &GroupId, id: &str, ext: &str) -> PathBuf {
    dir(paths).join(format!("{}.{ext}", stem(group, id)))
}

pub fn session(paths: &Paths, group: &GroupId, id: &str) -> Session {
    if marker(paths, group, id, "done").is_file() {
        return Session::Closed;
    }
    let run = marker(paths, group, id, "run");
    let fresh = std::fs::metadata(&run)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < STALE);
    if fresh {
        Session::Open
    } else {
        Session::None
    }
}

/// Forget a setup chat (the check ran, or it never started).
pub fn clear(paths: &Paths, group: &GroupId, id: &str) {
    for ext in ["run", "done", "cmd", "command", "sh"] {
        let _ = std::fs::remove_file(marker(paths, group, id, ext));
    }
}

/// The first message: short, no quotes that a shell would care about.
pub fn message(title: &str, group_name: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_alphanumeric() || " .,:-_()+/#".contains(c) {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "Read setup.md in this folder and help me set up the '{}' pack of the group {}.",
        clean(title),
        clean(group_name)
    )
}

/// POSIX single quoting.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The script a terminal runs, and the file extension it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub ext: &'static str,
    pub body: String,
}

/// `Err`: a path cmd.exe could not quote safely.
pub fn script(
    os: &str,
    folder: &Path,
    claude: &Path,
    message: &str,
    done: &Path,
) -> Result<Script, String> {
    if os == "windows" {
        let bad = |p: &Path| {
            p.to_string_lossy()
                .contains(['"', '%', '^', '&', '|', '<', '>', '!'])
        };
        if bad(folder) || bad(claude) || bad(done) {
            return Err("a folder name has a character cmd.exe cannot quote".into());
        }
        let body = format!(
            "@echo off\r\ncd /d \"{}\"\r\ncall \"{}\" \"{}\"\r\necho done> \"{}\"\r\necho.\r\necho Claude Code closed. claude-sync now checks the pack; you can close this window.\r\ntimeout /t 5 >nul\r\n",
            folder.display(),
            claude.display(),
            message,
            done.display()
        );
        return Ok(Script { ext: "cmd", body });
    }
    let body = format!(
        "#!/bin/sh\ncd {} || exit 1\n{} {}\ntouch {}\necho\necho 'Claude Code closed. claude-sync now checks the pack; you can close this window.'\nsleep 3\n",
        sh_quote(&folder.to_string_lossy()),
        sh_quote(&claude.to_string_lossy()),
        sh_quote(message),
        sh_quote(&done.to_string_lossy())
    );
    Ok(Script {
        ext: if os == "macos" { "command" } else { "sh" },
        body,
    })
}

/// The commands that might open `script`, best first. `has(name)` says whether a program is
/// on `PATH`. The caller runs the first one that starts.
pub fn openers(
    os: &str,
    script: &Path,
    folder: &Path,
    has: &dyn Fn(&str) -> bool,
) -> Vec<Vec<String>> {
    let s = script.to_string_lossy().into_owned();
    let list = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match os {
        "windows" => {
            let mut out = Vec::new();
            if has("wt.exe") {
                out.push(list(&[
                    "wt.exe",
                    "-d",
                    &folder.to_string_lossy(),
                    "cmd.exe",
                    "/c",
                    &s,
                ]));
            }
            out.push(list(&[
                "cmd.exe",
                "/c",
                "start",
                "claude-sync setup",
                "cmd.exe",
                "/c",
                &s,
            ]));
            out
        }
        "macos" => vec![list(&["open", "-a", "Terminal", &s])],
        _ => {
            let mut out = Vec::new();
            let mut add = |prog: &str, args: &[&str]| {
                if has(prog) {
                    let mut c = vec![prog.to_string()];
                    c.extend(args.iter().map(|a| a.to_string()));
                    c.extend(["sh".to_string(), s.clone()]);
                    out.push(c);
                }
            };
            add("x-terminal-emulator", &["-e"]);
            add("gnome-terminal", &["--"]);
            add("konsole", &["-e"]);
            add("xfce4-terminal", &["-x"]);
            add("alacritty", &["-e"]);
            add("kitty", &[]);
            add("xterm", &["-e"]);
            out
        }
    }
}

/// The command to type by hand when no terminal can be opened.
pub fn manual_command(os: &str, folder: &Path, claude: &Path, message: &str) -> String {
    if os == "windows" {
        format!(
            "cd /d \"{}\" && \"{}\" \"{}\"",
            folder.display(),
            claude.display(),
            message
        )
    } else {
        format!(
            "cd {} && {} {}",
            sh_quote(&folder.to_string_lossy()),
            sh_quote(&claude.to_string_lossy()),
            sh_quote(message)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Started,
    /// Claude Code is not installed (or not found).
    NoClaude,
    /// No terminal could be opened; `command` is what to run by hand.
    NoTerminal {
        command: String,
    },
}

/// Does `name` exist in a `PATH` folder?
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// Open the terminal for `plan`. `claude` is where Claude Code is (`None`: not installed).
pub fn start(
    paths: &Paths,
    group: &GroupId,
    plan: &super::ops::SetupPlan,
    group_name: &str,
    os: &str,
    claude: Option<&Path>,
) -> crate::Result<Outcome> {
    let Some(claude) = claude else {
        return Ok(Outcome::NoClaude);
    };
    let msg = message(&plan.title, group_name);
    let manual = || Outcome::NoTerminal {
        command: manual_command(os, &plan.folder, claude, &msg),
    };
    clear(paths, group, &plan.pack);
    std::fs::create_dir_all(dir(paths))?;
    let done = marker(paths, group, &plan.pack, "done");
    let Ok(s) = script(os, &plan.folder, claude, &msg, &done) else {
        return Ok(manual());
    };
    let file = marker(paths, group, &plan.pack, s.ext);
    crate::state::write_atomic(&file, s.body.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))?;
    }
    for cmd in openers(os, &file, &plan.folder, &on_path) {
        let mut c = std::process::Command::new(&cmd[0]);
        c.args(&cmd[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // The terminal brings its own window; the helper process needs none.
            c.creation_flags(crate::claude_cli::CREATE_NO_WINDOW);
        }
        if let Ok(mut child) = c.spawn() {
            std::fs::write(marker(paths, group, &plan.pack, "run"), b"")?;
            // Reap it when it ends (no zombie on Unix).
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(Outcome::Started);
        }
    }
    Ok(manual())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_has_nothing_a_shell_would_read() {
        let m = message("Pass\"Bridge; $(x)", "Mehdi's group `x`");
        assert!(!m.contains(['"', ';', '$', '`', '\\', '&', '%']), "{m}");
        assert!(m.starts_with("Read setup.md in this folder"));
        assert!(m.contains("'Pass Bridge (x)'"), "{m}");
    }

    #[test]
    fn unix_scripts_quote_every_path_and_write_the_marker() {
        let s = script(
            "linux",
            Path::new("/home/a b/it's/packs/x"),
            Path::new("/usr/bin/claude"),
            "Read setup.md",
            Path::new("/state/x.done"),
        )
        .unwrap();
        assert_eq!(s.ext, "sh");
        assert!(s.body.starts_with("#!/bin/sh\n"));
        assert!(s.body.contains(r"cd '/home/a b/it'\''s/packs/x' || exit 1"));
        assert!(s.body.contains("'/usr/bin/claude' 'Read setup.md'"));
        assert!(s.body.contains("touch '/state/x.done'"));
        let m = script(
            "macos",
            Path::new("/p"),
            Path::new("/c"),
            "m",
            Path::new("/d"),
        )
        .unwrap();
        assert_eq!(m.ext, "command");
    }

    #[test]
    fn windows_scripts_call_claude_and_refuse_unquotable_paths() {
        let s = script(
            "windows",
            Path::new(r"C:\Users\a\packs\x"),
            Path::new(r"C:\Users\a\claude.cmd"),
            "Read setup.md",
            Path::new(r"C:\s\x.done"),
        )
        .unwrap();
        assert_eq!(s.ext, "cmd");
        assert!(s.body.contains("cd /d \"C:\\Users\\a\\packs\\x\"\r\n"));
        assert!(s
            .body
            .contains("call \"C:\\Users\\a\\claude.cmd\" \"Read setup.md\"\r\n"));
        assert!(s.body.contains("echo done> \"C:\\s\\x.done\""));
        let bad = script(
            "windows",
            Path::new(r"C:\100%\x"),
            Path::new("c"),
            "m",
            Path::new("d"),
        );
        assert!(bad.is_err());
    }

    #[test]
    fn each_os_tries_its_terminals_in_order() {
        let script = Path::new("/s/x.sh");
        let folder = Path::new("/f");
        let none = openers("linux", script, folder, &|_| false);
        assert!(none.is_empty());
        let some = openers("linux", script, folder, &|p| {
            p == "xterm" || p == "gnome-terminal"
        });
        assert_eq!(some[0], ["gnome-terminal", "--", "sh", "/s/x.sh"]);
        assert_eq!(some[1], ["xterm", "-e", "sh", "/s/x.sh"]);
        let mac = openers("macos", script, folder, &|_| false);
        assert_eq!(mac, [["open", "-a", "Terminal", "/s/x.sh"]]);
        let win = openers("windows", script, folder, &|p| p == "wt.exe");
        assert_eq!(win[0][0], "wt.exe");
        assert_eq!(win.last().unwrap()[0], "cmd.exe");
        let win_plain = openers("windows", script, folder, &|_| false);
        assert_eq!(win_plain.len(), 1);
    }

    #[test]
    fn a_session_is_open_until_the_marker_appears() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths {
            claude_home: t.path().join("c"),
            state_dir: t.path().join("s"),
        };
        let g = GroupId::new("o", "r");
        assert_eq!(session(&paths, &g, "x"), Session::None);
        std::fs::create_dir_all(dir(&paths)).unwrap();
        std::fs::write(marker(&paths, &g, "x", "run"), b"").unwrap();
        assert_eq!(session(&paths, &g, "x"), Session::Open);
        std::fs::write(marker(&paths, &g, "x", "done"), b"").unwrap();
        assert_eq!(session(&paths, &g, "x"), Session::Closed);
        clear(&paths, &g, "x");
        assert_eq!(session(&paths, &g, "x"), Session::None);
    }

    #[test]
    fn without_claude_nothing_is_written() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths {
            claude_home: t.path().join("c"),
            state_dir: t.path().join("s"),
        };
        let g = GroupId::new("o", "r");
        let plan = super::super::ops::SetupPlan {
            folder: t.path().into(),
            title: "T".into(),
            pack: "x".into(),
        };
        assert_eq!(
            start(&paths, &g, &plan, "G", "linux", None).unwrap(),
            Outcome::NoClaude
        );
        assert!(!dir(&paths).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_script_runs_claude_and_leaves_the_marker() {
        let t = tempfile::tempdir().unwrap();
        let claude = t.path().join("claude");
        std::fs::write(&claude, "#!/bin/sh\npwd > \"$(dirname \"$0\")/where\"\necho \"$1\" > \"$(dirname \"$0\")/arg\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let folder = t.path().join("pack dir");
        std::fs::create_dir_all(&folder).unwrap();
        let done = t.path().join("x.done");
        let s = script("linux", &folder, &claude, "Read setup.md", &done).unwrap();
        let file = t.path().join("run.sh");
        std::fs::write(&file, s.body.replace("sleep 3", "true")).unwrap();
        let st = std::process::Command::new("sh")
            .arg(&file)
            .status()
            .unwrap();
        assert!(st.success());
        assert!(done.is_file());
        let where_ = std::fs::read_to_string(t.path().join("where")).unwrap();
        assert!(where_.trim().ends_with("pack dir"), "{where_}");
        assert_eq!(
            std::fs::read_to_string(t.path().join("arg"))
                .unwrap()
                .trim(),
            "Read setup.md"
        );
    }
}
