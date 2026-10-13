//! Secret scan and redaction (§9). Patterns ported from v0 lib/secrets.sh.
use regex::Regex;
use std::sync::LazyLock;

/// A line holding this marker is never reported by [`scan`].
pub const ALLOW_MARKER: &str = "claude-sync: allow-secret";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub line: usize,
    pub pattern: &'static str,
}

/// v0's patterns, same names and order: longer, more specific ones first so a
/// `github_pat` is not reported as something else.
const PATTERNS: &[(&str, &str)] = &[
    ("github_pat", r"github_pat_[A-Za-z0-9_]{50,}"),
    ("sk-ant", r"sk-ant-[A-Za-z0-9_-]{20,}"),
    ("ghp", r"ghp_[A-Za-z0-9]{36}"),
    ("gh-token", r"gh[osu]_[A-Za-z0-9]{36}"),
    ("aws-access-key", r"AKIA[0-9A-Z]{16}"),
    ("slack-token", r"xox[abpr]-[A-Za-z0-9-]{10,}"),
    (
        "private-key",
        r"-----BEGIN[[:space:]][A-Z[:space:]]*PRIVATE[[:space:]]KEY-----",
    ),
    ("token-field", r#""(access|refresh)_token"[[:space:]]*:"#),
];

static SCAN: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    PATTERNS
        .iter()
        .map(|(name, re)| (*name, Regex::new(re).expect("secret pattern")))
        .collect()
});

/// Redaction rules: `(name, regex, replacement)`. The replacement keeps the
/// non-secret prefix (`$1`) where there is one, so a redacted log still reads.
static REDACT: LazyLock<Vec<(&'static str, Regex, String)>> = LazyLock::new(|| {
    let mut rules = Vec::new();
    // A private key: the whole block, or everything after a header with no end line.
    rules.push((
        "private-key",
        Regex::new(
            r"-----BEGIN[[:space:]][A-Z[:space:]]*PRIVATE[[:space:]]KEY-----(?s:.*?)(-----END[[:space:]][A-Z[:space:]]*PRIVATE[[:space:]]KEY-----|\z)",
        )
        .expect("redact pattern"),
        "[redacted:private-key]".to_string(),
    ));
    // Token fields: keep the key, drop the value (quoted or bare).
    rules.push((
        "token-field",
        Regex::new(
            r#"("(?:access|refresh)_token"[[:space:]]*:[[:space:]]*)("(?:[^"\\]|\\.)*"|[^,}\s]+)?"#,
        )
        .expect("redact pattern"),
        "${1}\"[redacted:token-field]\"".to_string(),
    ));
    for (name, re) in PATTERNS
        .iter()
        .filter(|(n, _)| !matches!(*n, "private-key" | "token-field"))
    {
        rules.push((
            name,
            Regex::new(re).expect("redact pattern"),
            format!("[redacted:{name}]"),
        ));
    }
    // HTTP credentials, whatever the token looks like.
    rules.push((
        "authorization",
        Regex::new(r"(?i)(authorization[[:space:]]*[:=][[:space:]]*(?:token|basic)[[:space:]]+)[^[:space:]\x22',;]+")
            .expect("redact pattern"),
        "${1}[redacted:authorization]".to_string(),
    ));
    rules.push((
        "bearer",
        Regex::new(r"(?i)(bearer[[:space:]]+)[A-Za-z0-9._~+/=-]{8,}").expect("redact pattern"),
        "${1}[redacted:bearer]".to_string(),
    ));
    rules
});

/// Hits in `text` (1-based lines), at most one per line (the first pattern that
/// matches, as in v0). Lines containing `claude-sync: allow-secret` are skipped.
/// Never returns the matched text.
pub fn scan(text: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        if line.contains(ALLOW_MARKER) {
            continue;
        }
        if let Some((name, _)) = SCAN.iter().find(|(_, re)| re.is_match(line)) {
            hits.push(Hit {
                line: i + 1,
                pattern: name,
            });
        }
    }
    hits
}

/// The first hit's pattern name, if any (for single values such as an `env` entry).
pub fn first_hit(text: &str) -> Option<&'static str> {
    scan(text).first().map(|h| h.pattern)
}

