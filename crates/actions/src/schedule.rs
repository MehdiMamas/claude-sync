//! When the background loop acts (Parity 10, 16, 20, 21, 3). Pure: `due` gets the time and
//! the facts and says what to do, so a test drives it with a fake clock.
use std::collections::BTreeMap;

pub const INVITES_EVERY: i64 = 15 * 60;
pub const RENAME_EVERY: i64 = 24 * 60 * 60;

#[derive(Debug, Default)]
pub struct Schedule {
    next_sync: i64,
    next_invites: i64,
    next_rename: BTreeMap<String, i64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Due {
    pub sync: bool,
    pub invites: bool,
    /// Repos to GET once to follow a rename or transfer.
    pub renames: Vec<String>,
}

pub struct Facts<'a> {
    pub now: i64,
    pub signed_in: bool,
    /// A call got 401: nothing runs until the person signs in again.
    pub expired: bool,
    pub paused: bool,
    pub interval: i64,
    pub limited_until: Option<i64>,
    pub repos: &'a [String],
}

impl Schedule {
    /// The window opened: look for invitations on the next tick.
    pub fn invites_now(&mut self) {
        self.next_invites = 0;
    }

    pub fn due(&mut self, f: &Facts) -> Due {
        let mut d = Due::default();
        if !f.signed_in || f.expired || f.paused {
            return d;
        }
        if f.limited_until.is_some_and(|u| f.now < u) {
            return d; // wait until it lifts, never hammer
        }
        if f.now >= self.next_sync {
            self.next_sync = f.now + f.interval;
            d.sync = true;
        }
        if f.now >= self.next_invites {
            self.next_invites = f.now + INVITES_EVERY;
            d.invites = true;
        }
        self.next_rename.retain(|r, _| f.repos.contains(r));
        for r in f.repos {
            let n = self.next_rename.entry(r.clone()).or_insert(0);
            if f.now >= *n {
                *n = f.now + RENAME_EVERY;
                d.renames.push(r.clone());
            }
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(now: i64, repos: &'a [String]) -> Facts<'a> {
        Facts {
            now,
            signed_in: true,
            expired: false,
            paused: false,
            interval: 60,
            limited_until: None,
            repos,
        }
    }

    #[test]
    fn intervals_and_days() {
        let repos = vec!["a/b".to_string()];
        let mut s = Schedule::default();
        let d = s.due(&facts(1000, &repos));
        assert!(d.sync && d.invites && d.renames == repos);
        assert_eq!(s.due(&facts(1059, &repos)), Due::default());
        let d = s.due(&facts(1060, &repos));
        assert!(d.sync && !d.invites && d.renames.is_empty());
        assert!(s.due(&facts(1000 + INVITES_EVERY, &repos)).invites);
        assert_eq!(s.due(&facts(1000 + RENAME_EVERY, &repos)).renames, repos);
    }

    #[test]
    fn pause_expiry_and_limits_stop_everything() {
        let mut s = Schedule::default();
        let mut f = facts(5000, &[]);
        f.paused = true;
        assert_eq!(s.due(&f), Due::default());
        f.paused = false;
        f.expired = true;
        assert_eq!(s.due(&f), Due::default());
        f.expired = false;
        f.limited_until = Some(6000);
        assert_eq!(s.due(&f), Due::default());
        f.now = 6000;
        assert!(s.due(&f).sync);
    }
}
