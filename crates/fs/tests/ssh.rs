use claude_sync_fs::ssh::{pass, Env, Os, Probe, Record};
use claude_sync_fs::Home;
use std::path::Path;

struct P {
    os: Os,
    nc: bool,
}
impl Probe for P {
    fn tailscale_ip(&self) -> Option<String> {
        Some("100.64.0.9".into())
    }
    fn keygen(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, "private").map_err(|e| e.to_string())?;
        std::fs::write(path.with_extension("pub"), "ssh-ed25519 AAAAmine me@box\n")
            .map_err(|e| e.to_string())
    }
    fn os(&self) -> Os {
        self.os
    }
    fn lan_ip(&self) -> Option<String> {
        Some("192.168.1.5".into())
    }
    fn has_nc(&self) -> bool {
        self.nc
    }
}

fn peer(h: &Home, lan: Option<&str>) {
    let r = Record {
        id: "peer0001".into(),
        name: "other".into(),
        alias: "laptop".into(),
        os: "linux".into(),
        user: "sam".into(),
        host: "100.64.0.7".into(),
        pubkey: "ssh-ed25519 AAAApeer".into(),
        ts: 1,
        lan: lan.map(String::from),
    };
    let d = h.state.join("machines");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("peer0001.json"), serde_json::to_vec(&r).unwrap()).unwrap();
}

