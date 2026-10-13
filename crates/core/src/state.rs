//! `state.json` (§10), plus small file and time helpers the engine shares.
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub login: Option<String>,
    pub device_name: Option<String>,
    /// Joined groups in priority order (first wins collisions).
    pub groups: Vec<crate::GroupId>,
    pub poll_seconds: Option<u64>,
    pub paused: bool,
}

impl AppState {
    /// A missing file is the default state.
    pub fn load(paths: &crate::Paths) -> crate::Result<Self> {
        match std::fs::read(paths.state_file()) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, paths: &crate::Paths) -> crate::Result<()> {
        write_json(&paths.state_file(), self)
    }
}

/// Write to a temp file next to `path`, then rename over it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> crate::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| crate::Error::Invalid(format!("no parent for {}", path.display())))?;
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> crate::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `20261013T094501Z`: a UTC time usable as a folder name on every OS.
pub fn utc_stamp(unix: u64) -> String {
    let (days, secs) = (unix / 86_400, unix % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// Parses GitHub's `2026-10-13T09:45:01Z` (also accepts `+00:00`). Other offsets are applied.
pub fn parse_rfc3339(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ if rest.len() == 6
            && (rest.starts_with('+') || rest.starts_with('-'))
            && &rest[3..4] == ":" =>
        {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            sign * (rest[1..3].parse::<i64>().ok()? * 3600 + rest[4..6].parse::<i64>().ok()? * 60)
        }
        _ => return None,
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset;
    u64::try_from(secs).ok()
}

// Howard Hinnant's civil calendar algorithms.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_defaults_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::Paths {
            claude_home: dir.path().join("c"),
            state_dir: dir.path().join("s"),
        };
        assert!(AppState::load(&paths).unwrap().groups.is_empty());
        let st = AppState {
            login: Some("me".into()),
            groups: vec![crate::GroupId::new("a", "b")],
            paused: true,
            ..Default::default()
        };
        st.save(&paths).unwrap();
        let back = AppState::load(&paths).unwrap();
        assert_eq!(back.groups, st.groups);
        assert!(back.paused);
        let leftovers: Vec<_> = std::fs::read_dir(&paths.state_dir).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temp file left behind");
    }

    #[test]
    fn utc_times() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(1_791_884_701), "20261013T094501Z");
        assert_eq!(utc_stamp(951_782_400), "20000229T000000Z");
        assert_eq!(parse_rfc3339("2026-10-13T09:45:01Z"), Some(1_791_884_701));
        assert_eq!(
            parse_rfc3339("2026-10-13T11:45:01.250+02:00"),
            Some(1_791_884_701)
        );
        assert_eq!(parse_rfc3339("2000-02-29T00:00:00Z"), Some(951_782_400));
        assert_eq!(parse_rfc3339("yesterday"), None);
    }
}