/// `text` with every match replaced by `[redacted:<pattern>]`. Also redacts
/// `Bearer <token>` and `Authorization: token|basic <x>`. The allow marker does not
/// apply here: logs and feedback are always redacted.
pub fn redact(text: &str) -> String {
    let mut out = std::borrow::Cow::Borrowed(text);
    for (_, re, with) in REDACT.iter() {
        if re.is_match(&out) {
            out = std::borrow::Cow::Owned(re.replace_all(&out, with.as_str()).into_owned());
        }
    }
    out.into_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn rep(c: char, n: usize) -> String {
        std::iter::repeat_n(c, n).collect()
    }

    /// One sample per v0 pattern (tests/secrets-block.test.sh).
    pub(crate) fn samples() -> Vec<(String, &'static str)> {
        vec![
            (format!("sk-ant-{}", rep('a', 20)), "sk-ant"),
            (format!("ghp_{}", rep('a', 36)), "ghp"),
            (format!("github_pat_{}", rep('a', 50)), "github_pat"),
            (format!("gho_{}", rep('a', 36)), "gh-token"),
            (format!("ghs_{}", rep('b', 36)), "gh-token"),
            (format!("ghu_{}", rep('c', 36)), "gh-token"),
            (format!("AKIA{}", rep('A', 16)), "aws-access-key"),
            (format!("xoxb-{}", rep('a', 10)), "slack-token"),
            (format!("xoxp-{}", rep('1', 12)), "slack-token"),
            ("-----BEGIN RSA PRIVATE KEY-----".into(), "private-key"),
            ("-----BEGIN OPENSSH PRIVATE KEY-----".into(), "private-key"),
            (r#""access_token":"#.into(), "token-field"),
            (r#""refresh_token"  :"#.into(), "token-field"),
        ]
    }

    #[test]
    fn every_v0_pattern_is_found() {
        for (secret, name) in samples() {
            assert_eq!(
                scan(&secret),
                vec![Hit {
                    line: 1,
                    pattern: name
                }],
                "{name}"
            );
        }
    }

    #[test]
    fn line_numbers_are_one_based_and_every_line_reports() {
        let a = format!("ghp_{}", rep('c', 36));
        let b = format!("sk-ant-{}", rep('b', 20));
        let text = format!("one\ntwo\n{a}\n{b}\n");
        assert_eq!(
            scan(&text),
            vec![
                Hit {
                    line: 3,
                    pattern: "ghp"
                },
                Hit {
                    line: 4,
                    pattern: "sk-ant"
                }
            ]
        );
        // CRLF files count lines the same way.
        assert_eq!(scan(&text.replace('\n', "\r\n"))[0].line, 3);
    }

    #[test]
    fn github_pat_is_not_reported_as_something_else() {
        let pat = format!("github_pat_{}", rep('a', 60));
        assert_eq!(scan(&pat)[0].pattern, "github_pat");
    }

    #[test]
    fn allow_marker_skips_only_its_line() {
        let marked = format!("sk-ant-{} {ALLOW_MARKER}", rep('b', 20));
        assert!(scan(&marked).is_empty());
        let bad = format!("ghp_{}", rep('c', 36));
        assert_eq!(
            scan(&format!("{marked}\n{bad}")),
            vec![Hit {
                line: 2,
                pattern: "ghp"
            }]
        );
    }

    #[test]
    fn near_misses_are_not_hits() {
        for clean in [
            format!("ghp_{}", rep('a', 35)),
            format!("sk-ant-{}", rep('a', 19)),
            format!("AKIA{}", rep('a', 16)),
            format!("xoxz-{}", rep('a', 10)),
            "-----BEGIN PUBLIC KEY-----".to_string(),
            r#""token": "x""#.to_string(),
            "access_token is a word".to_string(),
        ] {
            assert!(scan(&clean).is_empty(), "{clean}");
        }
    }

    #[test]
    fn redact_removes_every_pattern() {
        for (secret, name) in samples() {
            let out = redact(&format!("before {secret} after"));
            assert!(out.contains(&format!("[redacted:{name}]")), "{name}: {out}");
            assert!(out.starts_with("before "), "{name}: {out}");
            if !matches!(name, "token-field" | "private-key") {
                assert!(!out.contains(&secret), "{name}: {out}");
                assert!(out.ends_with(" after"), "{name}: {out}");
            }
        }
    }

    #[test]
    fn redact_token_field_values() {
        let out =
            redact(r#"{"access_token": "abc123", "refresh_token":"def\"456", "scope":"repo"}"#);
        assert!(!out.contains("abc123") && !out.contains("def"), "{out}");
        assert!(
            out.contains(r#""access_token": "[redacted:token-field]""#),
            "{out}"
        );
        assert!(out.contains(r#""scope":"repo""#), "{out}");
        let bare = redact(r#"{"access_token":abc123}"#);
        assert!(!bare.contains("abc123"), "{bare}");
    }

    #[test]
    fn redact_private_key_block() {
        let key =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEsecretbody\n-----END RSA PRIVATE KEY-----\nnext";
        let out = redact(key);
        assert_eq!(out, "[redacted:private-key]\nnext");
        let open = redact("x -----BEGIN EC PRIVATE KEY-----\nMIIEsecret");
        assert_eq!(open, "x [redacted:private-key]");
    }

    #[test]
    fn redact_http_credentials() {
        let out =
            redact("Authorization: Bearer gho_short.tok-en123 and authorization: token abcdef0123");
        assert!(
            !out.contains("gho_short") && !out.contains("abcdef0123"),
            "{out}"
        );
        assert!(out.contains("Bearer [redacted:bearer]"), "{out}");
        assert!(
            out.contains("authorization: token [redacted:authorization]"),
            "{out}"
        );
        assert_eq!(
            redact("Authorization: Basic dXNlcjpwYXNz"),
            "Authorization: Basic [redacted:authorization]"
        );
        // Ordinary prose is left alone.
        assert_eq!(redact("the bearer of bad news"), "the bearer of bad news");
    }

    #[test]
    fn redact_ignores_allow_marker_and_keeps_clean_text() {
        let s = format!("sk-ant-{} {ALLOW_MARKER}", rep('b', 20));
        assert!(!redact(&s).contains("sk-ant-b"));
        assert_eq!(redact("nothing here\r\n"), "nothing here\r\n");
    }
}
