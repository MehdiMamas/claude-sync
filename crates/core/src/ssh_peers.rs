//! Reach your own machines over SSH (docs/architecture.md §21). With the choice on
//! (`choices::SshPeersChoice`), each device publishes a record of itself to
//! `<state_dir>/machines/<id>.json`, which personal sync carries to the account's other
//! devices (`machines/<id>.json` in the personal repo). Every device then adds the other
//! devices' public keys to `~/.ssh/authorized_keys` (one managed block) and their names to
//! `~/.ssh/claude-sync.conf`, which `~/.ssh/config` includes on its first line.
//!
//! Everything is user scope. Paths and the outside commands (`tailscale`, `ssh-keygen`)
//! are injected ([`Env`], [`Probe`]) so tests never touch the real `~/.ssh`.
use crate::apply::sha256_hex;
use crate::claude_cli;
use crate::state::{now_unix, write_atomic};
use crate::{Error, Paths, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const BEGIN: &str = "# >>> claude-sync peers >>>";
pub const END: &str = "# <<< claude-sync peers <<<";
/// Keys from these ranges only: Tailscale's IPv4 and IPv6 addresses.
pub const FROM: &str = "100.64.0.0/10,fd7a:115c:a1e0::/48";
pub const CONF: &str = "claude-sync.conf";
const INCLUDE: &str = "Include claude-sync.conf";
const STATE_FILE: &str = "ssh-peers.json";
const KEY: &str = "id_ed25519";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    Other,
}

/// What a device would otherwise ask the outside world.
pub trait Probe {
    /// This device's Tailscale IPv4 address when Tailscale runs.
    fn tailscale_ip(&self) -> Option<String>;
    /// Create an ed25519 key at `path` (and `path.pub`) without a passphrase.
    fn keygen(&self, path: &Path) -> Result<()>;
    fn os(&self) -> Os;
    /// This device's private LAN IPv4 address, when it is reachable from the LAN.
    fn lan_ip(&self) -> Option<String>;
    /// Whether an executable `nc` is on PATH.
    fn has_nc(&self) -> bool;
}

pub struct RealProbe;

/// How long the `tailscale ip` answer (also "none") is reused.
const TAILSCALE_TTL: Duration = Duration::from_secs(600);

/// The last `tailscale ip` answer and when it was fetched.
type TsSlot = Option<(Instant, Option<String>)>;

/// The slot's answer when it is younger than `ttl`, else `fetch()`, stored with `now`.
fn cached_ip(
    slot: &Mutex<TsSlot>,
    now: Instant,
    ttl: Duration,
    fetch: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, value)) = guard.as_ref() {
        if now.saturating_duration_since(*at) < ttl {
            return value.clone();
        }
    }
    let value = fetch();
    *guard = Some((now, value.clone()));
    value
}

/// Where to look for the Tailscale CLI, in order. A macOS GUI app gets launchd's PATH,
/// and the Tailscale app ships its CLI only inside the bundle.
fn ts_candidates(os: Os) -> Vec<&'static str> {
    let mut v = vec!["tailscale"];
    match os {
        Os::Mac => v.push("/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
        Os::Windows => v.push(r"C:\Program Files\Tailscale\tailscale.exe"),
        Os::Other => {}
    }
    v
}

