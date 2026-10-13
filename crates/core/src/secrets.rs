//! Secret scan and redaction (§9). Patterns ported from v0 lib/secrets.sh.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub line: usize,
    pub pattern: &'static str,
}

/// Hits in `text` (1-based lines). Lines containing `claude-sync: allow-secret` are skipped.
pub fn scan(_text: &str) -> Vec<Hit> {
    todo!()
}

/// `text` with every match replaced by `[redacted:<pattern>]`. Also redacts `Bearer <token>`.
pub fn redact(_text: &str) -> String {
    todo!()
}
