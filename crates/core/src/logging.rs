//! Logs to `<state>/logs/claude-sync.log`, daily rotation, 7 kept, redacted (§11, issue #23).

/// Install the global subscriber. Returns a guard the app keeps alive.
pub fn init(_paths: &crate::Paths) -> crate::Result<Box<dyn std::any::Any + Send>> {
    todo!()
}

/// Last `max_bytes` of the current log (already redacted), for the feedback form.
pub fn tail(_paths: &crate::Paths, _max_bytes: usize) -> crate::Result<String> {
    todo!()
}
