//! Logs never hold a token or a key: known credential shapes become `***`.
//! v2-Parity rows covered: 30 (log hygiene).
use claude_sync_engine::secrets::{scan, ALLOW_MARKER};

/// `text` with every line that looks like a secret replaced by `[redacted]`, and every
/// `Authorization`/`token` value masked.
pub fn redact(text: &str) -> String {
    let mut in_key = false;
    text.lines()
        .map(|l| {
            // the allow marker lets a line be pushed, never logged
            let bare = l.replace(ALLOW_MARKER, "");
            if in_key || !scan(&bare).is_empty() {
                // a private key is redacted from BEGIN to its END line
                if l.contains("PRIVATE KEY-----") {
                    in_key = !in_key && l.contains("-----BEGIN");
                }
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

    #[test]
    fn allow_marked_lines_are_still_redacted() {
        let t = format!("ghp_{} # claude-sync: allow-secret\nok", "a".repeat(36));
        assert_eq!(redact(&t), "[redacted]\nok");
    }

    #[test]
    fn a_private_key_is_redacted_to_its_end_line() {
        let t = "a\n-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK\nQWxsIHlvdXI=\n-----END RSA PRIVATE KEY-----\nb";
        assert_eq!(
            redact(t),
            "a\n[redacted]\n[redacted]\n[redacted]\n[redacted]\nb"
        );
    }
}
