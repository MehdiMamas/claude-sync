//! Review of other members' changes to a group's `CLAUDE.md` (docs/architecture.md §7a).
//!
//! The copy in `~/.claude-sync/groups/<g>/CLAUDE.md` is this device's, not the group's.
//! When the group's `CLAUDE.md` changes, the change is split into line items (one block of
//! added lines, one block of removed lines). Items from this account (any device, or a
//! clone pushing under the same login) apply at once; items from someone else wait as
//! **pending** until the user accepts or declines each one. A declined item is never
//! applied and never asked again; the next change in that place is a new item.
//!
//! Three texts per group, kept in `groups/<g>/claude-md-review.json`:
//! - `group`: the group's `CLAUDE.md` as last seen (the base snapshot's version);
//! - `accepted`: what the copy should hold: the group's text with this user's decisions;
//! - the copy itself, which the user may edit (an unshared local edit, §7).
//!
//! Items are patched by content, not by line number: a removal finds its lines, an addition
//! goes next to the lines it followed. A patch that has nothing left to do is dropped.
use crate::state::write_json;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// `format` of `claude-md-review.json` this app writes and reads.
pub const FORMAT: u32 = 2;
/// The file next to `status.json` in the group's state folder.
pub const FILE: &str = "claude-md-review.json";
/// Declined items remembered per group.
const MAX_DECLINED: usize = 500;
/// Lines of context kept on each side of an item.
const CONTEXT: usize = 2;
/// Above this many cells the diff of the changed middle gives up on alignment and
/// reports it as removed then added (still correct, only coarser).
const MAX_DIFF_CELLS: usize = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Add,
    Remove,
}

/// One block of added or removed lines from one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    /// The lines, without line endings.
    pub lines: Vec<String>,
    /// Up to two lines before the item in the group's text, nearest last.
    #[serde(default)]
    pub after: Vec<String>,
    /// Up to two lines after it, nearest first.
    #[serde(default)]
    pub before: Vec<String>,
    /// Line index in the text it was diffed in (a tie-breaker when context repeats).
    #[serde(default)]
    pub pos: usize,
    /// GitHub login of the commit's author; empty when unknown.
    pub author: String,
    pub commit: String,
    pub message: String,
    pub at_unix: u64,
}

impl Item {
    fn is_inverse_of(&self, other: &Item) -> bool {
        self.kind != other.kind && self.lines == other.lines
    }
}

/// One step of the group's history between two syncs: the `CLAUDE.md` text after `commit`.
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

/// `groups/<g>/claude-md-review.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReviewState {
    pub format: u32,
    /// This device's choice on "Review changes others make to CLAUDE.md". `None` follows
    /// the group's `autoAcceptClaudeMd`. Off: others' changes apply at once.
    pub review: Option<bool>,
    /// The group's text as last seen.
    pub group: String,
    /// The text the copy should hold (as the app last wrote it).
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
            pending: Vec::new(),
            declined: Vec::new(),
        }
    }
}

