//! SSH between the user's own machines (architecture §21, byte formats as in 0.1.x): each
//! machine publishes `machines/<id>.json`; this pass writes the other machines into the
//! marked block of `~/.ssh/authorized_keys` (not on Windows), `~/.ssh/claude-sync.conf` and
//! the `Include` first line of `~/.ssh/config`. A peer's private `lan` address is a fallback
//! through `nc` when the Tailscale address does not answer. Everything lives under an
//! injected [`Home`]; the machine itself is asked through [`Probe`].
//!
//! v2-Parity rows covered: 46 (ssh peers), step-1 gap `machines/<id>.json`.
use crate::atomic::{read_file, sha256_hex, write_atomic};
use crate::disk::Home;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const BEGIN: &str = "# >>> claude-sync peers >>>";
pub const END: &str = "# <<< claude-sync peers <<<";
/// Tailscale's address ranges: a peer key works only from there (and its LAN block).
pub const FROM: &str = "100.64.0.0/10,fd7a:115c:a1e0::/48";
pub const CONF: &str = "claude-sync.conf";
const INCLUDE: &str = "Include claude-sync.conf";
const STATE_FILE: &str = "ssh-peers.json";
const KEY: &str = "id_ed25519";

type Result<T> = std::result::Result<T, String>;
fn e<T: std::fmt::Display>(x: T) -> String {
    x.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    Other,
}

/// What a machine is asked, so tests answer for it.
pub trait Probe {
    fn tailscale_ip(&self) -> Option<String>;
    /// An ed25519 key at `path` (and `path.pub`), no passphrase.
    fn keygen(&self, path: &Path) -> Result<()>;
    fn os(&self) -> Os;
    /// The private LAN IPv4 address, when the machine is on a LAN (not WSL, not Windows).
    fn lan_ip(&self) -> Option<String>;
    fn has_nc(&self) -> bool;
}

pub struct RealProbe;

impl Probe for RealProbe {
    fn tailscale_ip(&self) -> Option<String> {
        let o = std::process::Command::new("tailscale")
            .args(["ip", "-4"])
            .output()
            .ok()?;
        let t = String::from_utf8_lossy(&o.stdout);
        let ip = t.lines().next()?.trim();
        (o.status.success() && ip.parse::<std::net::Ipv4Addr>().is_ok()).then(|| ip.to_string())
    }
    fn keygen(&self, path: &Path) -> Result<()> {
        let mut c = std::process::Command::new("ssh-keygen");
        crate::quiet(&mut c);
        let o = c
            .args(["-t", "ed25519", "-N", "", "-q", "-f"])
            .arg(path)
            .output()
            .map_err(|x| format!("cannot run ssh-keygen: {x}"))?;
        o.status
            .success()
            .then_some(())
            .ok_or_else(|| "ssh-keygen failed".into())
    }
    fn os(&self) -> Os {
        match std::env::consts::OS {
            "macos" => Os::Mac,
            "windows" => Os::Windows,
            _ => Os::Other,
        }
    }
    fn lan_ip(&self) -> Option<String> {
        // connecting a UDP socket sends nothing; it only picks the outgoing interface
        let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        s.connect("192.0.2.1:9").ok()?;
        let wsl = std::env::var_os("WSL_DISTRO_NAME").is_some();
        lan_from(s.local_addr().ok().map(|a| a.ip()), wsl, self.os())
    }
    fn has_nc(&self) -> bool {
        std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("nc").is_file()))
    }
}

/// The private block (`10.0.0.0/8`, `172.16.0.0/12` or `192.168.0.0/16`) holding `ip`.
fn private_block(ip: &str) -> Option<&'static str> {
    let [a, b, ..] = ip.parse::<std::net::Ipv4Addr>().ok()?.octets();
    match (a, b) {
        (10, _) => Some("10.0.0.0/8"),
        (172, 16..=31) => Some("172.16.0.0/12"),
        (192, 168) => Some("192.168.0.0/16"),
        _ => None,
    }
}

/// A LAN address as written in a record: canonical IPv4 text in a private block.
fn valid_lan(s: &str) -> bool {
    s.parse::<std::net::Ipv4Addr>()
        .is_ok_and(|ip| ip.to_string() == s)
        && private_block(s).is_some()
}

/// The LAN address to publish from what the device reports: none on Windows or inside
/// WSL (its NAT address is not reachable from the LAN), nor for a non-private or IPv6 address.
fn lan_from(addr: Option<std::net::IpAddr>, wsl: bool, os: Os) -> Option<String> {
    if wsl || os == Os::Windows {
        return None;
    }
    let std::net::IpAddr::V4(ip) = addr? else {
        return None;
    };
    let s = ip.to_string();
    private_block(&s).map(|_| s)
}

