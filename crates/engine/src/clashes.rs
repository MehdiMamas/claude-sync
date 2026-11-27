//! Lines of CLAUDE.md that clash across the texts Claude reads on this device (v1
//! conflicts.rs rules, byte for byte in what counts as a clash). Pure and
//! deterministic.
//!
//! A *duplicate* says the same thing: equal once case, list markers, punctuation and spacing
//! are ignored, or equal apart from filler words ("Never push to main", "Do not push to
//! main"). A *contradiction* is equal apart from a negation ("Always use tabs", "Never use
//! tabs"). Rule lines exclude headings, code blocks, imports, comments, tables, quotes and
//! lines of fewer than two words.
//!
//! v2-Parity rows covered: 38.
use serde::Serialize;

/// Most clashes reported at once.
pub const MAX: usize = 200;

const FILLER: &[&str] = &[
    "always", "do", "does", "must", "should", "please", "you", "will", "can", "make", "sure", "be",
    "to", "ever", "remember", "ensure",
];
const NEGATIONS: &[&str] = &[
    "never", "not", "dont", "doesnt", "cannot", "cant", "wont", "shouldnt", "mustnt", "avoid", "no",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Duplicate,
    Contradiction,
    /// Two groups set the same plain setting to different values.
    Setting,
}

/// One CLAUDE.md to compare. `source` is the group repo, `None` for the user's own text.
#[derive(Debug, Clone)]
pub struct Text {
    pub source: Option<String>,
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    pub source: Option<String>,
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Clash {
    pub kind: Kind,
    /// For a setting: its JSON pointer.
    pub target: Option<String>,
    pub a: Side,
    pub b: Side,
}

struct Rule {
    line: String,
    words: String,
    key: String,
    negated: bool,
}

fn rule(line: &str) -> Option<Rule> {
    let mut s = line;
    if let Some(r) = s.strip_prefix(['-', '*', '+']) {
        s = r.trim_start();
    } else {
        let d = s.chars().take_while(char::is_ascii_digit).count();
        if let Some(r) = s[d..].strip_prefix(['.', ')']).filter(|_| d > 0) {
            s = r.trim_start();
        }
    }
    for b in ["[ ]", "[x]", "[X]"] {
        s = s.strip_prefix(b).map_or(s, str::trim_start);
    }
    let lower = s.to_lowercase().replace(['\'', '’'], "");
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
        line: line.into(),
        words: words.join(" "),
        key: if key.len() >= 2 {
            key.join(" ")
        } else {
            String::new()
        },
        negated,
    })
}

fn rules(text: &str) -> Vec<Rule> {
    let mut fence = false;
    let mut out = vec![];
    for raw in text.lines() {
        let l = raw.trim();
        if l.starts_with("```") || l.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence || l.is_empty() || l.starts_with(['#', '@', '|', '>']) || l.starts_with("<!--") {
            continue;
        }
        out.extend(rule(l));
    }
    out
}