/// What one `pull` did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pulled {
    /// Items that now wait for the user.
    pub new: Vec<Item>,
    /// Items applied to `accepted` (this account's, or everyone's with review off), in order.
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

    /// `None` when the file does not exist.
    pub fn load(file: &Path) -> crate::Result<Option<Self>> {
        let bytes = match std::fs::read(file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            r => r?,
        };
        let mut state: Self = serde_json::from_slice(&bytes)?;
        if state.format > FORMAT {
            return Err(crate::Error::Invalid(format!(
                "{} was written by a newer claude-sync (format {})",
                file.display(),
                state.format
            )));
        }
        // Format 1 held a per-device bool whose default was on: an explicit choice can't be
        // told from the default, so the device follows the group.
        if state.format < 2 {
            state.review = None;
            state.format = FORMAT;
        }
        Ok(Some(state))
    }

    /// Atomic (temp file + rename).
    pub fn save(&self, file: &Path) -> crate::Result<()> {
        write_json(
            file,
            &Self {
                format: FORMAT,
                ..self.clone()
            },
        )
    }

    /// The group's text moved on through `steps`. This account's items (and everyone's when
    /// review is off) are applied to `accepted`; the rest wait in `pending`.
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
                // Undoing a change still waiting for review: neither is left to decide.
                if let Some(i) = self.pending.iter().position(|p| p.is_inverse_of(&item)) {
                    self.pending.remove(i);
                    out.new.retain(|p| !p.is_inverse_of(&item));
                    continue;
                }
                let patched = apply(&self.accepted, &item);
                if step.mine || !self.review.unwrap_or(true) {
                    match patched {
                        Some(text) => {
                            self.accepted = text;
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
        // In step before and everything applied: take the group's text as it is, so the
        // patching never drifts from it.
        if in_step && all_applied && self.pending.is_empty() {
            self.accepted = prev.clone();
        }
        self.group = prev;
        self.settle();
        out
    }

    /// Accept pending items (`None`: all). Returns the items applied.
    pub fn accept(&mut self, ids: Option<&[String]>) -> Vec<Item> {
        let mut applied = Vec::new();
        for item in self.take(ids) {
            if let Some(text) = apply(&self.accepted, &item) {
                self.accepted = text;
                applied.push(item);
            }
        }
        self.settle();
        applied
    }

    /// Decline pending items (`None`: all): they are never applied nor asked again.
    pub fn decline(&mut self, ids: Option<&[String]>, now: u64) -> usize {
        let items = self.take(ids);
        for item in &items {
            self.declined.push(Declined {
                id: item.id.clone(),
                at_unix: now,
            });
        }
        let excess = self.declined.len().saturating_sub(MAX_DECLINED);
        self.declined.drain(..excess);
        items.len()
    }

    /// Decline one line of the accepted text for this user only (a clash with another
    /// group, docs/architecture.md §7b): it is taken out of `accepted` and remembered with
    /// the declined items, so "Use the group's version" brings it back. Lines compare
    /// without surrounding spaces. `None` when the text has no such line.
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
        self.declined.push(Declined {
            id: item.id.clone(),
            at_unix: now,
        });
        let excess = self.declined.len().saturating_sub(MAX_DECLINED);
        self.declined.drain(..excess);
        self.settle();
        Some(item)
    }

    /// "Use the group's version": the copy follows the group again, decisions forgotten.
    pub fn reset(&mut self) {
        self.accepted = self.group.clone();
        self.pending.clear();
        self.declined.clear();
    }

    fn take(&mut self, ids: Option<&[String]>) -> Vec<Item> {
        let (taken, kept) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|i| ids.is_none_or(|ids| ids.contains(&i.id)));
        self.pending = kept;
        taken
    }

    /// Pending items with nothing left to do (the same change arrived another way).
    pub(crate) fn settle(&mut self) {
        let accepted = &self.accepted;
        self.pending.retain(|i| apply(accepted, i).is_some());
    }
}

/// The user's own edits to their copy (`accepted` -> `copy`) applied to `onto`. Used to share
/// a local edit (onto the group's text, so declined differences stay out of the push) and
/// to keep an unshared edit while accepted items change the copy.
pub fn rebase(from: &str, to: &str, onto: &str) -> String {
    let step = Step {
        text: to.into(),
        mine: true,
        author: String::new(),
        commit: String::new(),
        message: String::new(),
        at_unix: 0,
    };
    apply_all(onto, &items_between(from, to, &step))
}

/// The desktop notification for new pending items of one group, e.g. "ana changed Friends's
/// CLAUDE.md: 2 additions, 1 removal to review". `None` when there is nothing to review.
pub fn notification(group: &str, items: &[Item]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut authors: Vec<&str> = Vec::new();
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
        [one] => one.to_string(),
        [a, b] => format!("{a} and {b}"),
        [a, rest @ ..] => format!("{a} and {} others", rest.len()),
        [] => unreachable!(),
    };
    let count = |kind: Kind, one: &str, many: &str| {
        let n = items.iter().filter(|i| i.kind == kind).count();
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

/// `items` patched into `text` in order; items with nothing to do are skipped.
pub fn apply_all(text: &str, items: &[Item]) -> String {
    items
        .iter()
        .fold(text.to_string(), |t, i| apply(&t, i).unwrap_or(t))
}

/// `item` patched into `text`; `None` when there is nothing to do (the lines to remove are
/// gone, or the lines to add are already there).
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
            .take_while(|(want, i)| keys[*i] == want.as_str())
            .count();
        let before = item
            .before
            .iter()
            .zip(end..keys.len())
            .take_while(|(want, i)| keys[*i] == want.as_str())
            .count();
        after + before
    };
    let best = |candidates: Vec<usize>, width: usize| {
        candidates
            .into_iter()
            .max_by_key(|&p| (score(p, p + width), std::cmp::Reverse(p.abs_diff(item.pos))))
    };
    match item.kind {
        Kind::Remove => {
            let at = best((0..keys.len()).filter(|&p| matches_at(p)).collect(), k)?;
            let mut out = String::with_capacity(text.len());
            for l in lines[..at].iter().chain(&lines[at + k..]) {
                out.push_str(l);
            }
            Some(out)
        }
        Kind::Add => {
            let at = best((0..=keys.len()).collect(), 0).unwrap_or(keys.len());
            if matches_at(at) || (at >= k && matches_at(at - k)) {
                return None;
            }
            let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
            let mut out = String::with_capacity(text.len() + k * 40);
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
            for l in &lines[at..] {
                out.push_str(l);
            }
            Some(out)
        }
    }
}

