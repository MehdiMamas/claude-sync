//! Whether you are at the computer (docs/architecture.md §22). Opt-in: nothing here runs
//! unless `Choices.activity.share` is on. The idle time comes from direct OS calls, and a
//! small state machine turns it into `active`, `idle` or `unknown` and decides when to
//! write `<state>/activity.json`. Nothing leaves this machine in this step.
use crate::state::write_json;
use crate::Paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Idle at or above this many seconds counts as `idle`.
pub const IDLE_AFTER_SECS: u64 = 120;
/// The value is written again after this long with no change of state.
pub const HEARTBEAT_SECS: u64 = 300;
/// How often the loops read the idle time.
pub const READ_EVERY_SECS: u64 = 30;

pub fn file(paths: &Paths) -> PathBuf {
    paths.state_dir.join("activity.json")
}

/// Seconds since the last keyboard or mouse input in this session. `None`: this OS (or
/// session) cannot say, shown as `unknown`, never as idle.
pub fn idle_seconds() -> Option<u64> {
    os::idle_seconds()
}

#[cfg(windows)]
mod os {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    pub fn idle_seconds() -> Option<u64> {
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` is a valid LASTINPUTINFO with `cbSize` set.
        if unsafe { GetLastInputInfo(&mut info) } == 0 {
            return None;
        }
        // SAFETY: no arguments. Both values are 32-bit tick counts that wrap together.
        let now = unsafe { GetTickCount() };
        Some(u64::from(now.wrapping_sub(info.dwTime)) / 1000)
    }
}

#[cfg(target_os = "macos")]
mod os {
    use core_foundation_sys::base::{CFRelease, CFTypeRef};
    use core_foundation_sys::number::{CFNumberGetValue, CFNumberRef};
    use core_foundation_sys::string::CFStringCreateWithCString;
    use std::ffi::{c_char, c_void};

    type Port = u32;
    const UTF8: u32 = 0x0800_0100;
    const SINT64: isize = 4;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingService(main_port: Port, matching: *mut c_void) -> Port;
        fn IORegistryEntryCreateCFProperty(
            entry: Port,
            key: *const c_void,
            allocator: *const c_void,
            options: u32,
        ) -> CFTypeRef;
        fn IOObjectRelease(object: Port) -> i32;
    }

    pub fn idle_seconds() -> Option<u64> {
        // SAFETY: plain IOKit/CoreFoundation calls; every object created here is released
        // before returning, and `IOServiceGetMatchingService` consumes the matching dict.
        unsafe {
            let matching = IOServiceMatching(c"IOHIDSystem".as_ptr());
            if matching.is_null() {
                return None;
            }
            let service = IOServiceGetMatchingService(0, matching);
            if service == 0 {
                return None;
            }
            let key = CFStringCreateWithCString(std::ptr::null(), c"HIDIdleTime".as_ptr(), UTF8);
            let value = if key.is_null() {
                std::ptr::null()
            } else {
                IORegistryEntryCreateCFProperty(service, key.cast(), std::ptr::null(), 0)
            };
            let mut ns: i64 = 0;
            let ok = !value.is_null()
                && CFNumberGetValue(value as CFNumberRef, SINT64, (&mut ns as *mut i64).cast())
                    != 0;
            if !value.is_null() {
                CFRelease(value);
            }
            if !key.is_null() {
                CFRelease(key.cast());
            }
            IOObjectRelease(service);
            (ok && ns >= 0).then(|| ns as u64 / 1_000_000_000)
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod os {
    pub fn idle_seconds() -> Option<u64> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Active,
    Idle,
    Unknown,
}

/// What `activity.json` holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Write {
    pub schema: u32,
    pub state: State,
    pub idle_seconds: Option<u64>,
    pub updated_at: u64,
}

/// Decides, from each reading, whether to write. Pure: the clock and the reading are
/// passed in.
#[derive(Debug, Default)]
pub struct Tracker {
    last: Option<(State, u64)>,
}

impl Tracker {
    /// `now` is Unix seconds. A write on a change of state, or when the last write is
    /// `HEARTBEAT_SECS` old; otherwise `None`.
    pub fn step(&mut self, now: u64, idle: Option<u64>) -> Option<Write> {
        let state = match idle {
            None => State::Unknown,
            Some(s) if s >= IDLE_AFTER_SECS => State::Idle,
            Some(_) => State::Active,
        };
        let due = match self.last {
            Some((st, at)) => st != state || now.saturating_sub(at) >= HEARTBEAT_SECS,
            None => true,
        };
        if !due {
            return None;
        }
        self.last = Some((state, now));
        Some(Write {
            schema: 1,
            state,
            idle_seconds: idle,
            updated_at: now,
        })
    }

    /// Forget the last write (sharing was turned off), so turning it on writes at once.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

pub fn write(paths: &Paths, w: &Write) -> crate::Result<()> {
    write_json(&file(paths), w)
}

/// Delete the value on this machine.
pub fn remove(paths: &Paths) {
    if let Err(e) = std::fs::remove_file(file(paths)) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("could not remove activity.json: {e}");
        }
    }
}

/// One pass of the 30 s loop: follows the choice. Off: the file is removed and the tracker
/// reset. On: read once and write when the tracker says so.
pub fn pass(paths: &Paths, tracker: &mut Tracker, now: u64, idle: impl FnOnce() -> Option<u64>) {
    let share = crate::choices::load(paths).is_ok_and(|c| c.activity.share);
    if !share {
        if tracker.last.take().is_some() || file(paths).exists() {
            remove(paths);
        }
        return;
    }
    if let Some(w) = tracker.step(now, idle()) {
        if let Err(e) = write(paths, &w) {
            tracing::warn!("could not write activity.json: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().unwrap();
        let p = Paths {
            claude_home: tmp.path().join(".claude"),
            state_dir: tmp.path().join(".claude-sync"),
        };
        (tmp, p)
    }

    #[test]
    fn active_below_the_threshold_idle_at_it() {
        let mut t = Tracker::default();
        assert_eq!(t.step(1000, Some(119)).unwrap().state, State::Active);
        assert_eq!(t.step(1030, Some(120)).unwrap().state, State::Idle);
        assert_eq!(t.step(1060, Some(5)).unwrap().state, State::Active);
    }

    #[test]
    fn heartbeat_after_300_seconds_without_a_change() {
        let mut t = Tracker::default();
        assert!(t.step(1000, Some(1)).is_some());
        assert!(t.step(1299, Some(2)).is_none());
        let w = t.step(1300, Some(3)).unwrap();
        assert_eq!(
            (w.state, w.updated_at, w.idle_seconds),
            (State::Active, 1300, Some(3))
        );
        assert!(t.step(1330, Some(4)).is_none());
    }

    #[test]
    fn no_write_without_a_change() {
        let mut t = Tracker::default();
        assert!(t.step(1000, Some(10)).is_some());
        for i in 1..10 {
            assert!(t.step(1000 + i * 30, Some(10)).is_none());
        }
    }

    #[test]
    fn no_reading_is_unknown_never_idle() {
        let mut t = Tracker::default();
        let w = t.step(1000, None).unwrap();
        assert_eq!((w.state, w.idle_seconds), (State::Unknown, None));
        assert_eq!(serde_json::to_value(&w).unwrap()["state"], "unknown");
    }

    #[test]
    fn file_has_the_documented_shape_and_remove_deletes_it() {
        let (_t, p) = paths();
        let mut t = Tracker::default();
        let w = t.step(42, Some(7)).unwrap();
        write(&p, &w).unwrap();
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file(&p)).unwrap()).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["state"], "active");
        assert_eq!(v["idleSeconds"], 7);
        assert_eq!(v["updatedAt"], 42);
        remove(&p);
        assert!(!file(&p).exists());
        remove(&p); // already gone: no error
    }

    #[test]
    fn pass_follows_the_choice() {
        let (_t, p) = paths();
        let mut t = Tracker::default();
        pass(&p, &mut t, 100, || Some(1));
        assert!(!file(&p).exists(), "off by default: nothing written");
        crate::choices::set_activity(&p, true).unwrap();
        pass(&p, &mut t, 130, || Some(1));
        assert!(file(&p).exists());
        crate::choices::set_activity(&p, false).unwrap();
        pass(&p, &mut t, 160, || panic!("no reading while off"));
        assert!(!file(&p).exists(), "turning it off removes the file");
        crate::choices::set_activity(&p, true).unwrap();
        pass(&p, &mut t, 190, || Some(1));
        assert!(file(&p).exists(), "turning it on again writes at once");
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn the_os_gives_a_real_reading() {
        assert!(idle_seconds().is_some());
    }
}
