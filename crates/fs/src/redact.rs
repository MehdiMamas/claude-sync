//! Logs never hold a token or a key: known credential shapes become `***`.
//! v2-Parity rows covered: 30 (log hygiene).
use claude_sync_engine::secrets::scan;

/// `text` with every line that looks like a secret replaced by `[redacted]`, and every
/// `Authorization`/`token` value masked.
pub fn redact(text: &str) -> String {
    text.lines()
        .map(|l| {
            if !scan(l).is_empty() {
                return "[redacted]".to_string();
            }
            let low = l.to_ascii_lowercase();
            match ["authorization:", "bearer ", "token="]
                .iter()
                .find(|k| low.contains(*k))
            {
                Some(k) => {
                    let at = low.find(k).unwrap_or(0) + k.len();
                    format!("{}***", &l[..at])
                }
                None => l.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_headers_are_masked() {
        let t = format!("ok\nghp_{}\nAuthorization: Bearer abc", "a".repeat(36));
        let r = redact(&t);
        assert!(!r.contains("aaaa") && !r.contains("abc"));
        assert!(r.starts_with("ok\n"));
    }
}