/// `machines/<id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub name: String,
    pub alias: String,
    pub os: String,
    pub user: String,
    pub host: String,
    pub pubkey: String,
    pub ts: u64,
    /// The device's private LAN IPv4 address, used when Tailscale is down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan: Option<String>,
}

impl Record {
    /// The same record apart from its time.
    fn same_as(&self, o: &Record) -> bool {
        Record {
            ts: 0,
            ..self.clone()
        } == Record { ts: 0, ..o.clone() }
    }
}

/// `<state_dir>/ssh-peers.json`: this machine's id and its chosen alias.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Local {
    id: String,
    alias: Option<String>,
}

fn valid_alias(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_alphanumeric())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn valid_user(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn valid_host(s: &str) -> bool {
    (1..=255).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'))
}

/// `<type> <base64>` from one public-key line, or `None`. Only `ssh-ed25519` and
/// `ecdsa-sha2-*` keys, on a single line with no options in front.
pub fn parse_key(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.contains(['\n', '\r']) {
        return None;
    }
    let mut parts = line.split_whitespace();
    let (kind, key) = (parts.next()?, parts.next()?);
    let kind_ok = kind == "ssh-ed25519" || kind.starts_with("ecdsa-sha2-");
    let key_ok = key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='));
    (kind_ok && key_ok && kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then(|| format!("{kind} {key}"))
}

/// One `authorized_keys` line for a peer.
fn key_line(r: &Record, key: &str, lan_block: Option<&str>) -> String {
    let from = match lan_block {
        Some(b) => format!("{FROM},{b}"),
        None => FROM.to_string(),
    };
    format!(
        "from=\"{from}\",no-agent-forwarding,no-X11-forwarding {key} claude-sync:{}",
        r.alias
    )
}

/// The managed block for `lines` (no trailing newline), or `None` for no lines.
pub fn render_block(lines: &[String]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    Some(format!("{BEGIN}\n{}\n{END}", lines.join("\n")))
}

/// `existing` with the managed block replaced by `block` (removed for `None`). Lines
/// outside the block are kept as they are; a missing block is added at the end.
pub fn replace_block(existing: &str, block: Option<&str>) -> String {
    let eol = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let lines: Vec<&str> = existing.lines().collect();
    let begin = lines.iter().position(|l| l.trim_end() == BEGIN);
    let end = begin.and_then(|b| {
        lines[b..]
            .iter()
            .position(|l| l.trim_end() == END)
            .map(|e| e + b)
    });
    let mut out: Vec<String> = Vec::new();
    match (begin, end) {
        (Some(b), Some(e)) => {
            out.extend(lines[..b].iter().map(|l| l.to_string()));
            if let Some(block) = block {
                out.extend(block.lines().map(str::to_string));
            }
            out.extend(lines[e + 1..].iter().map(|l| l.to_string()));
        }
        _ => {
            out.extend(lines.iter().map(|l| l.to_string()));
            if let Some(block) = block {
                out.extend(block.lines().map(str::to_string));
            }
        }
    }
    if out.is_empty() {
        return String::new();
    }
    out.join(eol) + eol
}

/// `~/.ssh/config` with `Include claude-sync.conf` as its first line (once), or without it.
fn with_include(existing: &str, on: bool) -> String {
    let eol = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines: Vec<&str> = existing.lines().filter(|l| l.trim() != INCLUDE).collect();
    if on {
        lines.insert(0, INCLUDE);
    }
    if lines.iter().all(|l| l.trim().is_empty()) {
        return String::new();
    }
    lines.join(eol) + eol
}

/// The `Host` patterns the user's own config defines.
fn user_hosts(config: &str) -> Vec<String> {
    config
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let (kw, rest) = l.split_once(|c: char| c.is_whitespace() || c == '=')?;
            kw.eq_ignore_ascii_case("host").then_some(rest)
        })
        .flat_map(|rest| {
            rest.trim_start_matches(['=', ' ', '\t'])
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Where everything is: the home, this machine's name and user.
pub struct Env<'a> {
    pub home: &'a Home,
    pub now: i64,
    pub hostname: String,
    pub user: String,
}

impl Env<'_> {
    fn machines(&self) -> PathBuf {
        self.home.state.join("machines")
    }
}

fn load_local(env: &Env) -> Local {
    std::fs::read(env.home.state.join(STATE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_local(env: &Env, l: &Local) -> Result<()> {
    let mut b = serde_json::to_vec_pretty(l).map_err(e)?;
    b.push(b'\n');
    write_atomic(&env.home.state.join(STATE_FILE), &b).map_err(e)
}

/// This machine's id, made once.
fn machine_id(env: &Env) -> Result<String> {
    let mut l = load_local(env);
    if l.id.is_empty() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let seed = format!(
            "{nanos}:{}:{}",
            std::process::id(),
            env.home.state.display()
        );
        l.id = sha256_hex(seed.as_bytes())[..16].to_string();
        save_local(env, &l)?;
    }
    Ok(l.id)
}

/// Set the name other machines use for this one (`ssh <alias>`).
pub fn set_alias(env: &Env, alias: &str) -> Result<()> {
    if !valid_alias(alias) {
        return Err(format!(
            "`{alias}` is not a usable name: use letters, digits, `.`, `_` and `-`"
        ));
    }
    let mut l = load_local(env);
    l.alias = Some(alias.to_string());
    save_local(env, &l)
}

fn read_text(path: &Path) -> Result<Option<String>> {
    Ok(read_file(path)
        .map_err(e)?
        .map(|b| String::from_utf8_lossy(&b).into_owned()))
}

/// Write `text` to `path` when it differs (an empty text removes the file), with mode 0600.
fn put(path: &Path, text: &str) -> Result<()> {
    if read_text(path)?.unwrap_or_default() == text {
        return Ok(());
    }
    if text.is_empty() {
        return match std::fs::remove_file(path) {
            Err(x) if x.kind() != std::io::ErrorKind::NotFound => Err(e(x)),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        make_ssh_dir(dir)?;
    }
    write_atomic(path, text.as_bytes()).map_err(e)?;
    set_mode(path, 0o600);
    Ok(())
}

fn make_ssh_dir(dir: &Path) -> Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(e)?;
        set_mode(dir, 0o700);
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

/// The other machines' records in `machines/`, valid ones only.
fn read_records(env: &Env) -> Vec<Record> {
    let Ok(entries) = std::fs::read_dir(env.machines()) else {
        return Vec::new();
    };
    let mut out: Vec<Record> = entries
        .filter_map(|x| std::fs::read(x.ok()?.path()).ok())
        .filter_map(|b| serde_json::from_slice::<Record>(&b).ok())
        .collect();
    out.sort_by(|a, b| (&a.alias, &a.id).cmp(&(&b.alias, &b.id)));
    out
}

fn own_file(env: &Env, id: &str) -> PathBuf {
    env.machines().join(format!("{id}.json"))
}

/// Write this machine's record when it changed (turning the choice off removes it).
pub fn publish(env: &Env, probe: &dyn Probe, on: bool) -> Result<()> {
    if !on {
        let l = load_local(env);
        if !l.id.is_empty() {
            match std::fs::remove_file(own_file(env, &l.id)) {
                Err(x) if x.kind() != std::io::ErrorKind::NotFound => return Err(e(x)),
                _ => {}
            }
        }
        return Ok(());
    }
    let id = machine_id(env)?;
    let pub_path = env.home.ssh.join(format!("{KEY}.pub"));
    let pubkey = match read_text(&pub_path)? {
        Some(t) => t,
        None => {
            if env.home.ssh.join(KEY).exists() {
                return Err(format!(
                    "{KEY} exists without {KEY}.pub; nothing was changed"
                ));
            }
            make_ssh_dir(&env.home.ssh)?;
            probe.keygen(&env.home.ssh.join(KEY))?;
            read_text(&pub_path)?.ok_or_else(|| "ssh-keygen made no public key".to_string())?
        }
    };
    let pubkey =
        parse_key(&pubkey).ok_or_else(|| format!("{KEY}.pub is not an ed25519 or ecdsa key"))?;
    let os = probe.os();
    let base = load_local(env).alias.unwrap_or_else(|| {
        match os {
            Os::Mac => "mac",
            _ => "pc",
        }
        .to_string()
    });
    let taken: Vec<String> = read_records(env)
        .into_iter()
        .filter(|r| r.id < id)
        .map(|r| r.alias)
        .collect();
    let mut alias = base.clone();
    let mut n = 2;
    while taken.contains(&alias) {
        alias = format!("{base}-{n}");
        n += 1;
    }
    let rec = Record {
        id: id.clone(),
        name: env.hostname.clone(),
        alias,
        os: std::env::consts::OS.to_string(),
        user: env.user.clone(),
        host: probe.tailscale_ip().unwrap_or_else(|| env.hostname.clone()),
        pubkey,
        ts: u64::try_from(env.now).unwrap_or(0) * 1000,
        lan: probe.lan_ip().filter(|l| valid_lan(l)),
    };
    let file = own_file(env, &id);
    let unchanged = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice::<Record>(&b).ok())
        .is_some_and(|old| old.same_as(&rec));
    if !unchanged {
        let mut b = serde_json::to_vec_pretty(&rec).map_err(e)?;
        b.push(b'\n');
        write_atomic(&file, &b).map_err(e)?;
    }
    Ok(())
}

/// The peers worth adding: other machines whose fields are safe to write into ssh files.
fn peers(env: &Env, own_id: &str) -> Vec<(Record, String)> {
    read_records(env)
        .into_iter()
        .filter(|r| r.id != own_id && valid_alias(&r.alias) && valid_user(&r.user))
        .filter(|r| valid_host(&r.host))
        .filter_map(|mut r| {
            let key = parse_key(&r.pubkey)?;
            r.lan = r.lan.take().filter(|l| valid_lan(l));
            Some((r, key))
        })
        .collect()
}

/// The `Host` name per peer, in order: the alias (`<alias>-cs` when the user's own config
/// has it), made unique against `mine`, earlier peers and their `-lan` names.
fn host_names(peers: &[(Record, String)], mine: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut used: Vec<String> = mine.to_vec();
    for (r, _) in peers {
        let first = if mine.contains(&r.alias) {
            format!("{}-cs", r.alias)
        } else {
            r.alias.clone()
        };
        let mut name = first.clone();
        if used.contains(&name) {
            let short: String = r.id.chars().take(4).collect();
            name = format!("{first}-{short}");
            let base = name.clone();
            let mut n = 2;
            while used.contains(&name) {
                name = format!("{base}-{n}");
                n += 1;
            }
        }
        used.push(name.clone());
        used.push(format!("{name}-lan"));
        names.push(name);
    }
    names
}

/// Add the other machines to `authorized_keys` (not on Windows) and `claude-sync.conf`, or
/// remove everything this module wrote when `on` is false.
pub fn apply(env: &Env, probe: &dyn Probe, on: bool) -> Result<()> {
    let own_id = load_local(env).id;
    let list = if on { peers(env, &own_id) } else { Vec::new() };
    let windows = probe.os() == Os::Windows;

    if !windows {
        let path = env.home.ssh.join("authorized_keys");
        let lan_block = probe.lan_ip().as_deref().and_then(private_block);
        let lines: Vec<String> = list
            .iter()
            .map(|(r, k)| key_line(r, k, lan_block))
            .collect();
        let old = read_text(&path)?;
        if old.is_some() || !lines.is_empty() {
            let new = replace_block(
                old.as_deref().unwrap_or(""),
                render_block(&lines).as_deref(),
            );
            put(&path, &new)?;
        }
    }

    let config_path = env.home.ssh.join("config");
    let config = read_text(&config_path)?;
    let conf_path = env.home.ssh.join(CONF);
    if on {
        let mine = user_hosts(config.as_deref().unwrap_or(""));
        let mut conf = String::new();
        let names = host_names(&list, &mine);
        let lan_ok = !windows && probe.has_nc();
        for ((r, _), name) in list.iter().zip(&names) {
            conf.push_str(&format!(
                "Host {name}\n  HostName {}\n  User {}\n",
                r.host, r.user
            ));
            let Some(lan) = r.lan.as_deref().filter(|_| lan_ok) else {
                continue;
            };
            let host = &r.host;
            conf.push_str(&format!(
                "  HostKeyAlias {host}\n  ProxyCommand sh -c 'nc -z -w 2 %h %p 2>/dev/null && exec nc %h %p; nc -z -w 2 {lan} %p 2>/dev/null && exec nc {lan} %p; exit 1'\n"
            ));
            let lan_name = format!("{name}-lan");
            if !mine.contains(&lan_name) && !names.contains(&lan_name) {
                conf.push_str(&format!(
                    "Host {lan_name}\n  HostName {lan}\n  User {}\n  HostKeyAlias {host}\n",
                    r.user
                ));
            }
        }
        put(&conf_path, &conf)?;
        // Even with no peers yet the file exists (empty) so the Include never dangles.
        if conf.is_empty() {
            put(&conf_path, "\n")?;
        }
    } else {
        put(&conf_path, "")?;
    }
    if config.is_some() || on {
        let new = with_include(config.as_deref().unwrap_or(""), on);
        put(&config_path, &new)?;
    }
    Ok(())
}

/// One pass: publish this machine, then apply the others.
pub fn pass(env: &Env, probe: &dyn Probe, on: bool) -> Result<()> {
    publish(env, probe, on)?;
    apply(env, probe, on)
}
