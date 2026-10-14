//! Rules that clash across the user's groups (docs/architecture.md §7b).
//!
//! Deterministic, no model involved. Each `CLAUDE.md` Claude reads on this device (every
//! group's copy and the user's own text) is cut into rule lines; headings, code, imports
//! and comments are left out. Two lines from different sources are:
//! - a **duplicate** when they say the same thing: equal once case, list markers,
//!   punctuation and spacing are ignored, or equal apart from "always", "do", "must" and
//!   the like with the same sense ("Never push to main" and "Do not push to main");
//! - a **contradiction** when they are equal apart from those words but one is negated
//!   and the other is not ("Always use tabs" and "Never use tabs").
//!
//! Settings clash when two groups' `settings.shared.json` set the same plain value to
//! different values. Arrays are merged by union on apply, so they never clash.
use crate::GroupId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where a line or value comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Source {
    /// A group's `CLAUDE.md` copy or settings on this device.
    Group { id: GroupId },
    /// The user's own `~/.claude/CLAUDE.md` text (outside the app's block).
    Own,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Duplicate,
    Contradiction,
    Setting,
}

/// One side of a clash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    pub source: Source,
    /// The group's name, or "Your CLAUDE.md".
    pub name: String,
    /// The line as written, or the setting's value as JSON.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    /// Stable while both sides stay the same.
    pub id: String,
    pub kind: Kind,
    /// For a setting: its JSON pointer (`/model`).
    pub pointer: Option<String>,
    /// For a setting, the side that applies on this device (the group joined first).
    pub a: Side,
    pub b: Side,
}

/// One `CLAUDE.md` to compare.
#[derive(Debug, Clone)]
pub struct Text {
    pub source: Source,
    pub name: String,
    pub text: String,
}

/// One group's `settings.shared.json`, in join order.
#[derive(Debug, Clone)]
pub struct SettingsFile {
    pub id: GroupId,
    pub name: String,
    pub json: Value,
    /// Pointers this device keeps local for this group: they are not applied, so they do
    /// not clash.
    pub kept: Vec<String>,
}

/// Most clashes reported at once; the list is for a person to go through.
pub const MAX: usize = 200;

/// Words that only set how strongly or which way a rule is meant.
const FILLER: &[&str] = &[
    "always", "do", "does", "must", "should", "please", "you", "will", "can", "make", "sure", "be",
    "to", "ever", "remember", "ensure",
];
/// Words that turn a rule around.
const NEGATIONS: &[&str] = &[
    "never", "not", "dont", "doesnt", "cannot", "cant", "wont", "shouldnt", "mustnt", "avoid", "no",
];

/// A line read as a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    /// The line as written, trimmed.
    line: String,
    /// Lower case words, without list markers and punctuation.
    words: String,
    /// `words` without filler and negations: what the rule is about.
    key: String,
    negated: bool,
}

/// The rule lines of a `CLAUDE.md`: not headings, code blocks, imports, comments, tables
/// or lines of fewer than two words.
fn rules(text: &str) -> Vec<Rule> {
    let mut out = Vec::new();
    let mut fence = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence || skipped(line) {
            continue;
        }
        if let Some(rule) = rule(line) {
            out.push(rule);
        }
    }
    out
}

fn skipped(line: &str) -> bool {
    line.is_empty()
        || line.starts_with('#')
        || line.starts_with('@')
        || line.starts_with("<!--")
        || line.starts_with('|')
        || line.starts_with('>')
}

fn rule(line: &str) -> Option<Rule> {
    let body = strip_marker(line);
    let lower = body.to_lowercase().replace(['\'', '’'], "");
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() < 2 {
        return None;
    }
    let negated = words.iter().filter(|w| NEGATIONS.contains(w)).count() % 2 == 1;
    let key: Vec<&str> = words
        .iter()
        .copied()
        .filter(|w| !FILLER.contains(w) && !NEGATIONS.contains(w))
        .collect();
    Some(Rule {
        line: line.to_string(),
        words: words.join(" "),
        key: if key.len() >= 2 {
            key.join(" ")
        } else {
            String::new()
        },
        negated,
    })
}

/// `- x`, `* x`, `+ x`, `1. x`, `1) x` and `[ ] x` become `x`.
fn strip_marker(line: &str) -> &str {
    let mut s = line;
    if let Some(rest) = s.strip_prefix(['-', '*', '+']) {
        s = rest.trim_start();
    } else {
        let digits = s.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 {
            if let Some(rest) = s[digits..].strip_prefix(['.', ')']) {
                s = rest.trim_start();
            }
        }
    }
    for box_ in ["[ ]", "[x]", "[X]"] {
        if let Some(rest) = s.strip_prefix(box_) {
            s = rest.trim_start();
        }
    }
    s
}

