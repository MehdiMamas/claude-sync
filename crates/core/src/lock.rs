//! One sync pass at a time across processes: the app, the command line and its daemon
//! share `sync.lock` in the state folder. The OS drops the lock when the process dies.
use crate::Paths;
use std::fs::{File, OpenOptions, TryLockError};
use std::time::{Duration, Instant};

/// Held while a pass runs; released on drop.
#[derive(Debug)]
pub struct SyncLock {
    _file: Option<File>,
}

/// Another process is syncing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

impl SyncLock {
    /// Take the lock, waiting up to `wait` for another process to finish. `Err(Busy)`:
    /// still held. When the lock file cannot be created the pass runs unlocked rather
    /// than not at all.
    pub fn acquire(paths: &Paths, wait: Duration) -> Result<SyncLock, Busy> {
        let unlocked = Ok(SyncLock { _file: None });
        if std::fs::create_dir_all(&paths.state_dir).is_err() {
            return unlocked;
        }
        let file = match OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(paths.lock_file())
        {
            Ok(f) => f,
            Err(_) => return unlocked,
        };
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(SyncLock { _file: Some(file) }),
                Err(TryLockError::WouldBlock) => {
                    if start.elapsed() >= wait {
                        return Err(Busy);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(TryLockError::Error(_)) => return unlocked,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(d: &std::path::Path) -> Paths {
        Paths {
            claude_home: d.join("claude"),
            state_dir: d.join("state"),
        }
    }

    #[test]
    fn a_second_syncer_waits_then_gives_up() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let first = SyncLock::acquire(&p, Duration::ZERO).unwrap();
        assert_eq!(
            SyncLock::acquire(&p, Duration::from_millis(200)).unwrap_err(),
            Busy
        );
        drop(first);
        assert!(SyncLock::acquire(&p, Duration::ZERO).is_ok());
    }
}