fn kind(a: &Rule, b: &Rule) -> Option<Kind> {
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

/// Clashing lines between different texts, in the order of `texts` (the earlier is side `a`).
pub fn clashes(texts: &[Text]) -> Vec<Clash> {
    let parsed: Vec<Vec<Rule>> = texts.iter().map(|t| rules(&t.text)).collect();
    let mut out: Vec<Clash> = vec![];
    for (i, ta) in texts.iter().enumerate() {
        for (j, tb) in texts.iter().enumerate().skip(i + 1) {
            if ta.source == tb.source {
                continue;
            }
            for ra in &parsed[i] {
                for rb in &parsed[j] {
                    let Some(kind) = kind(ra, rb) else { continue };
                    let side = |t: &Text, r: &Rule| Side {
                        source: t.source.clone(),
                        name: t.name.clone(),
                        text: r.line.clone(),
                    };
                    let c = Clash {
                        kind,
                        target: None,
                        a: side(ta, ra),
                        b: side(tb, rb),
                    };
                    if !out.contains(&c) {
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

/// A group's `settings.shared.json` as last synced.
pub struct SettingsFile {
    pub source: String,
    pub name: String,
    pub json: serde_json::Value,
}

/// The plain values of a settings object by JSON pointer. Arrays are left out (merged by union
/// on apply), objects are walked into.
fn leaves(v: &serde_json::Value, at: &str, out: &mut Vec<(String, serde_json::Value)>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, v) in m {
                let k = k.replace('~', "~0").replace('/', "~1");
                leaves(v, &format!("{at}/{k}"), out);
            }
        }
        serde_json::Value::Array(_) => {}
        v => out.push((at.to_string(), v.clone())),
    }
}

/// Values two groups set differently, in join order (v1): side `a` is the earlier group, whose
/// value applies on this device.
pub fn settings_clashes(files: &[SettingsFile]) -> Vec<Clash> {
    let all: Vec<Vec<(String, serde_json::Value)>> = files
        .iter()
        .map(|f| {
            let mut o = vec![];
            leaves(&f.json, "", &mut o);
            o
        })
        .collect();
    let mut out = vec![];
    for (i, fa) in files.iter().enumerate() {
        for (j, fb) in files.iter().enumerate().skip(i + 1) {
            for (ptr, va) in &all[i] {
                let Some((_, vb)) = all[j].iter().find(|(p, _)| p == ptr) else {
                    continue;
                };
                if va == vb {
                    continue;
                }
                let side = |f: &SettingsFile, v: &serde_json::Value| Side {
                    source: Some(f.source.clone()),
                    name: f.name.clone(),
                    text: v.to_string(),
                };
                out.push(Clash {
                    kind: Kind::Setting,
                    target: Some(ptr.clone()),
                    a: side(fa, va),
                    b: side(fb, vb),
                });
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

    fn t(source: Option<&str>, text: &str) -> Text {
        Text {
            source: source.map(String::from),
            name: source.unwrap_or("Your CLAUDE.md").into(),
            text: text.into(),
        }
    }

    #[test]
    fn duplicates_contradictions_and_skipped_lines() {
        let own = t(
            None,
            "# Rules\n- Always use tabs\n- Never push to main\n```\nuse tabs now\n```\nok\n",
        );
        let g = t(
            Some("a/b"),
            "* Never use tabs\n1. Do not push to main\n| use tabs now |\n",
        );
        let c = clashes(&[own, g]);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].kind, Kind::Contradiction);
        assert_eq!(c[1].kind, Kind::Duplicate);
        assert_eq!(c[0].b.source.as_deref(), Some("a/b"));
    }

    #[test]
    fn same_source_never_clashes_and_the_list_is_capped() {
        let one = t(Some("a/b"), "Never use tabs\nAlways use tabs\n");
        assert!(clashes(&[one.clone(), one]).is_empty());
        let many: String = (0..300)
            .map(|i| format!("keep rule number {i}\n"))
            .collect();
        let c = clashes(&[t(None, &many), t(Some("a/b"), &many)]);
        assert_eq!(c.len(), MAX);
    }

    #[test]
    fn two_groups_that_set_one_plain_value_differently_clash_and_arrays_never_do() {
        let f = |src: &str, j: serde_json::Value| SettingsFile {
            source: src.into(),
            name: src.into(),
            json: j,
        };
        let a = f(
            "a/x",
            serde_json::json!({"model": "opus", "env": {"K": "1"}, "list": [1]}),
        );
        let b = f(
            "b/x",
            serde_json::json!({"model": "sonnet", "env": {"K": "1"}, "list": [2]}),
        );
        let c = settings_clashes(&[a, b]);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kind, Kind::Setting);
        assert_eq!(c[0].target.as_deref(), Some("/model"));
        assert_eq!(
            (c[0].a.text.as_str(), c[0].b.text.as_str()),
            ("\"opus\"", "\"sonnet\"")
        );
        assert_eq!(
            c[0].a.source.as_deref(),
            Some("a/x"),
            "the earlier group is side a"
        );
    }
}