/// How `a` and `b` clash, if they do.
fn clash(a: &Rule, b: &Rule) -> Option<Kind> {
    if a.words == b.words {
        return Some(Kind::Duplicate);
    }
    if a.key.is_empty() || a.key != b.key {
        return None;
    }
    Some(if a.negated == b.negated {
        Kind::Duplicate
    } else {
        Kind::Contradiction
    })
}

fn conflict_id(kind: Kind, pointer: Option<&str>, a: &Side, b: &Side) -> String {
    let text = serde_json::to_string(&(kind, pointer, a, b)).unwrap_or_default();
    crate::apply::sha256_hex(text.as_bytes())[..16].to_string()
}

fn conflict(kind: Kind, pointer: Option<String>, a: Side, b: Side) -> Conflict {
    Conflict {
        id: conflict_id(kind, pointer.as_deref(), &a, &b),
        kind,
        pointer,
        a,
        b,
    }
}

/// Lines that clash between different texts, in the order of `texts` (the earlier text is
/// side `a`). Each pair of lines is reported once.
pub fn claude_md(texts: &[Text]) -> Vec<Conflict> {
    let parsed: Vec<Vec<Rule>> = texts.iter().map(|t| rules(&t.text)).collect();
    let mut out = Vec::new();
    for (i, ta) in texts.iter().enumerate() {
        for (j, tb) in texts.iter().enumerate().skip(i + 1) {
            if ta.source == tb.source {
                continue;
            }
            for ra in &parsed[i] {
                for rb in &parsed[j] {
                    let Some(kind) = clash(ra, rb) else { continue };
                    let side = |t: &Text, r: &Rule| Side {
                        source: t.source.clone(),
                        name: t.name.clone(),
                        text: r.line.clone(),
                    };
                    let c = conflict(kind, None, side(ta, ra), side(tb, rb));
                    if !out.iter().any(|x: &Conflict| x.id == c.id) {
                        out.push(c);
                    }
                    if out.len() >= MAX {
                        return out;
                    }
                }
            }
        }
    }
    out
}

/// How the lines of `incoming` clash with `texts` (the target group's and the others'):
/// the warnings shown before sharing or copying them. Side `a` is the incoming line.
pub fn incoming(incoming: &Text, texts: &[Text]) -> Vec<Conflict> {
    let mine = rules(&incoming.text);
    let mut out = Vec::new();
    for t in texts {
        for rb in rules(&t.text) {
            for ra in &mine {
                let Some(kind) = clash(ra, &rb) else { continue };
                out.push(conflict(
                    kind,
                    None,
                    Side {
                        source: incoming.source.clone(),
                        name: incoming.name.clone(),
                        text: ra.line.clone(),
                    },
                    Side {
                        source: t.source.clone(),
                        name: t.name.clone(),
                        text: rb.line.clone(),
                    },
                ));
                if out.len() >= MAX {
                    return out;
                }
            }
        }
    }
    out
}

/// The plain values of a settings object by JSON pointer. Arrays are left out (merged by
/// union on apply), objects are walked into.
pub fn leaves(v: &Value) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    walk(v, "", false, &mut out);
    out
}

/// Same, with arrays as values: what can be copied to another group one by one.
pub fn values(v: &Value) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    walk(v, "", true, &mut out);
    out
}

fn walk(v: &Value, at: &str, arrays: bool, out: &mut Vec<(String, Value)>) {
    match v {
        Value::Object(map) => {
            for (k, v) in map {
                walk(v, &crate::settings::join(at, k), arrays, out);
            }
        }
        Value::Array(_) if !arrays => {}
        v => out.push((at.to_string(), v.clone())),
    }
}

fn kept(file: &SettingsFile, pointer: &str) -> bool {
    file.kept
        .iter()
        .any(|k| pointer == k || pointer.starts_with(&format!("{k}/")))
}

