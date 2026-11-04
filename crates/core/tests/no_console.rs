//! No child process of the Windows app or CLI may open a console window (#178). A text scan:
//! every `Command::new(` must be followed, within a few lines, by `quiet(` (which sets
//! `CREATE_NO_WINDOW`), `creation_flags`, or a `run_command*` call (which calls `quiet`),
//! unless the site is on the unix- or macOS-only list below.
use std::path::{Path, PathBuf};

/// Lines after the spawn in which the flag must appear.
const WINDOW: usize = 12;

/// Spawns that never run on Windows: (file, program text on the spawn line; `None` = any).
const UNIX_ONLY: [(&str, Option<&str>); 3] = [
    // systemd and crontab.
    ("crates/cli/src/service.rs", None),
    // `open`, macOS.
    (
        "app/src-tauri/src/commands.rs",
        Some("Command::new(\"open\")"),
    ),
    // `ps`, unix (the Windows branch uses `tasklist` with `creation_flags`).
    ("app/src-tauri/src/cli.rs", Some("Command::new(\"ps\")")),
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "target" && name != "tests" && name != "node_modules" {
                rust_files(&p, out);
            }
        } else if name.ends_with(".rs") {
            out.push(p);
        }
    }
}

/// Spawns without a console flag in `text` (the file `rel`): `file:line` each.
fn offenders(rel: &str, text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut bad = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("Command::new(") || line.trim_start().starts_with("//") {
            continue;
        }
        let allowed = UNIX_ONLY
            .iter()
            .any(|(f, prog)| *f == rel && prog.is_none_or(|p| line.contains(p)));
        let end = (i + WINDOW + 1).min(lines.len());
        let flagged = lines[i..end].iter().any(|l| {
            [
                "quiet(",
                "creation_flags",
                "run_command(",
                "run_command_input(",
            ]
            .iter()
            .any(|m| l.contains(m))
        });
        if !allowed && !flagged {
            bad.push(format!("{rel}:{}", i + 1));
        }
    }
    bad
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn every_spawn_hides_its_console_window() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    rust_files(&root.join("app/src-tauri/src"), &mut files);
    assert!(files.len() > 20, "scan found only {} files", files.len());
    let mut bad = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        bad.extend(offenders(&rel, &std::fs::read_to_string(&f).unwrap()));
    }
    assert!(
        bad.is_empty(),
        "spawns without quiet()/creation_flags (a console window flashes on Windows): {bad:?}"
    );
}

#[test]
fn run_command_input_uses_quiet() {
    let text = std::fs::read_to_string(repo_root().join("crates/core/src/claude_cli.rs"))
        .unwrap()
        .replace("\r\n", "\n");
    let start = text
        .find("pub fn run_command_input(")
        .expect("run_command_input");
    let body = &text[start..];
    let body = &body[..body.find("\n}\n").unwrap()];
    assert!(
        body.contains("quiet(&mut cmd)"),
        "run_command_input must call quiet"
    );
}

#[test]
fn the_scan_catches_a_spawn_without_the_flag() {
    let bad = "fn f() {\n    let mut cmd = Command::new(\"x\");\n    cmd.output();\n}\n";
    assert_eq!(
        offenders("crates/core/src/x.rs", bad),
        ["crates/core/src/x.rs:2"]
    );
    let ok = format!("{bad}// pad\n").replace("cmd.output();", "quiet(&mut cmd).output();");
    assert!(offenders("crates/core/src/x.rs", &ok).is_empty());
}
