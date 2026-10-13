//! Logs to `<state>/logs/`, daily rotation, 7 kept, redacted (§11, issue #23).
//!
//! Files are named `claude-sync.YYYY-MM-DD.log` (one per UTC day; that is how the
//! rolling appender names daily files). Every event goes through `secrets::redact`
//! before it reaches the file. Level: env `CLAUDE_SYNC_LOG` (`error` .. `trace`),
//! default `info`.
use crate::{secrets, Paths};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;

pub const LOG_PREFIX: &str = "claude-sync";
pub const LOG_SUFFIX: &str = "log";
pub const KEEP_FILES: usize = 7;

/// Install the global subscriber. Returns a guard the app keeps alive: dropping it
/// flushes the log.
pub fn init(paths: &Paths) -> crate::Result<Box<dyn std::any::Any + Send>> {
    let level = std::env::var("CLAUDE_SYNC_LOG")
        .ok()
        .and_then(|v| v.trim().parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::INFO);
    let (subscriber, guard) = build(&paths.logs_dir(), level)?;
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| crate::Error::Invalid("logging was already started".into()))?;
    Ok(Box::new(guard))
}

/// The subscriber `init` installs, writing under `dir`.
pub(crate) fn build(
    dir: &Path,
    level: LevelFilter,
) -> crate::Result<(impl tracing::Subscriber + Send + Sync, WorkerGuard)> {
    std::fs::create_dir_all(dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_PREFIX)
        .filename_suffix(LOG_SUFFIX)
        .max_log_files(KEEP_FILES)
        .build(dir)
        .map_err(|e| crate::Error::Invalid(format!("could not open the log: {e}")))?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(level)
        .with_writer(Redacting(writer))
        .finish();
    Ok((subscriber, guard))
}

/// Last `max_bytes` of the newest log file, from the start of a line, redacted again
/// (in case an older build wrote it). Empty when there is no log yet.
pub fn tail(paths: &Paths, max_bytes: usize) -> crate::Result<String> {
    let Some(file) = newest_log(&paths.logs_dir())? else {
        return Ok(String::new());
    };
    let bytes = std::fs::read(&file)?;
    let mut start = bytes.len().saturating_sub(max_bytes);
    if start > 0 {
        // Begin after the first line break so no line is cut in half.
        start = match bytes[start..].iter().position(|&b| b == b'\n') {
            Some(i) => start + i + 1,
            None => bytes.len(),
        };
    }
    Ok(secrets::redact(&String::from_utf8_lossy(&bytes[start..])))
}

/// `claude-sync.YYYY-MM-DD.log` sort by date; the greatest name is the newest.
fn newest_log(dir: &Path) -> io::Result<Option<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let prefix = format!("{LOG_PREFIX}.");
    let suffix = format!(".{LOG_SUFFIX}");
    let mut best: Option<PathBuf> = None;
    for ent in entries {
        let ent = ent?;
        let name = ent.file_name().to_string_lossy().into_owned();
        let newer = best
            .as_ref()
            .is_none_or(|b| b.file_name().is_some_and(|n| *n < *name));
        if name.starts_with(&prefix) && name.ends_with(&suffix) && newer && ent.path().is_file() {
            best = Some(ent.path());
        }
    }
    Ok(best)
}

/// Hands each formatted event to `secrets::redact` before the file sees it.
struct Redacting(NonBlocking);

impl<'a> MakeWriter<'a> for Redacting {
    type Writer = RedactingWriter;
    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter {
            buf: Vec::new(),
            inner: self.0.clone(),
        }
    }
}

/// Collects one event, then writes it redacted when dropped.
struct RedactingWriter {
    buf: Vec<u8>,
    inner: NonBlocking,
}

impl Write for RedactingWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for RedactingWriter {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&self.buf);
        // Keep the line end out of the redaction: an unterminated private key header
        // redacts to the end of the text and would swallow it.
        let body = text.trim_end_matches(['\r', '\n']);
        let mut out = secrets::redact(body);
        out.push_str(&text[body.len()..]);
        let _ = self.inner.write_all(out.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(tmp: &tempfile::TempDir) -> Paths {
        Paths {
            claude_home: tmp.path().join(".claude"),
            state_dir: tmp.path().join(".claude-sync"),
        }
    }

    fn log_text(dir: &Path) -> String {
        let file = newest_log(dir).unwrap().expect("a log file");
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("claude-sync.") && name.ends_with(".log"),
            "{name}"
        );
        std::fs::read_to_string(file).unwrap()
    }

    #[test]
    fn every_secret_pattern_is_redacted_in_the_log() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = paths(&tmp).logs_dir();
        let (sub, guard) = build(&dir, LevelFilter::INFO).unwrap();
        let samples = secrets::tests::samples();
        tracing::subscriber::with_default(sub, || {
            for (secret, name) in &samples {
                tracing::info!(pattern = name, "message {secret} end");
                tracing::warn!(value = %secret, "as a field");
            }
            tracing::info!("auth Authorization: Bearer abcdefgh12345678");
            tracing::debug!("below the level, not written");
        });
        drop(guard);
        let text = log_text(&dir);
        for (secret, name) in &samples {
            assert!(text.contains(&format!("[redacted:{name}]")), "{name}");
            if !matches!(*name, "token-field") {
                assert!(!text.contains(secret.as_str()), "{name} leaked:\n{text}");
            }
        }
        assert!(!text.contains("abcdefgh12345678"));
        assert!(!text.contains("below the level"));
        // One line per event, even when a private key header redacts to the end.
        assert_eq!(text.lines().count(), samples.len() * 2 + 1, "{text}");
        assert!(!text.contains('\x1b'), "no colour codes in the file");
    }

    #[test]
    fn token_field_values_are_redacted_in_the_log() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = paths(&tmp).logs_dir();
        let (sub, guard) = build(&dir, LevelFilter::INFO).unwrap();
        tracing::subscriber::with_default(sub, || {
            tracing::info!(r#"body {{"access_token": "s3cr3t-value", "scope": "repo"}}"#);
        });
        drop(guard);
        let text = log_text(&dir);
        assert!(!text.contains("s3cr3t-value"), "{text}");
        assert!(text.contains("[redacted:token-field]"), "{text}");
    }

    #[test]
    fn tail_reads_the_newest_file_from_a_line_start() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(&tmp);
        assert_eq!(tail(&p, 100).unwrap(), "");
        let dir = p.logs_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("claude-sync.2026-10-12.log"), "old day\n").unwrap();
        let token = format!("ghp_{}", "a".repeat(36));
        std::fs::write(
            dir.join("claude-sync.2026-10-13.log"),
            format!("first line is long enough\nsecond {token}\nthird\n"),
        )
        .unwrap();
        std::fs::write(dir.join("other.txt"), "zzz\n").unwrap();
        assert_eq!(
            tail(&p, 10_000).unwrap(),
            "first line is long enough\nsecond [redacted:ghp]\nthird\n"
        );
        assert_eq!(tail(&p, 60).unwrap(), "second [redacted:ghp]\nthird\n");
        assert_eq!(tail(&p, 3).unwrap(), "");
    }
}
