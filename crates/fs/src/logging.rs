//! The log file, as v1 wrote it: `<state>/logs/claude-sync.YYYY-MM-DD.log`, one file per UTC
//! day, 7 kept, every event redacted before it reaches the file. Level: env `CLAUDE_SYNC_LOG`
//! (`error` .. `trace`), default `info`.
use crate::redact::redact;
use std::io::{self, Write};
use std::path::Path;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;

pub const LOG_PREFIX: &str = "claude-sync";
pub const LOG_SUFFIX: &str = "log";
pub const KEEP_FILES: usize = 7;

/// Install the global subscriber writing under `<state>/logs`. Keep the guard alive: dropping
/// it flushes the log. `None` when it could not start (a second start, an unwritable folder).
pub fn init(state: &Path) -> Option<Box<dyn std::any::Any + Send>> {
    let level = std::env::var("CLAUDE_SYNC_LOG")
        .ok()
        .and_then(|v| v.trim().parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::INFO);
    let (subscriber, guard) = build(&state.join("logs"), level)?;
    tracing::subscriber::set_global_default(subscriber).ok()?;
    Some(Box::new(guard))
}

pub(crate) fn build(
    dir: &Path,
    level: LevelFilter,
) -> Option<(impl tracing::Subscriber + Send + Sync, WorkerGuard)> {
    std::fs::create_dir_all(dir).ok()?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_PREFIX)
        .filename_suffix(LOG_SUFFIX)
        .max_log_files(KEEP_FILES)
        .build(dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(level)
        .with_writer(Redacting(writer))
        .finish();
    Some((subscriber, guard))
}

/// Hands each formatted event to `redact` before the file sees it.
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
        let body = text.trim_end_matches(['\r', '\n']);
        let mut out = redact(body);
        out.push_str(&text[body.len()..]);
        let _ = self.inner.write_all(out.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_go_to_a_dated_file_redacted_and_below_the_level_not_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        let (sub, guard) = build(&dir, LevelFilter::INFO).unwrap();
        let token = format!("ghp_{}", "a".repeat(36));
        tracing::subscriber::with_default(sub, || {
            tracing::info!("signed in with {token} done");
            tracing::warn!("auth Authorization: Bearer abcdefgh12345678");
            tracing::info!("plain line");
            tracing::debug!("below the level");
        });
        drop(guard);
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        let name = files[0].file_name().to_string_lossy().into_owned();
        assert!(
            name.starts_with("claude-sync.") && name.ends_with(".log"),
            "{name}"
        );
        let text = std::fs::read_to_string(files[0].path()).unwrap();
        assert!(!text.contains(&token) && !text.contains("abcdefgh12345678"));
        assert!(text.contains("plain line") && !text.contains("below the level"));
        assert_eq!(text.lines().count(), 3, "{text}");
        assert!(!text.contains('\x1b'));
    }

    #[test]
    fn only_the_newest_seven_files_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        std::fs::create_dir_all(&dir).unwrap();
        for d in 1..=9 {
            std::fs::write(dir.join(format!("claude-sync.2026-01-0{d}.log")), "x\n").unwrap();
            // the appender drops the oldest by creation time: keep them apart
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let (sub, guard) = build(&dir, LevelFilter::INFO).unwrap();
        tracing::subscriber::with_default(sub, || tracing::info!("today"));
        drop(guard);
        // the appender prunes when it opens a new file: the oldest are gone
        let n = std::fs::read_dir(&dir).unwrap().count();
        assert!(n <= KEEP_FILES, "{n} files");
        assert!(!dir.join("claude-sync.2026-01-01.log").exists());
    }
}
