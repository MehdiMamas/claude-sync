//! Membership as code (§5, issue #19).
use crate::remote::Collaborator;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub invite: Vec<String>,
    pub remove: Vec<String>,
    pub cancel_invitations: Vec<u64>,
}

/// What the host's app must do so collaborators match `members`. Never removes `host`.
pub fn plan(_members: &[String], _host: &str, _current: &[Collaborator]) -> Plan {
    todo!()
}
