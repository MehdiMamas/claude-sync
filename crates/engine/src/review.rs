//! Review of other members' CLAUDE.md lines: the group's change is split into line items;
//! this account's items apply at once, others' wait as pending until accepted or declined
//! per item; a declined item is never applied or asked again. Items are patched by content,
//! not line number. Item ids and the JSON shape match v1 (`claude-md-review.json`, format 2)
//! so a stored review state carries over.
//!
//! Parity: per-line accept and decline, accept all, decline all, a declined line stays out
//! (and "use the group's version" brings it back), an undo of a pending change cancels it,
//! a change with nothing left to do is dropped, notification text.
//!
//! v2-Parity rows covered: 37, 38, 82.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FORMAT: u32 = 2;
const MAX_DECLINED: usize = 500;
const CONTEXT: usize = 2;
const MAX_DIFF_CELLS: usize = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Add,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub lines: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub before: Vec<String>,
    #[serde(default)]
    pub pos: usize,
    pub author: String,
    pub commit: String,
    pub message: String,
    pub at_unix: u64,
}

impl Item {
    fn is_inverse_of(&self, o: &Item) -> bool {
        self.kind != o.kind && self.lines == o.lines
    }
}

/// The group's `CLAUDE.md` text after one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub text: String,
    /// Made by this account.
    pub mine: bool,
    pub author: String,
    pub commit: String,
    pub message: String,
    pub at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Declined {
    pub id: String,
    pub at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReviewState {
    pub format: u32,
    /// `None` follows the group's `autoAcceptClaudeMd`; off applies others' changes at once.
    pub review: Option<bool>,
    pub group: String,
    pub accepted: String,
    pub pending: Vec<Item>,
    pub declined: Vec<Declined>,
}

impl Default for ReviewState {
    fn default() -> Self {
        Self {
            format: FORMAT,
            review: None,
            group: String::new(),
            accepted: String::new(),
            pending: vec![],
            declined: vec![],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pulled {
    pub new: Vec<Item>,
    pub applied: Vec<Item>,
}

impl ReviewState {
    /// A group seen for the first time: its text is accepted as it is.
    pub fn new(text: &str) -> Self {
        Self {
            group: text.into(),
            accepted: text.into(),
            ..Self::default()
        }
    }

    pub fn pull(&mut self, steps: &[Step]) -> Pulled {
        let mut out = Pulled::default();
        let in_step = self.accepted == self.group && self.pending.is_empty();
        let mut all_applied = true;
        let mut prev = self.group.clone();
        for step in steps {
            if step.text == prev {
                continue;
            }
            for item in items_between(&prev, &step.text, step) {
                if self.declined.iter().any(|d| d.id == item.id) {
                    all_applied = false;
                    continue;
                }
                if let Some(i) = self.pending.iter().position(|p| p.is_inverse_of(&item)) {
                    self.pending.remove(i);
                    out.new.retain(|p| !p.is_inverse_of(&item));
                    continue;
                }
                let patched = apply(&self.accepted, &item);
                if step.mine || !self.review.unwrap_or(true) {
                    match patched {
                        Some(t) => {
                            self.accepted = t;
                            out.applied.push(item);
                        }
                        None => all_applied = false,
                    }
                } else if patched.is_some() {
                    all_applied = false;
                    self.pending.push(item.clone());
                    out.new.push(item);
                }
            }
            prev = step.text.clone();
        }
        if in_step && all_applied && self.pending.is_empty() {
            self.accepted = prev.clone();
        }
        self.group = prev;
        self.settle();
        out
    }

    /// Accept pending items (`None`: all).
    pub fn accept(&mut self, ids: Option<&[String]>) -> Vec<Item> {
        let mut applied = vec![];
        for item in self.take(ids) {
            if let Some(t) = apply(&self.accepted, &item) {
                self.accepted = t;
                applied.push(item);
            }
        }
        self.settle();
        applied
    }

    /// Decline pending items (`None`: all).
    pub fn decline(&mut self, ids: Option<&[String]>, now: u64) -> usize {
        let items = self.take(ids);
        for i in &items {
            self.remember(&i.id, now);
        }
        items.len()
    }

    /// Take one line of the accepted text out for this user only; "use the group's version"
    /// ([`reset`](Self::reset)) brings it back. `None` when the text has no such line.
    pub fn drop_line(&mut self, line: &str, now: u64) -> Option<Item> {
        let line = line.trim();
        let lines = split(&self.accepted);
        let at = lines.iter().position(|l| key(l).trim() == line)?;
        let new: String = lines[..at]
            .iter()
            .chain(&lines[at + 1..])
            .copied()
            .collect();
        let step = Step {
            text: new.clone(),
            mine: true,
            author: String::new(),
            commit: format!("local:{now}"),
            message: String::new(),
            at_unix: now,
        };
        let item = items_between(&self.accepted, &new, &step)
            .into_iter()
            .find(|i| i.kind == Kind::Remove)?;
        self.accepted = new;
        self.remember(&item.id, now);
        self.settle();
        Some(item)
    }

    /// "Use the group's version": decisions forgotten.
    pub fn reset(&mut self) {
        self.accepted = self.group.clone();
        self.pending.clear();
        self.declined.clear();
    }

    fn remember(&mut self, id: &str, now: u64) {
        self.declined.push(Declined {
            id: id.into(),
            at_unix: now,
        });
        let excess = self.declined.len().saturating_sub(MAX_DECLINED);
        self.declined.drain(..excess);
    }

    fn take(&mut self, ids: Option<&[String]>) -> Vec<Item> {
        let (taken, kept) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|i| ids.is_none_or(|ids| ids.contains(&i.id)));
        self.pending = kept;
        taken
    }

    fn settle(&mut self) {
        let a = &self.accepted;
        self.pending.retain(|i| apply(a, i).is_some());
    }
}

/// The notification for new pending items, e.g. "ana changed Friends's CLAUDE.md: 2
/// additions, 1 removal to review". `None` when there is nothing to review.
pub fn notification(group: &str, items: &[Item]) -> Option<String> {
    let mut authors: Vec<&str> = vec![];
    for i in items {
        let a = if i.author.is_empty() {
            "Someone"
        } else {
            &i.author
        };
        if !authors.contains(&a) {
            authors.push(a);
        }
    }
    let who = match authors.as_slice() {
        [] => return None,
        [one] => one.to_string(),
        [a, b] => format!("{a} and {b}"),
        [a, rest @ ..] => format!("{a} and {} others", rest.len()),
    };
    let count = |k: Kind, one: &str, many: &str| {
        let n = items.iter().filter(|i| i.kind == k).count();
        (n > 0).then(|| format!("{n} {}", if n == 1 { one } else { many }))
    };
    let what: Vec<String> = [
        count(Kind::Add, "addition", "additions"),
        count(Kind::Remove, "removal", "removals"),
    ]
    .into_iter()
    .flatten()
    .collect();
    Some(format!(
        "{who} changed {group}'s CLAUDE.md: {} to review",
        what.join(", ")
    ))
}

/// `item` patched into `text`; `None` when there is nothing to do.
pub fn apply(text: &str, item: &Item) -> Option<String> {
    let lines = split(text);
    let keys: Vec<&str> = lines.iter().map(|l| key(l)).collect();
    let k = item.lines.len();
    if k == 0 {
        return None;
    }
    let matches_at = |p: usize| p + k <= keys.len() && keys[p..p + k].iter().eq(&item.lines);
    let score = |start: usize, end: usize| {
        let after = item
            .after
            .iter()
            .rev()
            .zip((0..start).rev())
            .take_while(|(w, i)| keys[*i] == w.as_str())
            .count();
        let before = item
            .before
            .iter()
            .zip(end..keys.len())
            .take_while(|(w, i)| keys[*i] == w.as_str())
            .count();
        after + before
    };
    let best = |c: Vec<usize>, width: usize| {
        c.into_iter()
            .max_by_key(|&p| (score(p, p + width), std::cmp::Reverse(p.abs_diff(item.pos))))
    };
    match item.kind {
        Kind::Remove => {
            let at = best((0..keys.len()).filter(|&p| matches_at(p)).collect(), k)?;
            Some(
                lines[..at]
                    .iter()
                    .chain(&lines[at + k..])
                    .copied()
                    .collect(),
            )
        }
        Kind::Add => {
            let at = best((0..=keys.len()).collect(), 0).unwrap_or(keys.len());
            if matches_at(at) || (at >= k && matches_at(at - k)) {
                return None;
            }
            let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
            let mut out = String::new();
            for (i, l) in lines[..at].iter().enumerate() {
                out.push_str(l);
                if i + 1 == at && !l.ends_with('\n') {
                    out.push_str(eol);
                }
            }
            for l in &item.lines {
                out.push_str(l);
                out.push_str(eol);
            }
            lines[at..].iter().for_each(|l| out.push_str(l));
            Some(out)
        }
    }
}

fn split(t: &str) -> Vec<&str> {
    t.split_inclusive('\n').collect()
}

fn key(l: &str) -> &str {
    l.trim_end_matches(['\n', '\r'])
}

/// The items that turn `old` into `new`: per changed block a removal and an addition.
pub fn items_between(old: &str, new: &str, step: &Step) -> Vec<Item> {
    let a: Vec<&str> = split(old).into_iter().map(key).collect();
    let b: Vec<&str> = split(new).into_iter().map(key).collect();
    let mut out = vec![];
    for h in hunks(&a, &b) {
        let after: Vec<String> = a[h.a.saturating_sub(CONTEXT)..h.a]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let end = h.a + h.del;
        let before: Vec<String> = a[end..(end + CONTEXT).min(a.len())]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for (kind, src) in [
            (Kind::Remove, &a[h.a..end]),
            (Kind::Add, &b[h.b..h.b + h.add]),
        ] {
            if src.is_empty() {
                continue;
            }
            let lines: Vec<String> = src.iter().map(|s| s.to_string()).collect();
            out.push(Item {
                id: item_id(&step.commit, kind, &lines, &after, &before),
                kind,
                lines,
                after: after.clone(),
                before: before.clone(),
                pos: h.a,
                author: step.author.clone(),
                commit: step.commit.clone(),
                message: step.message.clone(),
                at_unix: step.at_unix,
            });
        }
    }
    out
}

fn item_id(
    commit: &str,
    kind: Kind,
    lines: &[String],
    after: &[String],
    before: &[String],
) -> String {
    let t = serde_json::to_string(&(commit, kind, lines, after, before)).unwrap_or_default();
    let d = Sha256::digest(t.as_bytes());
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Copy)]
struct Hunk {
    a: usize,
    del: usize,
    b: usize,
    add: usize,
}

/// Line diff: LCS between the common prefix and suffix.
fn hunks(a: &[&str], b: &[&str]) -> Vec<Hunk> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let (n, m) = (ma.len(), mb.len());
    if n == 0 && m == 0 {
        return vec![];
    }
    // shortcut: past 4M cells the block is one removal plus one addition; upgrade to Myers if that is too coarse
    if n == 0 || m == 0 || n.saturating_mul(m) > MAX_DIFF_CELLS {
        return vec![Hunk {
            a: pre,
            del: n,
            b: pre,
            add: m,
        }];
    }
    let w = m + 1;
    let mut lcs = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * w + j] = if ma[i] == mb[j] {
                lcs[(i + 1) * w + j + 1] + 1
            } else {
                lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
            };
        }
    }
    let mut out = vec![];
    let (mut i, mut j) = (0, 0);
    let mut open: Option<Hunk> = None;
    while i < n || j < m {
        if i < n && j < m && ma[i] == mb[j] {
            out.extend(open.take());
            i += 1;
            j += 1;
            continue;
        }
        let h = open.get_or_insert(Hunk {
            a: pre + i,
            del: 0,
            b: pre + j,
            add: 0,
        });
        if j < m && (i == n || lcs[i * w + j + 1] >= lcs[(i + 1) * w + j]) {
            h.add += 1;
            j += 1;
        } else {
            h.del += 1;
            i += 1;
        }
    }
    out.extend(open.take());
    out
}