/// Lines with their endings.
fn split(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// A line without its ending: what items compare.
fn key(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

/// The items that turn `old` into `new`, attributed to `step`. Each changed block gives a
/// removal (its old lines) and an addition (its new lines).
pub fn items_between(old: &str, new: &str, step: &Step) -> Vec<Item> {
    let a: Vec<&str> = split(old).into_iter().map(key).collect();
    let b: Vec<&str> = split(new).into_iter().map(key).collect();
    let mut out = Vec::new();
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
        let mut push = |kind: Kind, lines: &[&str]| {
            if lines.is_empty() {
                return;
            }
            let lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
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
        };
        push(Kind::Remove, &a[h.a..h.a + h.del]);
        push(Kind::Add, &b[h.b..h.b + h.add]);
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
    let text = serde_json::to_string(&(commit, kind, lines, after, before)).unwrap_or_default();
    crate::apply::sha256_hex(text.as_bytes())[..16].to_string()
}

/// A changed block: `del` lines at `a` in the old text became `add` lines at `b` in the new.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hunk {
    a: usize,
    del: usize,
    b: usize,
    add: usize,
}

/// Line diff (longest common subsequence on the part between the common prefix and suffix).
fn hunks(a: &[&str], b: &[&str]) -> Vec<Hunk> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    if ma.is_empty() && mb.is_empty() {
        return Vec::new();
    }
    let (n, m) = (ma.len(), mb.len());
    if n == 0 || m == 0 || n.saturating_mul(m) > MAX_DIFF_CELLS {
        return vec![Hunk {
            a: pre,
            del: n,
            b: pre,
            add: m,
        }];
    }
    // lcs[i][j]: length of the LCS of ma[i..] and mb[j..].
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
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut open: Option<Hunk> = None;
    let mut close = |open: &mut Option<Hunk>| {
        if let Some(h) = open.take() {
            out.push(h);
        }
    };
    while i < n || j < m {
        if i < n && j < m && ma[i] == mb[j] {
            close(&mut open);
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
    close(&mut open);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(text: &str, author: &str, mine: bool, commit: &str) -> Step {
        Step {
            text: text.into(),
            mine,
            author: author.into(),
            commit: commit.into(),
            message: format!("{author}: edit"),
            at_unix: 10,
        }
    }

    fn kinds(items: &[Item]) -> Vec<(Kind, Vec<String>)> {
        items.iter().map(|i| (i.kind, i.lines.clone())).collect()
    }

    fn ids(items: &[Item]) -> Vec<String> {
        items.iter().map(|i| i.id.clone()).collect()
    }

    const BASE: &str = "# Rules\n- one\n- two\n- three\n";

    #[test]
    fn diff_splits_blocks_into_additions_and_removals() {
        let s = step("# Rules\n- one\n- 2\n- three\n- four\n", "ana", false, "c2");
        let items = items_between(BASE, &s.text, &s);
        assert_eq!(
            kinds(&items),
            [
                (Kind::Remove, vec!["- two".to_string()]),
                (Kind::Add, vec!["- 2".to_string()]),
                (Kind::Add, vec!["- four".to_string()]),
            ]
        );
        assert_eq!(items[0].after, ["# Rules", "- one"]);
        assert_eq!(items[0].before, ["- three"]);
        assert_eq!(items[2].author, "ana");
        assert_ne!(items[1].id, items[2].id);
        assert_eq!(
            apply_all(BASE, &items),
            s.text,
            "patching gives the new text"
        );
    }

    #[test]
    fn patches_follow_content_not_line_numbers() {
        let s = step(
            "# Rules\n- one\n- two\n- new\n- three\n",
            "ana",
            false,
            "c2",
        );
        let add = &items_between(BASE, &s.text, &s)[0];
        // The user's copy has extra lines on top and lost "- one".
        let mine = "intro\nmore\n# Rules\n- two\n- three\n";
        assert_eq!(
            apply(mine, add).unwrap(),
            "intro\nmore\n# Rules\n- two\n- new\n- three\n"
        );
        assert_eq!(apply(&s.text, add), None, "already there");
        let s = step("# Rules\n- one\n- three\n", "ana", false, "c3");
        let remove = &items_between(BASE, &s.text, &s)[0];
        assert_eq!(
            apply(mine, remove).unwrap(),
            "intro\nmore\n# Rules\n- three\n"
        );
        assert_eq!(apply("# Rules\n", remove), None, "already gone");
    }

    #[test]
    fn line_endings_are_kept() {
        let base = "a\r\nb\r\n";
        let s = step("a\r\nx\r\nb\r\n", "ana", false, "c2");
        let item = &items_between(base, &s.text, &s)[0];
        assert_eq!(apply(base, item).unwrap(), "a\r\nx\r\nb\r\n");
        let s = step("a\nb\nc\n", "ana", false, "c2");
        let item = &items_between("a\nb\n", &s.text, &s)[0];
        assert_eq!(
            apply("a\nb", item).unwrap(),
            "a\nb\nc\n",
            "no newline at the end"
        );
    }

    #[test]
    fn others_wait_and_mine_apply() {
        let mut st = ReviewState::new(BASE);
        let theirs = step(
            "# Rules\n- one\n- two\n- three\n- four\n",
            "ana",
            false,
            "c2",
        );
        let got = st.pull(std::slice::from_ref(&theirs));
        assert_eq!(kinds(&got.new), [(Kind::Add, vec!["- four".to_string()])]);
        assert!(got.applied.is_empty());
        assert_eq!(st.accepted, BASE, "not applied");
        assert_eq!(st.group, theirs.text);

        let mine = step("# Rules\n- two\n- three\n- four\n", "bo", true, "c3");
        let got = st.pull(&[mine]);
        assert!(got.new.is_empty());
        assert_eq!(
            kinds(&got.applied),
            [(Kind::Remove, vec!["- one".to_string()])]
        );
        assert_eq!(st.accepted, "# Rules\n- two\n- three\n");
        assert_eq!(st.pending.len(), 1, "ana's line still waits");
    }

    #[test]
    fn accept_applies_and_decline_keeps_and_is_not_asked_again() {
        let mut st = ReviewState::new(BASE);
        let s = step("# Rules\n- one\n- three\n- four\n", "ana", false, "c2");
        st.pull(&[s]);
        assert_eq!(st.pending.len(), 2);
        let remove = st.pending[0].id.clone();
        let add = st.pending[1].id.clone();

        assert_eq!(st.decline(Some(std::slice::from_ref(&remove)), 5), 1);
        assert_eq!(st.accepted, BASE, "declined removal keeps the line");
        let applied = st.accept(Some(&[add]));
        assert_eq!(applied.len(), 1);
        assert_eq!(st.accepted, "# Rules\n- one\n- two\n- three\n- four\n");
        assert!(st.pending.is_empty());

        // The same history seen again (a retried pass) asks nothing.
        let mut again = st.clone();
        again.group = BASE.into();
        again.accepted = st.accepted.clone();
        let got = again.pull(&[step(
            "# Rules\n- one\n- three\n- four\n",
            "ana",
            false,
            "c2",
        )]);
        assert!(got.new.is_empty(), "{:?}", got.new);
        assert!(again.pending.is_empty());

        // Unrelated later changes do not bring the declined removal back.
        let got = st.pull(&[step(
            "# Rules\n- one\n- three\n- four\n- five\n",
            "ana",
            false,
            "c3",
        )]);
        assert_eq!(kinds(&got.new), [(Kind::Add, vec!["- five".to_string()])]);
        assert!(st.accepted.contains("- two"));
        // A new change in the same place is a new question.
        let got = st.pull(&[step(
            "# Rules\n- one\n- two\n- three\n- four\n- five\n",
            "ana",
            false,
            "c4",
        )]);
        assert!(
            got.new.is_empty(),
            "re-adding a line the user kept is nothing to do"
        );
    }

    #[test]
    fn a_change_undone_before_review_disappears() {
        let mut st = ReviewState::new(BASE);
        let added = step("# Rules\n- one\n- two\n- three\n- x\n", "ana", false, "c2");
        let undone = step(BASE, "carl", false, "c3");
        let got = st.pull(&[added, undone]);
        assert!(got.new.is_empty());
        assert!(st.pending.is_empty());
        assert_eq!(st.accepted, BASE);
    }

    #[test]
    fn review_off_applies_everyone_but_keeps_past_declines() {
        let mut st = ReviewState::new(BASE);
        st.pull(&[step("# Rules\n- one\n- three\n", "ana", false, "c2")]);
        st.decline(None, 1);
        st.review = Some(false);
        let got = st.pull(&[step(
            "# Rules\n- one\n- three\n- four\n",
            "ana",
            false,
            "c3",
        )]);
        assert!(got.new.is_empty());
        assert_eq!(st.accepted, "# Rules\n- one\n- two\n- three\n- four\n");
        st.reset();
        assert_eq!(st.accepted, st.group);
        assert!(st.declined.is_empty());
    }

    #[test]
    fn in_step_copies_take_the_group_text_exactly() {
        let mut st = ReviewState::new("a\na\nb\n");
        st.review = Some(false);
        let s = step("a\nb\na\na\nb\n", "ana", false, "c2");
        st.pull(std::slice::from_ref(&s));
        assert_eq!(st.accepted, s.text);
    }

    #[test]
    fn rebase_moves_only_the_users_own_edit() {
        // The user declined ana's removal of "- two" and then added "- mine".
        let accepted = "# Rules\n- one\n- two\n- three\n";
        let copy = "# Rules\n- one\n- two\n- three\n- mine\n";
        let group = "# Rules\n- one\n- three\n";
        assert_eq!(
            rebase(accepted, copy, group),
            "# Rules\n- one\n- three\n- mine\n"
        );
        assert_eq!(
            rebase(accepted, accepted, group),
            group,
            "no edit, nothing to push"
        );
    }

    #[test]
    fn state_round_trips_atomically_and_refuses_newer_formats() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("g").join(FILE);
        assert_eq!(ReviewState::load(&file).unwrap(), None);
        let mut st = ReviewState::new(BASE);
        st.pull(&[step("# Rules\n- one\n", "ana", false, "c2")]);
        st.decline(Some(&ids(&st.pending)[..1]), 7);
        st.save(&file).unwrap();
        assert_eq!(ReviewState::load(&file).unwrap(), Some(st.clone()));
        // No temp file is left next to it.
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [FILE]);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"format\": 2"), "{text}");

        std::fs::write(&file, r#"{"format": 3}"#).unwrap();
        assert!(ReviewState::load(&file).is_err());
        std::fs::write(&file, r#"{"format": 1, "group": "x"}"#).unwrap();
        let old = ReviewState::load(&file).unwrap().unwrap();
        assert_eq!(old.review, None, "a format 1 file follows the group");
        assert_eq!(old.format, FORMAT);
        std::fs::write(&file, r#"{"format": 1, "review": false}"#).unwrap();
        assert_eq!(ReviewState::load(&file).unwrap().unwrap().review, None);
        std::fs::write(&file, r#"{"format": 2, "review": false}"#).unwrap();
        assert_eq!(
            ReviewState::load(&file).unwrap().unwrap().review,
            Some(false)
        );
    }

    #[test]
    fn large_rewrites_fall_back_to_one_block() {
        let a: String = (0..3000).map(|i| format!("a{i}\n")).collect();
        let b: String = (0..3000).map(|i| format!("b{i}\n")).collect();
        let s = step(&b, "ana", false, "c2");
        let items = items_between(&a, &b, &s);
        assert_eq!(items.len(), 2);
        assert_eq!(apply_all(&a, &items), b);
    }

    #[test]
    fn a_dropped_line_stays_out_until_the_group_version_is_used() {
        let text = "# G
- Use tabs
- Run tests
";
        let mut st = ReviewState::new(text);
        assert_eq!(st.drop_line("nothing like it", 5), None);
        let item = st.drop_line("  - Use tabs ", 5).unwrap();
        assert_eq!(
            (
                item.kind,
                item.lines.clone(),
                st.accepted.as_str(),
                st.declined.len()
            ),
            (
                Kind::Remove,
                vec!["- Use tabs".to_string()],
                "# G
- Run tests
",
                1
            )
        );
        // Later changes by others still apply around it.
        let next = "# G
- Use tabs
- Run tests
- Be kind
";
        st.review = Some(false);
        st.pull(&[step(next, "cy", false, "c2")]);
        assert_eq!(
            st.accepted,
            "# G
- Run tests
- Be kind
"
        );
        st.reset();
        assert_eq!(st.accepted, next);
    }

    #[test]
    fn notification_counts_additions_and_removals() {
        let s = step(
            "x
y
", "ana", false, "c1",
        );
        let items = items_between(
            "a
b
", "a
x

z
", &s,
        );
        assert_eq!(
            notification("Friends", &items).unwrap(),
            "ana changed Friends's CLAUDE.md: 1 addition, 1 removal to review"
        );
        let mut two = items.clone();
        two[0].author = String::new();
        assert!(notification("F", &two)
            .unwrap()
            .starts_with("Someone and ana changed"));
        assert_eq!(notification("F", &[]), None);
    }
}