/// Values two groups set differently, in join order: side `a` is the earlier group, whose
/// value applies on this device.
pub fn settings(files: &[SettingsFile]) -> Vec<Conflict> {
    let all: Vec<Vec<(String, Value)>> = files.iter().map(|f| leaves(&f.json)).collect();
    let mut out = Vec::new();
    for (i, fa) in files.iter().enumerate() {
        for (j, fb) in files.iter().enumerate().skip(i + 1) {
            for (ptr, va) in &all[i] {
                let Some((_, vb)) = all[j].iter().find(|(p, _)| p == ptr) else {
                    continue;
                };
                if va == vb || kept(fa, ptr) || kept(fb, ptr) {
                    continue;
                }
                let side = |f: &SettingsFile, v: &Value| Side {
                    source: Source::Group { id: f.id.clone() },
                    name: f.name.clone(),
                    text: v.to_string(),
                };
                out.push(conflict(
                    Kind::Setting,
                    Some(ptr.clone()),
                    side(fa, va),
                    side(fb, vb),
                ));
                if out.len() >= MAX {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn group(owner: &str, text: &str) -> Text {
        Text {
            source: Source::Group {
                id: GroupId::new(owner, "claude-sync-data"),
            },
            name: owner.into(),
            text: text.into(),
        }
    }

    fn own(text: &str) -> Text {
        Text {
            source: Source::Own,
            name: "Your CLAUDE.md".into(),
            text: text.into(),
        }
    }

    fn summary(cs: &[Conflict]) -> Vec<(Kind, String, String, String, String)> {
        cs.iter()
            .map(|c| {
                (
                    c.kind,
                    c.a.name.clone(),
                    c.a.text.clone(),
                    c.b.name.clone(),
                    c.b.text.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn lines_are_read_as_rules() {
        let r = rules(
            "# Rules\n\n- Always use tabs.\n```\nnever use tabs\n```\n@~/x/CLAUDE.md\n\
             <!-- note -->\n| a | b |\n> quote here\n2) Don't push to main!\n- [x] Run the tests\nOk\n",
        );
        let got: Vec<(&str, &str, bool)> = r
            .iter()
            .map(|r| (r.words.as_str(), r.key.as_str(), r.negated))
            .collect();
        assert_eq!(
            got,
            [
                ("always use tabs", "use tabs", false),
                ("dont push to main", "push main", true),
                ("run the tests", "run the tests", false),
            ]
        );
    }

    #[test]
    fn duplicates_and_contradictions_across_sources() {
        let a = group(
            "ana",
            "- Always use tabs.\n- Never push to main\n- Answer in French\n- Write tests first\n",
        );
        let b = group(
            "cy",
            "* always use TABS\n* Do not push to main.\n* Never answer in French\n* Use spaces\n",
        );
        let me = own("Write tests first!\n");
        let got = summary(&claude_md(&[a, b, me]));
        assert_eq!(
            got,
            [
                (
                    Kind::Duplicate,
                    "ana".into(),
                    "- Always use tabs.".into(),
                    "cy".into(),
                    "* always use TABS".into()
                ),
                (
                    Kind::Duplicate,
                    "ana".into(),
                    "- Never push to main".into(),
                    "cy".into(),
                    "* Do not push to main.".into()
                ),
                (
                    Kind::Contradiction,
                    "ana".into(),
                    "- Answer in French".into(),
                    "cy".into(),
                    "* Never answer in French".into()
                ),
                (
                    Kind::Duplicate,
                    "ana".into(),
                    "- Write tests first".into(),
                    "Your CLAUDE.md".into(),
                    "Write tests first!".into()
                ),
            ]
        );
    }

    #[test]
    fn lines_within_one_source_never_clash() {
        let a = group("ana", "- Use tabs\n- Never use tabs\n- Use tabs\n");
        assert!(claude_md(&[a.clone(), a]).is_empty());
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        let a = group("ana", "- Use tabs\n- Run tests\n");
        let b = group("cy", "- Never use tabs\n- Run tests\n");
        let one = claude_md(&[a.clone(), b.clone()]);
        let two = claude_md(&[a, b]);
        assert_eq!(one, two);
        assert_eq!(one.len(), 2);
        assert_ne!(one[0].id, one[1].id);
    }

    #[test]
    fn incoming_lines_are_checked_against_every_text() {
        let incoming_text = group("ana", "- Use tabs\n- Something new here\n");
        let target = group("cy", "- Never use tabs\n");
        let other = own("Use tabs.\n");
        let got = summary(&incoming(&incoming_text, &[target, other]));
        assert_eq!(
            got,
            [
                (
                    Kind::Contradiction,
                    "ana".into(),
                    "- Use tabs".into(),
                    "cy".into(),
                    "- Never use tabs".into()
                ),
                (
                    Kind::Duplicate,
                    "ana".into(),
                    "- Use tabs".into(),
                    "Your CLAUDE.md".into(),
                    "Use tabs.".into()
                ),
            ]
        );
    }

    #[test]
    fn settings_clash_on_plain_values_only() {
        let file = |owner: &str, json: Value, kept: &[&str]| SettingsFile {
            id: GroupId::new(owner, "claude-sync-data"),
            name: owner.into(),
            json,
            kept: kept.iter().map(|k| k.to_string()).collect(),
        };
        let a = file(
            "ana",
            json!({"model": "opus", "permissions": {"allow": ["a"], "defaultMode": "plan"}, "env": {"X": "1"}, "same": true}),
            &[],
        );
        let b = file(
            "cy",
            json!({"model": "sonnet", "permissions": {"allow": ["b"], "defaultMode": "acceptEdits"}, "env": {"X": "2"}, "same": true}),
            &["/env"],
        );
        let got: Vec<(Option<String>, String, String)> = settings(&[a, b])
            .into_iter()
            .map(|c| (c.pointer, c.a.text, c.b.text))
            .collect();
        assert_eq!(
            got,
            [
                (
                    Some("/model".into()),
                    "\"opus\"".into(),
                    "\"sonnet\"".into()
                ),
                (
                    Some("/permissions/defaultMode".into()),
                    "\"plan\"".into(),
                    "\"acceptEdits\"".into()
                ),
            ]
        );
    }
}
