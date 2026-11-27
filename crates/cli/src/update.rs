//! `update`: download this program's archive from the release `latest.json` names, check its
//! SHA-256 against the `.sha256` file and its minisign signature (`.sig`, the updater key the
//! app also trusts), then swap the running binary.
use claude_sync_github::Client;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

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
    let ext = if triple.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("claude-sync-cli-{triple}.{ext}")
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

/// The updater public key of `tauri.conf.json`: the one key v1 clients trust.
pub fn updater_pubkey() -> Result<String, String> {
    let conf: serde_json::Value =
        serde_json::from_str(include_str!("../../../app/src-tauri/tauri.conf.json"))
            .map_err(|e| e.to_string())?;
    conf["plugins"]["updater"]["pubkey"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "no updater key in tauri.conf.json".into())
}

fn b64(s: &str) -> Result<String, String> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| e.to_string())?;
    String::from_utf8(raw).map_err(|e| e.to_string())
}

/// `sig` (the `.sig` file: base64 of a minisign signature, as Tauri writes it) over `bytes`,
/// against `pubkey` (base64 of the minisign public key file, as in `tauri.conf.json`).
pub fn signature_ok(bytes: &[u8], sig: &str, pubkey: &str) -> bool {
    let check = || -> Result<(), String> {
        let key = minisign_verify::PublicKey::decode(&b64(pubkey)?).map_err(|e| e.to_string())?;
        let sig = minisign_verify::Signature::decode(&b64(sig)?).map_err(|e| e.to_string())?;
        key.verify(bytes, &sig, false).map_err(|e| e.to_string())
    };
    check().is_ok()
}

