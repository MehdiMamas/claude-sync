//! Membership as code (§5, issue #19).
use crate::remote::Collaborator;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub invite: Vec<String>,
    pub remove: Vec<String>,
    pub cancel_invitations: Vec<u64>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.invite.is_empty() && self.remove.is_empty() && self.cancel_invitations.is_empty()
    }
}

/// What the host's app must do so collaborators match `members`. Never removes `host`.
/// Logins compare case-insensitively; a pending invitation is never sent again.
pub fn plan(members: &[String], host: &str, current: &[Collaborator]) -> Plan {
    let key = |s: &str| s.to_ascii_lowercase();
    let host = key(host);
    let wanted = |login: &str| {
        let k = key(login);
        k == host || members.iter().any(|m| key(m) == k)
    };
    let known = |login: &str| {
        let k = key(login);
        k == host
            || current.iter().any(|c| match c {
                Collaborator::Member(l) | Collaborator::Invited { login: l, .. } => key(l) == k,
            })
    };

    let mut plan = Plan::default();
    for m in members {
        if !known(m) && !plan.invite.iter().any(|i| key(i) == key(m)) {
            plan.invite.push(m.clone());
        }
    }
    for c in current {
        match c {
            Collaborator::Member(l) if !wanted(l) => plan.remove.push(l.clone()),
            Collaborator::Invited {
                login,
                invitation_id,
            } if !wanted(login) => plan.cancel_invitations.push(*invitation_id),
            _ => {}
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use Collaborator::*;

    fn logins(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn add_member_invites() {
        let p = plan(
            &logins(&["Host", "friend"]),
            "Host",
            &[Member("Host".into())],
        );
        assert_eq!(
            p,
            Plan {
                invite: logins(&["friend"]),
                ..Plan::default()
            }
        );
    }

    #[test]
    fn pending_invite_is_not_resent() {
        let current = [
            Member("host".into()),
            Invited {
                login: "Friend".into(),
                invitation_id: 7,
            },
        ];
        assert!(plan(&logins(&["host", "friend"]), "host", &current).is_empty());
    }

    #[test]
    fn removed_member_is_removed_and_invite_cancelled() {
        let current = [
            Member("host".into()),
            Member("Gone".into()),
            Invited {
                login: "never".into(),
                invitation_id: 3,
            },
            Member("stays".into()),
        ];
        let p = plan(&logins(&["host", "STAYS"]), "host", &current);
        assert_eq!(
            p,
            Plan {
                invite: vec![],
                remove: logins(&["Gone"]),
                cancel_invitations: vec![3]
            }
        );
    }

    #[test]
    fn host_is_never_removed_or_invited() {
        // Host missing from the list and listed as a collaborator: still kept.
        let p = plan(
            &logins(&["friend"]),
            "HOST",
            &[Member("host".into()), Member("friend".into())],
        );
        assert!(p.is_empty());
        // Host in the list but not among collaborators (GitHub may not list the owner): no invite.
        assert!(plan(&logins(&["host"]), "Host", &[]).is_empty());
    }

    #[test]
    fn duplicate_logins_invite_once() {
        let p = plan(&logins(&["h", "Bob", "bob"]), "h", &[]);
        assert_eq!(p.invite, logins(&["Bob"]));
    }
}
