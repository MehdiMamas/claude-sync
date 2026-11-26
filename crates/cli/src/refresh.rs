//! After a command changed what the window shows (Parity 62): ask the running app to reload.
//! Launching the app binary with `--refresh` reaches its single-instance handler; with no app
//! running it would start one, so check first.
use std::path::Path;
use sysinfo::{ProcessesToUpdate, System};

/// The app has no command word: it runs bare, from autostart, or on a refresh.
fn is_app_args(args: &[std::ffi::OsString]) -> bool {
    args.iter().skip(1).all(|a| {
        let a = a.to_string_lossy();
        a == "--background" || a == "--refresh" || a.starts_with("-psn_")
    })
}

/// Another process runs `exe` as the app (not as a command or a job).
fn app_running(exe: &Path) -> bool {
    let Ok(mut sys) = System::new() else {
        return false;
    };
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let me = sysinfo::Pid::from_u32(std::process::id());
    sys.processes()
        .iter()
        .any(|(pid, p)| *pid != me && p.exe() == Some(exe) && is_app_args(p.cmd()))
}

/// Run `exe --refresh` if `running` says the app is up; true when it was started.
fn refresh_with(exe: &Path, running: impl Fn(&Path) -> bool) -> bool {
    if !running(exe) {
        return false;
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--refresh");
    claude_sync_fs::quiet(&mut cmd).spawn().is_ok()
}

pub fn refresh_running_app(exe: &Path) {
    refresh_with(exe, app_running);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A program that records its arguments, standing in for the app binary.
    fn fake_app(dir: &Path) -> std::path::PathBuf {
        let exe = dir.join("app");
        std::fs::write(
            &exe,
            "#!/bin/sh\necho \"$@\" > \"$(dirname \"$0\")/args\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    #[test]
    fn refresh_is_sent_only_to_a_running_app() {
        let d = tempfile::tempdir().unwrap();
        let exe = fake_app(d.path());
        assert!(!refresh_with(&exe, |_| false));
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!d.path().join("args").exists(), "no app: nothing launched");
        assert!(refresh_with(&exe, |_| true));
        for _ in 0..50 {
            if d.path().join("args").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let args = std::fs::read_to_string(d.path().join("args")).unwrap();
        assert_eq!(args.trim(), "--refresh");
    }

    #[test]
    fn only_a_bare_launch_is_the_app() {
        let v = |a: &[&str]| a.iter().map(Into::into).collect::<Vec<_>>();
        assert!(is_app_args(&v(&["claude-sync"])));
        assert!(is_app_args(&v(&["claude-sync", "--background"])));
        assert!(!is_app_args(&v(&["claude-sync", "personal", "enable"])));
        assert!(!is_app_args(&v(&["claude-sync", "run-job", "a/b", "x"])));
    }
}