/// Download `version`'s archive for `name`, verify it and put it in place of `exe`. Nothing is
/// changed unless the checksum and the signature (against `pubkey`) both match.
pub fn install(
    client: &Client,
    version: &str,
    name: &str,
    exe: &Path,
    pubkey: &str,
) -> Result<(), String> {
    let archive = client
        .release_asset(version, name)
        .map_err(|e| e.to_string())?;
    let listed = client
        .release_asset(version, &format!("{name}.sha256"))
        .map_err(|e| e.to_string())?;
    if !checksum_matches(&archive, &String::from_utf8_lossy(&listed)) {
        return Err(format!(
            "{name} does not match its checksum; nothing was changed"
        ));
    }
    let sig = client
        .release_asset(version, &format!("{name}.sig"))
        .map_err(|_| format!("{name} has no signature; nothing was changed"))?;
    if !signature_ok(&archive, &String::from_utf8_lossy(&sig), pubkey) {
        return Err(format!(
            "{name} does not match its signature; nothing was changed"
        ));
    }
    let work = workdir(exe)?;
    let result = replace(&archive, name, &work, exe);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn workdir(exe: &Path) -> Result<PathBuf, String> {
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
    // `tar` reads .tar.gz everywhere and .zip on Windows (bsdtar), as v1 relied on.
    let mut cmd = std::process::Command::new("tar");
    cmd.arg("-xf").arg(&file).arg("-C").arg(work);
    claude_sync_fs::quiet(&mut cmd);
    let status = cmd.status().map_err(|e| format!("cannot run tar: {e}"))?;
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
    fn archives_are_named_by_target() {
        assert_eq!(
            asset_name("aarch64-unknown-linux-gnu"),
            "claude-sync-cli-aarch64-unknown-linux-gnu.tar.gz"
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
        assert!(checksum_matches(b"abc", &format!("{abc}  x.tar.gz\n")));
        assert!(checksum_matches(b"abc", &abc.to_uppercase()));
        assert!(!checksum_matches(b"abd", abc));
        assert!(!checksum_matches(b"abc", "ba78"));
        assert!(!checksum_matches(b"abc", ""));
    }

    #[cfg(unix)]
    use claude_sync_github::Config;
    #[cfg(unix)]
    use wiremock::matchers::{method, path};
    #[cfg(unix)]
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A `.tar.gz` holding a `claude-sync-cli` file with `body`, made with the system `tar`.
    #[cfg(unix)]
    fn archive(dir: &Path, body: &str) -> Vec<u8> {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("claude-sync-cli"), body).unwrap();
        let out = dir.join("a.tar.gz");
        let mut tar = std::process::Command::new("tar");
        claude_sync_fs::quiet(&mut tar);
        let st = tar
            .arg("-czf")
            .arg(&out)
            .arg("-C")
            .arg(&src)
            .arg("claude-sync-cli")
            .status()
            .unwrap();
        assert!(st.success());
        std::fs::read(out).unwrap()
    }

    #[cfg(unix)]
    async fn serve(bytes: Vec<u8>, listed: String, sig: Option<String>) -> (MockServer, Config) {
        let server = MockServer::start().await;
        let name = "claude-sync-cli-test.tar.gz";
        let files = [
            (String::new(), Some(bytes)),
            (".sha256".into(), Some(listed.into_bytes())),
            (".sig".into(), sig.map(String::into_bytes)),
        ];
        for (ext, body) in files {
            if let Some(body) = body {
                Mock::given(method("GET"))
                    .and(path(format!("/download/v9.9.9/{name}{ext}")))
                    .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
                    .mount(&server)
                    .await;
            }
        }
        let cfg = Config {
            releases: server.uri(),
            ..Config::github()
        };
        (server, cfg)
    }

    /// A fresh key: its public key as `tauri.conf.json` holds it, and a signer in the format of
    /// a Tauri `.sig` file.
    #[cfg(unix)]
    fn key() -> (String, impl Fn(&[u8]) -> String) {
        use base64::Engine;
        let enc = |s: String| base64::engine::general_purpose::STANDARD.encode(s);
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pubkey = enc(kp.pk.to_box().unwrap().to_string());
        let sign = move |data: &[u8]| {
            let sig = minisign::sign(Some(&kp.pk), &kp.sk, std::io::Cursor::new(data), None, None)
                .unwrap();
            enc(sig.to_string())
        };
        (pubkey, sign)
    }

    #[cfg(unix)]
    fn hash(b: &[u8]) -> String {
        Sha256::digest(b)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect()
    }

    #[cfg(unix)]
    async fn run(cfg: Config, exe: PathBuf, pubkey: String) -> Result<(), String> {
        tokio::task::spawn_blocking(move || {
            install(
                &Client::new(cfg).unwrap(),
                "9.9.9",
                "claude-sync-cli-test.tar.gz",
                &exe,
                &pubkey,
            )
        })
        .await
        .unwrap()
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_good_signature_installs_and_a_wrong_hash_changes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let bytes = archive(d.path(), "NEW");
        let exe = d.path().join("claude-sync-cli");
        std::fs::write(&exe, "OLD").unwrap();
        let name = "claude-sync-cli-test.tar.gz";
        let (pubkey, sign) = key();

        let (_s, cfg) = serve(
            bytes.clone(),
            format!("{}  {name}\n", hash(b"other")),
            Some(sign(&bytes)),
        )
        .await;
        let err = run(cfg, exe.clone(), pubkey.clone()).await.unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "OLD");

        let (_s, cfg) = serve(
            bytes.clone(),
            format!("{}  {name}\n", hash(&bytes)),
            Some(sign(&bytes)),
        )
        .await;
        run(cfg, exe.clone(), pubkey).await.unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "NEW");
        let left: Vec<_> = std::fs::read_dir(d.path()).unwrap().flatten().collect();
        assert!(left.iter().all(|e| !e
            .file_name()
            .to_string_lossy()
            .starts_with(".claude-sync-update")));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_tampered_archive_or_a_missing_or_foreign_signature_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let good = archive(d.path(), "NEW");
        let evil = archive(&d.path().join("evil"), "EVIL");
        let exe = d.path().join("claude-sync-cli");
        std::fs::write(&exe, "OLD").unwrap();
        let name = "claude-sync-cli-test.tar.gz";
        let (pubkey, sign) = key();
        let (_, other_sign) = key();
        // the checksum file matches what is served, so only the signature can stop these
        for (served, sig, want) in [
            (&evil, Some(sign(&good)), "signature"),
            (&good, None, "no signature"),
            (&good, Some(other_sign(&good)), "signature"),
            (&good, Some("not base64!".to_string()), "signature"),
        ] {
            let (_s, cfg) = serve(served.clone(), format!("{}  {name}\n", hash(served)), sig).await;
            let err = run(cfg, exe.clone(), pubkey.clone()).await.unwrap_err();
            assert!(err.contains(want), "{err}");
            assert_eq!(std::fs::read_to_string(&exe).unwrap(), "OLD");
        }
    }
}
