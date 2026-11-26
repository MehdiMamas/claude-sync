//! Secret scanner: the same pattern set and names as v1 (`crates/core/src/secrets.rs`), so a
//! file that v1 refused to share is refused here. Parity rows: secret scan before every push
//! and share (never returns the matched text).
//!
//! v2-Parity rows covered: 29.
use regex::Regex;
use std::sync::LazyLock;

/// A line holding this marker is never reported by [`scan`].
pub const ALLOW_MARKER: &str = "claude-sync: allow-secret";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// 1-based.
    pub line: usize,
    pub pattern: &'static str,
}

/// Name and regex, longer and more specific first so a `github_pat` is not reported as
/// something else. At most one hit per line (the first pattern that matches).
pub const PATTERNS: &[(&str, &str)] = &[
    ("github_pat", r"github_pat_[A-Za-z0-9_]{50,}"),
    ("sk-ant", r"sk-ant-[A-Za-z0-9_-]{20,}"),
    ("ghp", r"ghp_[A-Za-z0-9]{36,}"),
    ("gh-token", r"gh[orsu]_[A-Za-z0-9]{36,}"),
    ("aws-access-key", r"AKIA[0-9A-Z]{16}"),
    ("slack-token", r"xox[abpr]-[A-Za-z0-9-]{10,}"),
    (
        "private-key",
        r"-----BEGIN[[:space:]][A-Z[:space:]]*PRIVATE[[:space:]]KEY-----",
    ),
    ("token-field", r#""(access|refresh)_token"[[:space:]]*:"#),
];

#[allow(clippy::expect_used)] // the patterns are constants checked by the tests
static COMPILED: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    PATTERNS
        .iter()
        .map(|(n, re)| (*n, Regex::new(re).expect("secret pattern")))
        .collect()
});

pub fn scan(text: &str) -> Vec<Hit> {
    text.split('\n')
        .enumerate()
        .filter(|(_, l)| !l.contains(ALLOW_MARKER))
        .filter_map(|(i, l)| {
            COMPILED
                .iter()
                .find(|(_, re)| re.is_match(l))
                .map(|(pattern, _)| Hit {
                    line: i + 1,
                    pattern,
                })
        })
        .collect()
}

/// The first hit's pattern name (for single values such as an `env` entry).
pub fn first_hit(text: &str) -> Option<&'static str> {
    scan(text).first().map(|h| h.pattern)
}
