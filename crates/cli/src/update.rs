//! `update`: ask the relay (the same check the app uses) whether a newer release exists,
//! then download this program's archive from the public GitHub release, check its SHA-256
//! and replace the running binary.
use crate::auth::{self, Standing};
use crate::config;
use claude_sync_core::{github, Paths};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const RELEASES: &str = "https://github.com/MehdiMamas/claude-sync/releases/download";

/// The Rust target triple this build was made for, as the release archives are named.
pub fn target_triple() -> Result<&'static str, String> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        (os, arch) => return Err(format!("no release of the command line for {os} {arch}")),
    })
}

pub fn asset_name(triple: &str) -> String {
    if triple.contains("windows") {
        format!("claude-sync-cli-{triple}.zip")
    } else {
        format!("claude-sync-cli-{triple}.tar.gz")
    }
}

/// The relay's `target` and `arch` words.
fn relay_target() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        o => o,
    };
    (os, std::env::consts::ARCH)
}

/// The version in the relay's answer. 204 (and any non-200) is "up to date".
pub fn newer_version(status: u16, body: &str) -> Result<Option<String>, String> {
    match status {
        204 => Ok(None),
        200 => {
            let v: serde_json::Value = serde_json::from_str(body)
                .map_err(|e| format!("the relay's answer is not JSON: {e}"))?;
            Ok(v.get("version")
                .and_then(|v| v.as_str())
                .map(|s| s.trim_start_matches('v').to_string()))
        }
        s => Err(format!("the update check answered {s}")),
    }
}

/// `sha256sum` output (`<hex>  <name>`) or a bare hash, against `bytes`.
pub fn checksum_matches(bytes: &[u8], listed: &str) -> bool {
    let want = listed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    let got: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    want.len() == 64 && want == got
}

pub async fn check(paths: &Paths) -> Result<Option<String>, String> {
    if !config::relay_configured() {
        return Err(
            "this build has no relay URL (CLAUDE_SYNC_RELAY_URL), so updates are off".into(),
        );
    }
    let Standing::In(signed) = auth::standing(paths)? else {
        return Err("sign in first: the update check needs your GitHub sign-in".into());
    };
    let token = signed.session.token().await.map_err(|e| e.to_string())?;
    let (target, arch) = relay_target();
    let res = github::http_client()
        .get(config::relay(&format!(
            "/api/update?target={target}&arch={arch}&current_version={}",
            config::VERSION
        )))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("update check failed: {e}"))?;
    let status = res.status().as_u16();
    let body = res.text().await.unwrap_or_default();
    newer_version(status, &body)
}

async fn download(url: &str) -> Result<Vec<u8>, String> {
    let res = github::http_client()
        .get(url)
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("{url} answered {}", res.status()));
    }
    res.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("download failed: {e}"))
}

/// Download version `version`, verify it and put it in place of `exe`.
pub async fn install(version: &str, exe: &Path) -> Result<(), String> {
    let name = asset_name(target_triple()?);
    let base = format!("{RELEASES}/v{version}/{name}");
    let archive = download(&base).await?;
    let listed = download(&format!("{base}.sha256")).await?;
    if !checksum_matches(&archive, &String::from_utf8_lossy(&listed)) {
        return Err(format!(
            "{name} does not match its checksum; nothing was changed"
        ));
    }
    let work = tempdir(exe)?;
    let result = replace(&archive, &name, &work, exe);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn tempdir(exe: &Path) -> Result<PathBuf, String> {
    let dir = exe
        .parent()
        .ok_or("cannot find the folder of this program")?
        .join(format!(".claude-sync-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot write next to {}: {e}", exe.display()))?;
    Ok(dir)
}

fn replace(archive: &[u8], name: &str, work: &Path, exe: &Path) -> Result<(), String> {
    let file = work.join(name);
    std::fs::write(&file, archive).map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new("tar");
    cmd.arg("-xf").arg(&file).arg("-C").arg(work);
    let status = claude_sync_core::claude_cli::quiet(&mut cmd)
        .status()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    if !status.success() {
        return Err("tar could not open the archive; nothing was changed".into());
    }
    let bin = if cfg!(windows) {
        "claude-sync-cli.exe"
    } else {
        "claude-sync-cli"
    };
    let new = work.join(bin);
    if !new.is_file() {
        return Err(format!("the archive has no {bin}; nothing was changed"));
    }
    // Rename over the running program: fine on Unix; Windows needs the old one moved aside.
    #[cfg(windows)]
    {
        let old = exe.with_extension("old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).map_err(|e| e.to_string())?;
        if let Err(e) = std::fs::rename(&new, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(e.to_string());
        }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let staged = exe.with_extension("new");
        std::fs::copy(&new, &staged).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
        std::fs::rename(&staged, exe).map_err(|e| {
            let _ = std::fs::remove_file(&staged);
            format!("cannot replace {}: {e}", exe.display())
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_relay_answer() {
        assert_eq!(newer_version(204, ""), Ok(None));
        assert_eq!(
            newer_version(200, r#"{"version":"0.2.0","url":"x","signature":"y"}"#),
            Ok(Some("0.2.0".into()))
        );
        assert_eq!(
            newer_version(200, r#"{"version":"v0.2.0"}"#),
            Ok(Some("0.2.0".into()))
        );
        assert!(newer_version(401, "").unwrap_err().contains("401"));
        assert!(newer_version(200, "nope").is_err());
    }

    #[test]
    fn archives_are_named_by_target() {
        assert_eq!(
            asset_name("x86_64-unknown-linux-gnu"),
            "claude-sync-cli-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            asset_name("x86_64-pc-windows-msvc"),
            "claude-sync-cli-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn the_checksum_must_match() {
        // sha256("abc")
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(checksum_matches(
            b"abc",
            &format!("{abc}  claude-sync-cli.tar.gz\n")
        ));
        assert!(checksum_matches(b"abc", &abc.to_uppercase()));
        assert!(!checksum_matches(b"abd", abc));
        assert!(!checksum_matches(b"abc", "ba78"));
        assert!(!checksum_matches(b"abc", ""));
    }
}
