//! When the sync loop wakes next (shared by the app and the command line daemon).
use std::time::Duration;

/// How long the sync loop sleeps: the interval, or until a rate limit lifts (plus 5 s,
/// at most an hour) when GitHub has limited us.
pub fn next_wake(now: u64, interval: Duration, limited_until: Option<u64>) -> Duration {
    match limited_until {
        Some(reset) if reset > now => Duration::from_secs((reset - now + 5).min(3600)),
        _ => interval,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn next_wake_is_the_interval_when_not_limited() {
        assert_eq!(next_wake(1000, MIN, None), MIN);
        // A limit that has already passed changes nothing.
        assert_eq!(next_wake(1000, MIN, Some(1000)), MIN);
        assert_eq!(next_wake(1000, MIN, Some(900)), MIN);
    }

    #[test]
    fn next_wake_waits_for_the_reset_plus_five_seconds() {
        assert_eq!(next_wake(1000, MIN, Some(1600)), Duration::from_secs(605));
    }

    #[test]
    fn next_wake_waits_at_most_an_hour() {
        assert_eq!(
            next_wake(1000, MIN, Some(1000 + 7200)),
            Duration::from_secs(3600)
        );
    }
}
