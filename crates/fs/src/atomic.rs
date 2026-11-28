//! Atomic writes and the link checks every write goes through (v1-SpecApply write_atomic,
//! blocked_by_user, read_file). v2-Parity rows covered: 25, 43.
use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// Temp file beside the target, fsync, rename over it; the target's mode is kept; a Windows
/// lock (PermissionDenied) is retried 5 times, 50 ms apart; the temp file never stays.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let tmp = parent.join(format!(
        ".{}.claude-sync-tmp-{}",
        name.unwrap_or_default(),
        std::process::id()
    ));
    let res = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        if let Ok(m) = std::fs::metadata(path) {
            let _ = f.set_permissions(m.permissions());
        }
        f.sync_all()?;
        drop(f);
        let mut tries = 0;
        loop {
            match std::fs::rename(&tmp, path) {
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied && tries < 5 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                r => return r,
            }
        }
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    #[cfg(unix)]
    if res.is_ok() {
        if let Ok(d) = std::fs::File::open(parent) {
            let _ = d.sync_all();
        }
    }
    res
}

/// A parent folder of `path` below `root` is a link or not a folder (lstat from the root; a
/// missing one stops the check, it will be created). The root itself may be a link.
pub fn blocked(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return true; // outside the managed folder
    };
    let mut cur = root.to_path_buf();
    let n = rel.components().count();
    for c in rel.components().take(n.saturating_sub(1)) {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// The bytes of a plain file; `None` for a missing path, a folder or a link (never followed).
pub fn read_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => std::fs::read(path).map(Some),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
