//! The text formats: the CLAUDE.md managed block, the authorized_keys peers block, the
//! proposal branch name and the marketplace `COMMIT` file. Bytes outside a managed block are
//! never changed.
use chrono::DateTime;

pub const BLOCK_BEGIN: &str = "<!-- claude-sync:begin (managed, do not edit inside) -->";
pub const BLOCK_END: &str = "<!-- claude-sync:end -->";
pub const PEERS_BEGIN: &str = "# >>> claude-sync peers >>>";
pub const PEERS_END: &str = "# <<< claude-sync peers <<<";
pub const PROPOSAL_PREFIX: &str = "proposal/";
pub const COMMIT_FILE: &str = "COMMIT";

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

fn eol_len_at(text: &[u8], at: usize) -> usize {
    match text.get(at..) {
        Some([b'\r', b'\n', ..]) => 2,
        Some([b'\n', ..]) => 1,
        _ => 0,
    }
}

/// Byte range of the managed block, including the line end after the end marker. The begin
/// marker must start a line.
fn find_block(text: &[u8]) -> Option<(usize, usize)> {
    let mut from = 0;
    loop {
        let s = find(text, BLOCK_BEGIN.as_bytes(), from)?;
        if s == 0 || text[s - 1] == b'\n' {
            let e = find(text, BLOCK_END.as_bytes(), s)? + BLOCK_END.len();
            return Some((s, e + eol_len_at(text, e)));
        }
        from = s + 1;
    }
}

/// The lines inside the managed block (none without a block), line ends trimmed.
pub fn block_lines(text: &[u8]) -> Vec<String> {
    let Some((s, e)) = find_block(text) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&text[s..e])
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| *l != BLOCK_BEGIN && *l != BLOCK_END)
        .map(str::to_string)
        .collect()
}

/// `existing` with the block set to `lines` (no lines: block removed). Everything outside the
/// block is kept byte for byte. A new block goes at the top, followed by one blank line when
/// the file has other content; removing the block removes that line again. The block uses
/// the file's line ends (CRLF if its first line ends with CRLF).
pub fn render_claude_md(existing: &[u8], lines: &[String]) -> Vec<u8> {
    let eol: &[u8] = match find(existing, b"\n", 0) {
        Some(i) if i > 0 && existing[i - 1] == b'\r' => b"\r\n",
        _ => b"\n",
    };
    let mut block = Vec::new();
    if !lines.is_empty() {
        block.extend_from_slice(BLOCK_BEGIN.as_bytes());
        block.extend_from_slice(eol);
        for l in lines {
            block.extend_from_slice(l.as_bytes());
            block.extend_from_slice(eol);
        }
        block.extend_from_slice(BLOCK_END.as_bytes());
        block.extend_from_slice(eol);
    }
    let mut out = Vec::with_capacity(existing.len() + block.len() + 2);
    match find_block(existing) {
        Some((s, e)) => {
            out.extend_from_slice(&existing[..s]);
            let mut after = &existing[e..];
            if block.is_empty() && s == 0 {
                after = &after[eol_len_at(after, 0)..];
            }
            out.extend_from_slice(&block);
            out.extend_from_slice(after);
        }
        None if block.is_empty() => out.extend_from_slice(existing),
        None => {
            out.extend_from_slice(&block);
            if !existing.is_empty() {
                out.extend_from_slice(eol);
                out.extend_from_slice(existing);
            }
        }
    }
    out
}

/// The lines inside the peers block of `~/.ssh/authorized_keys` (none without a block).
pub fn peers_lines(existing: &str) -> Vec<String> {
    let lines: Vec<&str> = existing.lines().collect();
    let Some(b) = lines.iter().position(|l| l.trim_end() == PEERS_BEGIN) else {
        return Vec::new();
    };
    match lines[b..].iter().position(|l| l.trim_end() == PEERS_END) {
        Some(e) => lines[b + 1..b + e].iter().map(|l| l.to_string()).collect(),
        None => Vec::new(),
    }
}

/// `existing` with the peers block set to `block_lines` (none: block removed, a missing block
/// is added at the end). Lines outside the block are kept; line ends follow the file.
pub fn replace_peers_block(existing: &str, block_lines: &[String]) -> String {
    let eol = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let lines: Vec<&str> = existing.lines().collect();
    let begin = lines.iter().position(|l| l.trim_end() == PEERS_BEGIN);
    let end = begin.and_then(|b| {
        lines[b..]
            .iter()
            .position(|l| l.trim_end() == PEERS_END)
            .map(|e| e + b)
    });
    let block: Vec<String> = if block_lines.is_empty() {
        Vec::new()
    } else {
        std::iter::once(PEERS_BEGIN.to_string())
            .chain(block_lines.iter().cloned())
            .chain(std::iter::once(PEERS_END.to_string()))
            .collect()
    };
    let mut out: Vec<String> = Vec::new();
    match (begin, end) {
        (Some(b), Some(e)) => {
            out.extend(lines[..b].iter().map(|l| l.to_string()));
            out.extend(block);
            out.extend(lines[e + 1..].iter().map(|l| l.to_string()));
        }
        _ => {
            out.extend(lines.iter().map(|l| l.to_string()));
            out.extend(block);
        }
    }
    if out.is_empty() {
        return String::new();
    }
    out.join(eol) + eol
}

/// `proposal/<login>-<yyyymmddHHMMSS>` (UTC). `None` for a time chrono cannot show.
pub fn proposal_branch(login: &str, unix_secs: i64) -> Option<String> {
    let t = DateTime::from_timestamp(unix_secs, 0)?;
    Some(format!(
        "{PROPOSAL_PREFIX}{login}-{}",
        t.format("%Y%m%d%H%M%S")
    ))
}

/// `proposal/bo-20261126101500` -> `("bo", "20261126101500")`. The login may hold dashes.
pub fn parse_proposal_branch(branch: &str) -> Option<(&str, &str)> {
    let (login, stamp) = branch.strip_prefix(PROPOSAL_PREFIX)?.rsplit_once('-')?;
    (!login.is_empty() && stamp.len() == 14 && stamp.bytes().all(|b| b.is_ascii_digit()))
        .then_some((login, stamp))
}

/// `marketplaces/<name>/COMMIT`: the group's commit sha, then its date when known, one per line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMarker {
    pub commit: String,
    pub date: Option<String>,
}

impl CommitMarker {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut lines = text.lines();
        let commit = lines.next().filter(|l| !l.is_empty())?.to_string();
        Some(CommitMarker {
            commit,
            date: lines.next().filter(|l| !l.is_empty()).map(String::from),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut text = format!("{}\n", self.commit);
        if let Some(d) = &self.date {
            text.push_str(&format!("{d}\n"));
        }
        text.into_bytes()
    }
}