/// The first line of `tailscale ip -4` when it is an IPv4 address.
fn parse_ts_ip(stdout: &[u8]) -> Option<String> {
    let first = String::from_utf8_lossy(stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    first.parse::<std::net::Ipv4Addr>().ok()?;
    Some(first)
}

fn fetch_tailscale_ip() -> Option<String> {
    ts_candidates(RealProbe.os()).into_iter().find_map(|bin| {
        let mut cmd = std::process::Command::new(bin);
        cmd.args(["ip", "-4"]);
        let out = claude_cli::quiet(&mut cmd)
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        parse_ts_ip(&out.stdout)
    })
}

impl Probe for RealProbe {
    fn tailscale_ip(&self) -> Option<String> {
        static SLOT: OnceLock<Mutex<TsSlot>> = OnceLock::new();
        let slot = SLOT.get_or_init(|| Mutex::new(None));
        cached_ip(slot, Instant::now(), TAILSCALE_TTL, fetch_tailscale_ip)
    }

    fn keygen(&self, path: &Path) -> Result<()> {
        let mut cmd = std::process::Command::new("ssh-keygen");
        cmd.args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(path)
            .stdin(std::process::Stdio::null());
        let status = claude_cli::quiet(&mut cmd)
            .status()
            .map_err(|e| Error::Invalid(format!("could not run ssh-keygen: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Invalid("ssh-keygen failed".into()))
        }
    }

    fn os(&self) -> Os {
        match std::env::consts::OS {
            "macos" => Os::Mac,
            "windows" => Os::Windows,
            _ => Os::Other,
        }
    }

    fn lan_ip(&self) -> Option<String> {
        // Connecting a UDP socket sends no packet; it only picks the outgoing address.
        let addr = std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|s| s.connect("1.1.1.1:53").and_then(|()| s.local_addr()))
            .ok()
            .map(|a| a.ip());
        let wsl = std::env::var_os("WSL_DISTRO_NAME").is_some()
            || Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists();
        lan_from(addr, wsl, self.os())
    }

    fn has_nc(&self) -> bool {
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path).any(|d| is_executable(&d.join("nc")))
    }
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
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

/// Where everything is. Tests point all of it into a temp folder.
#[derive(Debug, Clone)]
pub struct Env {
    pub ssh_dir: PathBuf,
    pub machines_dir: PathBuf,
    pub state_dir: PathBuf,
    pub hostname: String,
    pub user: String,
}

impl Env {
    /// This account's real `~/.ssh`.
    pub fn real(paths: &Paths, hostname: String) -> Result<Self> {
        let home = std::env::home_dir()
            .ok_or_else(|| Error::Invalid("cannot find the home directory".into()))?;
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        Ok(Self {
            ssh_dir: home.join(".ssh"),
            machines_dir: crate::personal::machines_dir(paths),
            state_dir: paths.state_dir.clone(),
            hostname,
            user,
        })
    }
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

fn load_local(env: &Env) -> Local {
    std::fs::read(env.state_dir.join(STATE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_local(env: &Env, l: &Local) -> Result<()> {
    let mut b = serde_json::to_vec_pretty(l)?;
    b.push(b'\n');
    write_atomic(&env.state_dir.join(STATE_FILE), &b)
}

/// This machine's id, made once.
fn machine_id(env: &Env) -> Result<String> {
    let mut l = load_local(env);
    if l.id.is_empty() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let seed = format!("{nanos}:{}:{}", std::process::id(), env.state_dir.display());
        l.id = sha256_hex(seed.as_bytes())[..16].to_string();
        save_local(env, &l)?;
    }
    Ok(l.id)
}

/// Set the name other machines use for this one (`ssh <alias>`).
pub fn set_alias(env: &Env, alias: &str) -> Result<()> {
    if !valid_alias(alias) {
        return Err(Error::Invalid(format!(
            "`{alias}` is not a usable name: use letters, digits, `.`, `_` and `-`"
        )));
    }
    let mut l = load_local(env);
    l.alias = Some(alias.to_string());
    save_local(env, &l)
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

fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(String::from_utf8_lossy(&b).into_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write `text` to `path` when it differs (an empty text removes the file), with mode 0600.
fn put(path: &Path, text: &str) -> Result<()> {
    let old = read_text(path)?;
    if old.as_deref().unwrap_or("") == text {
        return Ok(());
    }
    if text.is_empty() {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => return Ok(()),
        }
    }
    if let Some(dir) = path.parent() {
        make_ssh_dir(dir)?;
    }
    write_atomic(path, text.as_bytes())?;
    set_mode(path, 0o600);
    Ok(())
}

fn make_ssh_dir(dir: &Path) -> Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
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

/// The other machines' records in `machines_dir`, valid ones only.
fn read_records(env: &Env) -> Vec<Record> {
    let Ok(entries) = std::fs::read_dir(&env.machines_dir) else {
        return Vec::new();
    };
    let mut out: Vec<Record> = entries
        .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
        .filter_map(|b| serde_json::from_slice::<Record>(&b).ok())
        .collect();
    out.sort_by(|a, b| (&a.alias, &a.id).cmp(&(&b.alias, &b.id)));
    out
}

fn own_file(env: &Env, id: &str) -> PathBuf {
    env.machines_dir.join(format!("{id}.json"))
}

/// Write this machine's record when it changed (turning the choice off removes it).
pub fn publish(env: &Env, probe: &dyn Probe, on: bool) -> Result<()> {
    if !on {
        let l = load_local(env);
        if !l.id.is_empty() {
            match std::fs::remove_file(own_file(env, &l.id)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        return Ok(());
    }
    let id = machine_id(env)?;
    let pub_path = env.ssh_dir.join(format!("{KEY}.pub"));
    let pubkey = match read_text(&pub_path)? {
        Some(t) => t,
        None => {
            if env.ssh_dir.join(KEY).exists() {
                return Err(Error::Invalid(format!(
                    "{KEY} exists without {KEY}.pub; nothing was changed"
                )));
            }
            make_ssh_dir(&env.ssh_dir)?;
            probe.keygen(&env.ssh_dir.join(KEY))?;
            read_text(&pub_path)?
                .ok_or_else(|| Error::Invalid("ssh-keygen made no public key".into()))?
        }
    };
    let pubkey = parse_key(&pubkey)
        .ok_or_else(|| Error::Invalid(format!("{KEY}.pub is not an ed25519 or ecdsa key")))?;
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
        .filter(|r| r.id != id)
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
        ts: now_unix() * 1000,
        lan: probe.lan_ip().filter(|l| valid_lan(l)),
    };
    let file = own_file(env, &id);
    let unchanged = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice::<Record>(&b).ok())
        .is_some_and(|old| old.same_as(&rec));
    if !unchanged {
        let mut b = serde_json::to_vec_pretty(&rec)?;
        b.push(b'\n');
        write_atomic(&file, &b)?;
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

/// Add the other machines to `authorized_keys` (not on Windows) and `claude-sync.conf`, or
/// remove everything this module wrote when `on` is false.
pub fn apply(env: &Env, probe: &dyn Probe, on: bool) -> Result<()> {
    let own_id = load_local(env).id;
    let list = if on { peers(env, &own_id) } else { Vec::new() };
    let windows = probe.os() == Os::Windows;

    if !windows {
        let path = env.ssh_dir.join("authorized_keys");
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

    let config_path = env.ssh_dir.join("config");
    let config = read_text(&config_path)?;
    let conf_path = env.ssh_dir.join(CONF);
    if on {
        let mine = user_hosts(config.as_deref().unwrap_or(""));
        let mut conf = String::new();
        let names: Vec<String> = list
            .iter()
            .map(|(r, _)| {
                if mine.contains(&r.alias) {
                    format!("{}-cs", r.alias)
                } else {
                    r.alias.clone()
                }
            })
            .collect();
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

#[derive(Debug, Clone, Serialize)]
pub struct Peer {
    pub alias: String,
    pub name: String,
    pub os: String,
    pub host: String,
    pub user: String,
    pub lan: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub on: bool,
    pub alias: Option<String>,
    pub peers: Vec<Peer>,
}

pub fn status(paths: &Paths, env: &Env) -> Result<Status> {
    let on = crate::choices::load(paths)?.ssh_peers.on;
    let l = load_local(env);
    Ok(Status {
        on,
        alias: l.alias,
        peers: peers(env, &l.id)
            .into_iter()
            .map(|(r, _)| Peer {
                alias: r.alias,
                name: r.name,
                os: r.os,
                host: r.host,
                user: r.user,
                lan: r.lan,
            })
            .collect(),
    })
}

/// The pass the sync loops run after personal sync: follows the account's choice.
pub fn sync_pass(paths: &Paths, hostname: String) -> Result<()> {
    let on = crate::choices::load(paths)?.ssh_peers.on;
    let env = Env::real(paths, hostname)?;
    pass(&env, &RealProbe, on)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn tailscale_ip_output_is_parsed_strictly() {
        assert_eq!(
            parse_ts_ip(b"100.64.1.2\nfd7a::1\n"),
            Some("100.64.1.2".into())
        );
        assert_eq!(parse_ts_ip(b"  100.64.1.2  \n"), Some("100.64.1.2".into()));
        assert_eq!(parse_ts_ip(b""), None);
        assert_eq!(parse_ts_ip(b"not running\n"), None);
        assert_eq!(parse_ts_ip(b"fd7a::1\n"), None);
    }

    #[test]
    fn tailscale_bundle_paths_are_tried_per_os() {
        assert_eq!(ts_candidates(Os::Other), vec!["tailscale"]);
        let mac = ts_candidates(Os::Mac);
        assert_eq!(mac[0], "tailscale");
        assert!(mac[1].starts_with("/Applications/Tailscale.app/"));
        let win = ts_candidates(Os::Windows);
        assert_eq!(win.len(), 2);
        assert!(win[1].ends_with("tailscale.exe"));
    }

    #[test]
    fn tailscale_answer_is_reused_inside_the_ttl_also_when_none() {
        let slot = Mutex::new(None);
        let ttl = Duration::from_secs(600);
        let t0 = Instant::now();
        let calls = Cell::new(0);
        let fetch = |v: Option<&str>| {
            calls.set(calls.get() + 1);
            v.map(String::from)
        };
        assert_eq!(cached_ip(&slot, t0, ttl, || fetch(None)), None);
        let t1 = t0 + Duration::from_secs(599);
        assert_eq!(cached_ip(&slot, t1, ttl, || fetch(Some("100.1.1.1"))), None);
        assert_eq!(calls.get(), 1);
        let t2 = t0 + Duration::from_secs(601);
        assert_eq!(
            cached_ip(&slot, t2, ttl, || fetch(Some("100.1.1.1"))),
            Some("100.1.1.1".into())
        );
        assert_eq!(calls.get(), 2);
    }

    struct Fake {
        ip: Option<String>,
        os: Os,
        keygens: Cell<u32>,
        lan: Option<String>,
        nc: bool,
    }

    impl Fake {
        fn new(os: Os, ip: Option<&str>) -> Self {
            Self {
                ip: ip.map(str::to_string),
                os,
                keygens: Cell::new(0),
                lan: None,
                nc: false,
            }
        }
        fn lan(mut self, ip: &str) -> Self {
            self.lan = Some(ip.into());
            self
        }
        fn nc(mut self) -> Self {
            self.nc = true;
            self
        }
    }

    impl Probe for Fake {
        fn tailscale_ip(&self) -> Option<String> {
            self.ip.clone()
        }
        fn keygen(&self, path: &Path) -> Result<()> {
            self.keygens.set(self.keygens.get() + 1);
            std::fs::write(path, "private\n")?;
            std::fs::write(
                format!("{}.pub", path.display()),
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIfake me@box\n",
            )?;
            Ok(())
        }
        fn os(&self) -> Os {
            self.os
        }
        fn lan_ip(&self) -> Option<String> {
            self.lan.clone()
        }
        fn has_nc(&self) -> bool {
            self.nc
        }
    }

    fn env(dir: &Path, name: &str, host: &str, user: &str) -> Env {
        Env {
            ssh_dir: dir.join(name).join(".ssh"),
            machines_dir: dir.join(name).join("machines"),
            state_dir: dir.join(name).join("state"),
            hostname: host.into(),
            user: user.into(),
        }
    }

    fn rec(id: &str, alias: &str) -> Record {
        Record {
            id: id.into(),
            name: "box".into(),
            alias: alias.into(),
            os: "linux".into(),
            user: "ana".into(),
            host: "100.64.0.9".into(),
            pubkey: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIpeer".into(),
            ts: 1,
            lan: None,
        }
    }

    fn put_rec(env: &Env, r: &Record) {
        std::fs::create_dir_all(&env.machines_dir).unwrap();
        std::fs::write(
            env.machines_dir.join(format!("{}.json", r.id)),
            serde_json::to_vec(r).unwrap(),
        )
        .unwrap();
    }

    fn read(p: PathBuf) -> String {
        std::fs::read_to_string(p).unwrap_or_default()
    }

    #[test]
    fn block_is_rendered_and_replaced_idempotently_keeping_foreign_lines() {
        let lines = vec!["a".to_string(), "b".to_string()];
        let block = render_block(&lines).unwrap();
        assert_eq!(block, format!("{BEGIN}\na\nb\n{END}"));
        let mine = "ssh-ed25519 AAAA mine\n";
        let once = replace_block(mine, Some(&block));
        assert_eq!(once, format!("{mine}{block}\n"));
        assert_eq!(replace_block(&once, Some(&block)), once);
        // A changed block replaces the old one in place; lines after it stay.
        let after = format!("{once}ssh-rsa BBBB later\n");
        let changed = replace_block(&after, Some(&render_block(&["c".into()]).unwrap()));
        assert_eq!(
            changed,
            format!("{mine}{BEGIN}\nc\n{END}\nssh-rsa BBBB later\n")
        );
        // Removing it leaves exactly the foreign lines.
        assert_eq!(
            replace_block(&after, None),
            format!("{mine}ssh-rsa BBBB later\n")
        );
        assert_eq!(replace_block("", None), "");
        assert!(render_block(&[]).is_none());
    }

    #[test]
    fn keys_with_options_newlines_or_old_types_are_refused() {
        assert_eq!(
            parse_key("ssh-ed25519 AAAAC3Nza me@x\n").as_deref(),
            Some("ssh-ed25519 AAAAC3Nza")
        );
        assert!(parse_key("ecdsa-sha2-nistp256 AAAAE2VjZHNh c").is_some());
        assert!(parse_key("command=\"rm -rf /\" ssh-ed25519 AAAA").is_none());
        assert!(parse_key("from=\"*\" ssh-ed25519 AAAA").is_none());
        assert!(parse_key("ssh-ed25519 AAAA\nssh-ed25519 BBBB").is_none());
        assert!(parse_key("ssh-ed25519 AAAA\rcommand=x").is_none());
        assert!(parse_key("ssh-rsa AAAAB3Nza").is_none());
        assert!(parse_key("ssh-dss AAAAB3Nza").is_none());
        assert!(parse_key("ssh-ed25519").is_none());
        assert!(parse_key("ssh-ed25519 AA\"AA").is_none());
        assert!(parse_key("").is_none());
    }

    #[test]
    fn include_is_the_first_line_once_and_removed_on_off() {
        let cfg = "Host work\n  User me\n";
        let on = with_include(cfg, true);
        assert_eq!(on, format!("{INCLUDE}\n{cfg}"));
        assert_eq!(with_include(&on, true), on, "once");
        assert_eq!(with_include(&on, false), cfg);
        assert_eq!(with_include(&format!("{INCLUDE}\n"), false), "");
    }

    #[test]
    fn keygen_runs_only_when_the_key_is_missing_and_never_overwrites() {
        let t = tempfile::tempdir().unwrap();
        let e = env(t.path(), "a", "box", "ana");
        let p = Fake::new(Os::Other, None);
        publish(&e, &p, true).unwrap();
        assert_eq!(p.keygens.get(), 1);
        publish(&e, &p, true).unwrap();
        assert_eq!(p.keygens.get(), 1, "the key exists now");
        // A private key without its .pub is an error, not an overwrite.
        std::fs::remove_file(e.ssh_dir.join("id_ed25519.pub")).unwrap();
        assert!(publish(&e, &p, true).is_err());
        assert_eq!(p.keygens.get(), 1);
        assert_eq!(read(e.ssh_dir.join("id_ed25519")), "private\n");
    }

    #[test]
    fn host_is_the_tailscale_ip_or_the_hostname_and_alias_follows_the_os() {
        let t = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            env(t.path(), "a", "mac-book", "ana"),
            env(t.path(), "b", "tower", "ana"),
            env(t.path(), "c", "laptop", "ana"),
        );
        publish(&a, &Fake::new(Os::Mac, Some("100.64.1.2")), true).unwrap();
        publish(&b, &Fake::new(Os::Other, None), true).unwrap();
        publish(&c, &Fake::new(Os::Windows, None), true).unwrap();
        let get = |e: &Env| read_records(e).pop().unwrap();
        let (ra, rb, rc) = (get(&a), get(&b), get(&c));
        assert_eq!((ra.alias.as_str(), ra.host.as_str()), ("mac", "100.64.1.2"));
        assert_eq!((rb.alias.as_str(), rb.host.as_str()), ("pc", "tower"));
        assert_eq!(rc.alias, "pc");
        assert_eq!(rb.user, "ana");
        assert!(rb.pubkey.starts_with("ssh-ed25519 "));
    }

    #[test]
    fn a_chosen_alias_is_used_and_a_clash_gets_a_number() {
        let t = tempfile::tempdir().unwrap();
        let e = env(t.path(), "a", "box", "ana");
        assert!(set_alias(&e, "bad name").is_err());
        assert!(set_alias(&e, "").is_err());
        let p = Fake::new(Os::Other, None);
        put_rec(&e, &rec("other", "pc"));
        publish(&e, &p, true).unwrap();
        let mine: Vec<_> = read_records(&e)
            .into_iter()
            .filter(|r| r.id != "other")
            .collect();
        assert_eq!(mine[0].alias, "pc-2");
        set_alias(&e, "tower").unwrap();
        publish(&e, &p, true).unwrap();
        let mine: Vec<_> = read_records(&e)
            .into_iter()
            .filter(|r| r.id != "other")
            .collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].alias, "tower");
    }

    #[test]
    fn an_unchanged_record_is_not_written_again() {
        let t = tempfile::tempdir().unwrap();
        let e = env(t.path(), "a", "box", "ana");
        let p = Fake::new(Os::Other, None);
        publish(&e, &p, true).unwrap();
        let id = load_local(&e).id;
        let file = own_file(&e, &id);
        let first = std::fs::read(&file).unwrap();
        // A record with an old time and the same fields stays as it is.
        let mut old: Record = serde_json::from_slice(&first).unwrap();
        old.ts = 5;
        std::fs::write(&file, serde_json::to_vec_pretty(&old).unwrap()).unwrap();
        let before = std::fs::read(&file).unwrap();
        publish(&e, &p, true).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), before);
        // A changed field is written.
        let p2 = Fake::new(Os::Other, Some("100.64.0.7"));
        publish(&e, &p2, true).unwrap();
        assert_ne!(std::fs::read(&file).unwrap(), before);
    }

    #[test]
    fn two_homes_a_publishes_b_applies_and_off_removes() {
        let t = tempfile::tempdir().unwrap();
        let a = env(t.path(), "a", "mac-book", "ana");
        let b = env(t.path(), "b", "tower", "bob");
        let pa = Fake::new(Os::Mac, Some("100.64.1.2"));
        let pb = Fake::new(Os::Other, Some("100.64.3.4"));

        // B already has its own key lines and its own Host entries.
        std::fs::create_dir_all(&b.ssh_dir).unwrap();
        std::fs::write(b.ssh_dir.join("authorized_keys"), "ssh-rsa OLD mine\n").unwrap();
        std::fs::write(b.ssh_dir.join("config"), "Host mac\n  User me\n").unwrap();

        pass(&a, &pa, true).unwrap();
        pass(&b, &pb, true).unwrap();
        // The personal repo carries the records: copy each to the other.
        for (from, to) in [(&a, &b), (&b, &a)] {
            for r in read_records(from) {
                put_rec(to, &r);
            }
        }
        pass(&b, &pb, true).unwrap();
        pass(&a, &pa, true).unwrap();

        let keys = read(b.ssh_dir.join("authorized_keys"));
        assert!(keys.starts_with("ssh-rsa OLD mine\n"));
        assert!(keys.contains(BEGIN) && keys.contains(END));
        assert!(keys.contains(&format!(
            "from=\"{FROM}\",no-agent-forwarding,no-X11-forwarding ssh-ed25519 "
        )));
        assert!(keys.contains(" claude-sync:mac\n"));
        assert!(!keys.contains("claude-sync:pc"), "its own key is not added");
        let conf = read(b.ssh_dir.join(CONF));
        // B defines `mac` itself, so A is reachable as `mac-cs`.
        assert_eq!(conf, "Host mac-cs\n  HostName 100.64.1.2\n  User ana\n");
        let cfg = read(b.ssh_dir.join("config"));
        assert!(cfg.starts_with("Include claude-sync.conf\nHost mac\n"));
        // A, with no clash, gets the plain alias for B.
        let conf_a = read(a.ssh_dir.join(CONF));
        assert_eq!(conf_a, "Host pc\n  HostName 100.64.3.4\n  User bob\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(a.ssh_dir.join("authorized_keys")), 0o600);
            assert_eq!(mode(a.ssh_dir.clone()), 0o700);
        }

        // Running again changes nothing.
        let before = (keys.clone(), conf.clone(), cfg.clone());
        pass(&b, &pb, true).unwrap();
        assert_eq!(
            before,
            (
                read(b.ssh_dir.join("authorized_keys")),
                read(b.ssh_dir.join(CONF)),
                read(b.ssh_dir.join("config"))
            )
        );

        // Off on B removes what it wrote and its own record, and nothing else.
        let b_id = load_local(&b).id;
        pass(&b, &pb, false).unwrap();
        assert_eq!(
            read(b.ssh_dir.join("authorized_keys")),
            "ssh-rsa OLD mine\n"
        );
        assert!(!b.ssh_dir.join(CONF).exists());
        assert_eq!(read(b.ssh_dir.join("config")), "Host mac\n  User me\n");
        assert!(!own_file(&b, &b_id).exists());
    }

    #[test]
    fn windows_writes_the_conf_but_not_authorized_keys() {
        let t = tempfile::tempdir().unwrap();
        let e = env(t.path(), "w", "winbox", "ana");
        put_rec(&e, &rec("other", "mac"));
        pass(&e, &Fake::new(Os::Windows, None), true).unwrap();
        assert!(!e.ssh_dir.join("authorized_keys").exists());
        assert!(read(e.ssh_dir.join(CONF)).contains("Host mac\n"));
        assert!(read(e.ssh_dir.join("config")).starts_with(INCLUDE));
    }

    #[test]
    fn records_with_unsafe_fields_are_ignored() {
        let t = tempfile::tempdir().unwrap();
        let e = env(t.path(), "a", "box", "ana");
        let mut bad_alias = rec("1", "x y");
        bad_alias.alias = "x\nProxyCommand evil".into();
        let mut bad_user = rec("2", "u");
        bad_user.user = "a b".into();
        let mut bad_host = rec("3", "h");
        bad_host.host = "h; rm".into();
        let mut bad_key = rec("4", "k");
        bad_key.pubkey = "command=\"x\" ssh-ed25519 AAAA".into();
        for r in [
            &bad_alias,
            &bad_user,
            &bad_host,
            &bad_key,
            &rec("5", "good"),
        ] {
            put_rec(&e, r);
        }
        pass(&e, &Fake::new(Os::Other, None), true).unwrap();
        let conf = read(e.ssh_dir.join(CONF));
        assert!(conf.contains("Host good\n"));
        assert!(!conf.contains("evil") && !conf.contains("rm") && !conf.contains("Host u\n"));
        assert_eq!(
            read(e.ssh_dir.join("authorized_keys"))
                .matches("claude-sync:")
                .count(),
            1
        );
    }

    fn conf_for(os: Os, nc: bool, r: &Record, config: &str) -> String {
        let t = tempfile::tempdir().unwrap();
        let e = env(t.path(), "a", "box", "ana");
        std::fs::create_dir_all(&e.ssh_dir).unwrap();
        std::fs::write(e.ssh_dir.join("config"), config).unwrap();
        put_rec(&e, r);
        let mut p = Fake::new(os, None);
        p.nc = nc;
        pass(&e, &p, true).unwrap();
        read(e.ssh_dir.join(CONF))
    }

    const PROXY: &str = "  HostKeyAlias 100.64.0.9\n  ProxyCommand sh -c 'nc -z -w 2 %h %p 2>/dev/null && exec nc %h %p; nc -z -w 2 192.168.1.5 %p 2>/dev/null && exec nc 192.168.1.5 %p; exit 1'\n";

    #[test]
    fn a_peer_lan_gives_a_proxy_command_and_a_lan_host() {
        let mut r = rec("o", "mac");
        r.lan = Some("192.168.1.5".into());
        let conf = conf_for(Os::Other, true, &r, "");
        assert_eq!(
            conf,
            format!(
                "Host mac\n  HostName 100.64.0.9\n  User ana\n{PROXY}Host mac-lan\n  HostName 192.168.1.5\n  User ana\n  HostKeyAlias 100.64.0.9\n"
            )
        );
        let mac = conf_for(Os::Mac, true, &r, "");
        assert_eq!(mac, conf);
    }

    #[test]
    fn without_nc_or_on_windows_the_entry_is_todays() {
        let mut r = rec("o", "mac");
        r.lan = Some("192.168.1.5".into());
        let today = "Host mac\n  HostName 100.64.0.9\n  User ana\n";
        assert_eq!(conf_for(Os::Other, false, &r, ""), today);
        assert_eq!(conf_for(Os::Windows, true, &r, ""), today);
        assert_eq!(conf_for(Os::Other, true, &rec("o", "mac"), ""), today);
    }

    #[test]
    fn a_taken_lan_host_name_is_skipped() {
        let mut r = rec("o", "mac");
        r.lan = Some("192.168.1.5".into());
        let user = conf_for(Os::Other, true, &r, "Host mac-lan\n  User me\n");
        assert!(user.contains(PROXY) && !user.contains("Host mac-lan"));
        // Another peer's alias equal to `mac-lan`.
        let t = tempfile::tempdir().unwrap();
        let e = env(t.path(), "a", "box", "ana");
        put_rec(&e, &r);
        put_rec(&e, &rec("p", "mac-lan"));
        pass(&e, &Fake::new(Os::Other, None).nc(), true).unwrap();
        let conf = read(e.ssh_dir.join(CONF));
        assert_eq!(conf.matches("Host mac-lan\n").count(), 1);
        assert!(conf.contains("Host mac-lan\n  HostName 100.64.0.9\n"));
        // A `-cs` rename gives `-cs-lan`.
        let renamed = conf_for(Os::Other, true, &r, "Host mac\n  User me\n");
        assert!(renamed.contains("Host mac-cs-lan\n"));
    }

    #[test]
    fn bad_lan_values_are_rejected_on_publish_and_dropped_on_read() {
        for bad in [
            "192.168.1.1; rm",
            "192.168.01.1",
            " 192.168.1.1",
            "8.8.8.8",
            "100.64.0.1",
            "172.32.0.1",
            "::1",
        ] {
            assert!(!valid_lan(bad), "{bad}");
            let t = tempfile::tempdir().unwrap();
            let e = env(t.path(), "a", "box", "ana");
            publish(&e, &Fake::new(Os::Other, None).lan(bad), true).unwrap();
            assert_eq!(read_records(&e)[0].lan, None, "{bad}");
            let mut r = rec("o", "mac");
            r.lan = Some(bad.into());
            put_rec(&e, &r);
            let own = load_local(&e).id;
            let got = peers(&e, &own);
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].0.lan, None, "{bad}");
        }
        for good in ["10.1.2.3", "172.16.0.1", "172.31.255.1", "192.168.4.10"] {
            assert!(valid_lan(good), "{good}");
        }
    }

    #[test]
    fn from_gains_the_writers_private_block() {
        let keys = |lan: Option<&str>| {
            let t = tempfile::tempdir().unwrap();
            let e = env(t.path(), "a", "box", "ana");
            put_rec(&e, &rec("o", "mac"));
            let mut p = Fake::new(Os::Other, None);
            p.lan = lan.map(String::from);
            apply(&e, &p, true).unwrap();
            read(e.ssh_dir.join("authorized_keys"))
        };
        assert!(keys(Some("192.168.4.10")).contains(&format!("from=\"{FROM},192.168.0.0/16\",")));
        assert!(keys(Some("10.1.2.3")).contains(&format!("from=\"{FROM},10.0.0.0/8\",")));
        assert!(keys(Some("172.20.1.1")).contains(&format!("from=\"{FROM},172.16.0.0/12\",")));
        assert!(keys(None).contains(&format!("from=\"{FROM}\",no-agent")));
        assert_eq!(
            key_line(&rec("o", "mac"), "ssh-ed25519 K", None),
            format!("from=\"{FROM}\",no-agent-forwarding,no-X11-forwarding ssh-ed25519 K claude-sync:mac")
        );
    }

    #[test]
    fn lan_serializes_only_when_set() {
        let r = rec("o", "mac");
        assert!(!serde_json::to_string(&r).unwrap().contains("lan"));
        let old = r#"{"id":"o","name":"b","alias":"mac","os":"linux","user":"a","host":"h","pubkey":"k","ts":1}"#;
        assert_eq!(serde_json::from_str::<Record>(old).unwrap().lan, None);
        let mut with = r;
        with.lan = Some("10.0.0.2".into());
        let back: Record = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(back.lan.as_deref(), Some("10.0.0.2"));
    }

    #[test]
    fn lan_from_decides_from_what_the_device_reports() {
        use std::net::IpAddr;
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        assert_eq!(
            lan_from(ip("192.168.1.7"), false, Os::Other).as_deref(),
            Some("192.168.1.7")
        );
        assert_eq!(
            lan_from(ip("10.0.0.7"), false, Os::Mac).as_deref(),
            Some("10.0.0.7")
        );
        assert_eq!(lan_from(ip("192.168.1.7"), false, Os::Windows), None);
        assert_eq!(lan_from(ip("172.20.0.2"), true, Os::Other), None);
        assert_eq!(lan_from(ip("8.8.8.8"), false, Os::Other), None);
        assert_eq!(lan_from(ip("100.64.0.1"), false, Os::Other), None);
        assert_eq!(lan_from(ip("fd00::1"), false, Os::Other), None);
        assert_eq!(lan_from(None, false, Os::Other), None);
    }

    #[test]
    fn off_when_nothing_was_ever_written_touches_nothing() {
        let t = tempfile::tempdir().unwrap();
        let e = env(t.path(), "a", "box", "ana");
        pass(&e, &Fake::new(Os::Other, None), false).unwrap();
        assert!(!e.ssh_dir.exists());
    }
}