#[test]
fn peers_are_written_into_the_marked_block_conf_and_include_and_removed_again() {
    let t = tempfile::tempdir().unwrap();
    let h = Home::under(t.path());
    let env = Env {
        home: &h,
        now: 5,
        hostname: "box".into(),
        user: "me".into(),
    };
    std::fs::create_dir_all(&h.ssh).unwrap();
    std::fs::write(h.ssh.join("authorized_keys"), "ssh-rsa KEEP mine\n").unwrap();
    std::fs::write(h.ssh.join("config"), "Host work\n  User me\n").unwrap();
    peer(&h, Some("192.168.1.20"));

    pass(
        &env,
        &P {
            os: Os::Other,
            nc: true,
        },
        true,
    )
    .unwrap();
    let ak = std::fs::read_to_string(h.ssh.join("authorized_keys")).unwrap();
    assert!(ak.starts_with("ssh-rsa KEEP mine\n# >>> claude-sync peers >>>\n"));
    assert!(ak.contains("from=\"100.64.0.0/10,fd7a:115c:a1e0::/48,192.168.0.0/16\",no-agent-forwarding,no-X11-forwarding ssh-ed25519 AAAApeer claude-sync:laptop"));
    let conf = std::fs::read_to_string(h.ssh.join("claude-sync.conf")).unwrap();
    assert!(conf.contains("Host laptop\n  HostName 100.64.0.7\n  User sam\n  HostKeyAlias 100.64.0.7\n  ProxyCommand sh -c 'nc -z -w 2 %h %p"));
    assert!(conf.contains("Host laptop-lan\n  HostName 192.168.1.20\n"));
    let cfg = std::fs::read_to_string(h.ssh.join("config")).unwrap();
    assert!(cfg.starts_with("Include claude-sync.conf\nHost work\n"));
    // my own record is published, with the lan address, and the key was made once
    let id = std::fs::read_dir(h.state.join("machines"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n != "peer0001.json")
        .unwrap();
    let mine: Record =
        serde_json::from_slice(&std::fs::read(h.state.join("machines").join(id)).unwrap()).unwrap();
    assert_eq!(
        (
            mine.host.as_str(),
            mine.lan.as_deref(),
            mine.pubkey.as_str()
        ),
        (
            "100.64.0.7".replace("7", "9").as_str(),
            Some("192.168.1.5"),
            "ssh-ed25519 AAAAmine"
        )
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(h.ssh.join("claude-sync.conf"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    // no nc: no lan fallback; same run twice changes nothing
    pass(
        &env,
        &P {
            os: Os::Other,
            nc: false,
        },
        true,
    )
    .unwrap();
    assert!(!std::fs::read_to_string(h.ssh.join("claude-sync.conf"))
        .unwrap()
        .contains("ProxyCommand"));
    // Windows writes no authorized_keys block
    let before = std::fs::read_to_string(h.ssh.join("authorized_keys")).unwrap();
    pass(
        &env,
        &P {
            os: Os::Windows,
            nc: true,
        },
        true,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(h.ssh.join("authorized_keys")).unwrap(),
        before
    );
    // off: everything this wrote is gone, the user's lines stay
    pass(
        &env,
        &P {
            os: Os::Other,
            nc: true,
        },
        false,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(h.ssh.join("authorized_keys")).unwrap(),
        "ssh-rsa KEEP mine\n"
    );
    assert!(!h.ssh.join("claude-sync.conf").exists());
    assert_eq!(
        std::fs::read_to_string(h.ssh.join("config")).unwrap(),
        "Host work\n  User me\n"
    );
}

#[test]
fn a_hostile_record_is_ignored() {
    let t = tempfile::tempdir().unwrap();
    let h = Home::under(t.path());
    let env = Env {
        home: &h,
        now: 5,
        hostname: "box".into(),
        user: "me".into(),
    };
    let d = h.state.join("machines");
    std::fs::create_dir_all(&d).unwrap();
    let bad = r#"{"id":"x","name":"n","alias":"a\nHost evil","os":"linux","user":"u","host":"h","pubkey":"ssh-ed25519 AAAA","ts":1}"#;
    std::fs::write(d.join("x.json"), bad).unwrap();
    pass(
        &env,
        &P {
            os: Os::Other,
            nc: true,
        },
        true,
    )
    .unwrap();
    let conf = std::fs::read_to_string(h.ssh.join("claude-sync.conf")).unwrap();
    assert!(!conf.contains("evil"));
}

fn setup(files: &[(&str, &[u8])]) -> (tempfile::TempDir, Home) {
    let t = tempfile::tempdir().unwrap();
    let h = Home::under(t.path());
    std::fs::create_dir_all(&h.ssh).unwrap();
    for (n, b) in files {
        std::fs::write(h.ssh.join(n), b).unwrap();
    }
    peer(&h, None);
    (t, h)
}

fn run(h: &Home, on: bool) -> Result<(), String> {
    let env = Env {
        home: h,
        now: 5,
        hostname: "box".into(),
        user: "me".into(),
    };
    pass(
        &env,
        &P {
            os: Os::Other,
            nc: false,
        },
        on,
    )
}

fn backup_of(h: &Home, name: &str) -> Option<Vec<u8>> {
    let d = std::fs::read_dir(h.state.join("backups"))
        .ok()?
        .next()?
        .ok()?;
    std::fs::read(d.path().join("ssh").join(name)).ok()
}

#[test]
fn ssh_files_are_backed_up_before_each_rewrite() {
    let (_t, h) = setup(&[
        ("authorized_keys", b"ssh-rsa KEEP mine\n"),
        ("config", b"Host work\n"),
    ]);
    run(&h, true).unwrap();
    assert_eq!(
        backup_of(&h, "authorized_keys").unwrap(),
        b"ssh-rsa KEEP mine\n"
    );
    assert_eq!(backup_of(&h, "config").unwrap(), b"Host work\n");
    run(&h, true).unwrap(); // a pass that changes nothing backs up nothing new
                            // the conf is rewritten (peer gone) and the old one kept
    std::fs::remove_file(h.state.join("machines/peer0001.json")).unwrap();
    run(&h, true).unwrap();
    assert!(
        String::from_utf8(backup_of(&h, "claude-sync.conf").unwrap())
            .unwrap()
            .contains("Host laptop")
    );
}

#[test]
fn a_file_that_is_not_utf8_is_never_rewritten_and_is_reported() {
    let bad: &[u8] = b"ssh-rsa KEEP \xff\xfe\n";
    let (_t, h) = setup(&[("authorized_keys", bad)]);
    let err = run(&h, true).unwrap_err();
    assert!(
        err.contains("authorized_keys") && err.contains("not valid UTF-8"),
        "{err}"
    );
    assert_eq!(std::fs::read(h.ssh.join("authorized_keys")).unwrap(), bad);
    // the other files still went through
    assert!(h.ssh.join("claude-sync.conf").exists());
}

#[test]
fn a_begin_marker_with_no_end_leaves_the_file_alone_and_is_reported() {
    let text = b"ssh-rsa KEEP mine\n# >>> claude-sync peers >>>\nssh-rsa ALSO mine\n";
    let (_t, h) = setup(&[("authorized_keys", text)]);
    for on in [true, false] {
        let err = run(&h, on).unwrap_err();
        assert!(err.contains("no `# <<< claude-sync peers <<<`"), "{err}");
        assert_eq!(std::fs::read(h.ssh.join("authorized_keys")).unwrap(), text);
    }
}

#[test]
fn a_config_with_the_users_lines_or_only_blanks_is_never_deleted() {
    let (_t, h) = setup(&[("config", b"Host work\n  User me\n")]);
    run(&h, true).unwrap();
    run(&h, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(h.ssh.join("config")).unwrap(),
        "Host work\n  User me\n"
    );
    let (_t2, h2) = setup(&[("config", b"\n  \n")]);
    run(&h2, true).unwrap();
    run(&h2, false).unwrap();
    assert_eq!(std::fs::read(h2.ssh.join("config")).unwrap(), b"");
}
