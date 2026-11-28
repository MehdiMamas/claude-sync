//! "Set up with Claude" (Parity 79, v1 `packs/launch.rs`): a terminal that runs Claude Code in
//! the pack's folder with a short first message, so the member can pair a phone or sign in.
//! The terminal runs a script written to `<state>/packs-setup/`; it ends by touching a `.done`
//! marker (a terminal app hands its window to another process, so there is no id to wait for).
use std::path::{Path, PathBuf};

/// The first message: short, nothing a shell would read.
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

/// The script a terminal runs, and the file extension it needs. `Err`: a path cmd.exe cannot
/// quote safely.
pub fn script(
    os: &str,
    folder: &Path,
    claude: &Path,
    message: &str,
    done: &Path,
) -> Result<(&'static str, String), String> {
    if os == "windows" {
        let bad = |p: &Path| {
            p.to_string_lossy()
                .contains(['"', '%', '^', '&', '|', '<', '>', '!'])
        };
        if bad(folder) || bad(claude) || bad(done) {
            return Err("a folder name has a character cmd.exe cannot quote".into());
        }
        return Ok((
            "cmd",
            format!(
                "@echo off\r\ncd /d \"{}\"\r\ncall \"{}\" \"{}\"\r\necho done> \"{}\"\r\necho.\r\necho Claude Code closed. claude-sync now checks the pack; you can close this window.\r\ntimeout /t 5 >nul\r\n",
                folder.display(),
                claude.display(),
                message,
                done.display()
            ),
        ));
    }
    Ok((
        if os == "macos" { "command" } else { "sh" },
        format!(
            "#!/bin/sh\ncd {} || exit 1\n{} {}\ntouch {}\necho\necho 'Claude Code closed. claude-sync now checks the pack; you can close this window.'\nsleep 3\n",
            sh_quote(&folder.to_string_lossy()),
            sh_quote(&claude.to_string_lossy()),
            sh_quote(message),
            sh_quote(&done.to_string_lossy())
        ),
    ))
}

/// The commands that might open `script`, best first; `has(name)`: a program is on `PATH`.
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
            let mut out = vec![];
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
        _ => [
            ("x-terminal-emulator", &["-e"][..]),
            ("gnome-terminal", &["--"]),
            ("konsole", &["-e"]),
            ("xfce4-terminal", &["-x"]),
            ("alacritty", &["-e"]),
            ("kitty", &[]),
            ("xterm", &["-e"]),
        ]
        .into_iter()
        .filter(|(p, _)| has(p))
        .map(|(p, a)| {
            let mut c = vec![p.to_string()];
            c.extend(a.iter().map(|x| x.to_string()));
            c.extend(["sh".to_string(), s.clone()]);
            c
        })
        .collect(),
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

pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Started,
    /// No terminal could be opened; `command` is what to run by hand.
    NoTerminal {
        command: String,
    },
}

/// Write the script under `dir` (named `stem`) and open the first terminal that starts.
pub fn start(
    dir: &Path,
    stem: &str,
    folder: &Path,
    claude: &Path,
    msg: &str,
    os: &str,
) -> std::io::Result<Outcome> {
    let manual = || Outcome::NoTerminal {
        command: manual_command(os, folder, claude, msg),
    };
    std::fs::create_dir_all(dir)?;
    let done = dir.join(format!("{stem}.done"));
    let _ = std::fs::remove_file(&done);
    let Ok((ext, body)) = script(os, folder, claude, msg, &done) else {
        return Ok(manual());
    };
    let file: PathBuf = dir.join(format!("{stem}.{ext}"));
    claude_sync_fs::atomic::write_atomic(&file, body.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))?;
    }
    for cmd in openers(os, &file, folder, &on_path) {
        let mut c = std::process::Command::new(&cmd[0]);
        claude_sync_fs::quiet(&mut c);
        c.args(&cmd[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Ok(mut child) = c.spawn() {
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
        let m = message("Pass\"Bridge; $(x)", "Group `x`");
        assert!(!m.contains(['"', ';', '$', '`', '\\', '&', '%']), "{m}");
        assert!(m.starts_with("Read setup.md in this folder"));
        assert!(m.contains("'Pass Bridge (x)'"), "{m}");
    }

    #[test]
    fn unix_scripts_quote_every_path_and_touch_the_marker() {
        let (ext, body) = script(
            "macos",
            Path::new("/home/a b/it's/x"),
            Path::new("/usr/bin/claude"),
            "Read setup.md",
            Path::new("/state/x.done"),
        )
        .unwrap();
        assert_eq!(ext, "command");
        assert!(
            body.contains(r"cd '/home/a b/it'\''s/x' || exit 1"),
            "{body}"
        );
        assert!(body.contains("touch '/state/x.done'"));
    }

    #[test]
    fn windows_refuses_a_path_cmd_cannot_quote() {
        let r = script(
            "windows",
            Path::new("C:\\a&b"),
            Path::new("C:\\c.exe"),
            "m",
            Path::new("C:\\d"),
        );
        assert!(r.is_err());
    }

    #[test]
    fn openers_follow_the_os_and_what_is_installed() {
        let f = Path::new("/s.sh");
        let d = Path::new("/d");
        assert_eq!(
            openers("macos", f, d, &|_| false)[0],
            ["open", "-a", "Terminal", "/s.sh"]
        );
        let l = openers("linux", f, d, &|p| p == "xterm" || p == "kitty");
        assert_eq!(l[0], ["kitty", "sh", "/s.sh"]);
        assert_eq!(l[1], ["xterm", "-e", "sh", "/s.sh"]);
        assert!(openers("linux", f, d, &|_| false).is_empty());
        assert_eq!(openers("windows", f, d, &|p| p == "wt.exe").len(), 2);
    }
}
