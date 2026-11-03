//! Command line parsing. Exit codes: 0 done, 1 failed, 2 wrong usage.
use claude_sync_core::GroupId;

pub const USAGE: &str = "\
usage: claude-sync-cli <command> [--json]

  login                         sign in with GitHub (prints a code to enter in any browser)
  logout                        remove the sign-in from this machine
  groups                        groups joined here and invitations waiting
  join <owner/repo>             accept an invitation if there is one, and join
  leave <owner/repo>            leave a group
  sync                          one full sync now
  status                        sign-in, groups, personal memory
  personal status|enable|disable
  ssh status|enable|disable     reach your own machines over SSH (ssh <alias>)
  ssh alias <name>              the name your other machines use for this one
  review list                   changes waiting for you (CLAUDE.md, local edits, jobs)
  review accept <id>            accept one (ids come from `review list`)
  review undo <id>              decline or undo one
  daemon                        sync in a loop (run it with `service install`)
  service install|remove|status run the daemon in the background at start
  update [--check]              install the newest release
  version";

#[derive(Debug, PartialEq, Eq)]
pub enum Personal {
    Status,
    Enable,
    Disable,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Ssh {
    Status,
    Enable,
    Disable,
    Alias(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Review {
    List,
    Accept(String),
    Undo(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Service {
    Install,
    Remove,
    Status,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Cmd {
    Login,
    Logout,
    Groups,
    Join(GroupId),
    Leave(GroupId),
    Sync,
    Status,
    Personal(Personal),
    Ssh(Ssh),
    Review(Review),
    Daemon,
    Service(Service),
    Update {
        check: bool,
    },
    Version,
    /// `run-job <owner/repo> <id> [--now]`: started by the OS scheduler, not typed.
    RunJob(Vec<String>),
}

/// The command and `--json`, or the usage text.
pub fn parse(args: &[String]) -> Result<(Cmd, bool), String> {
    if args.first().map(String::as_str) == Some(claude_sync_core::jobs::RUN_JOB_ARG) {
        return Ok((Cmd::RunJob(args[1..].to_vec()), false));
    }
    let json = args.iter().any(|a| a == "--json");
    let check = args.iter().any(|a| a == "--check");
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| {
            *a != "--json"
                && !(*a == "--check" && args.first().map(String::as_str) == Some("update"))
        })
        .collect();
    let group = |s: &str| {
        GroupId::parse(s)
            .ok_or_else(|| format!("`{s}` is not a group: write owner/repo\n\n{USAGE}"))
    };
    let cmd = match words.as_slice() {
        ["login"] => Cmd::Login,
        ["logout"] => Cmd::Logout,
        ["groups"] => Cmd::Groups,
        ["join", g] => Cmd::Join(group(g)?),
        ["leave", g] => Cmd::Leave(group(g)?),
        ["sync"] => Cmd::Sync,
        ["status"] => Cmd::Status,
        ["personal", "status"] => Cmd::Personal(Personal::Status),
        ["personal", "enable"] => Cmd::Personal(Personal::Enable),
        ["personal", "disable"] => Cmd::Personal(Personal::Disable),
        ["ssh", "status"] => Cmd::Ssh(Ssh::Status),
        ["ssh", "enable"] => Cmd::Ssh(Ssh::Enable),
        ["ssh", "disable"] => Cmd::Ssh(Ssh::Disable),
        ["ssh", "alias", name] => Cmd::Ssh(Ssh::Alias((*name).into())),
        ["review", "list"] => Cmd::Review(Review::List),
        ["review", "accept", id] => Cmd::Review(Review::Accept((*id).into())),
        ["review", "undo", id] => Cmd::Review(Review::Undo((*id).into())),
        ["daemon"] => Cmd::Daemon,
        ["service", "install"] => Cmd::Service(Service::Install),
        ["service", "remove"] => Cmd::Service(Service::Remove),
        ["service", "status"] => Cmd::Service(Service::Status),
        ["update"] => Cmd::Update { check },
        ["version"] | ["--version"] | ["-V"] => Cmd::Version,
        _ => return Err(USAGE.into()),
    };
    Ok((cmd, json))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(parse(&v(&["login"])), Ok((Cmd::Login, false)));
        assert_eq!(parse(&v(&["status", "--json"])), Ok((Cmd::Status, true)));
        assert_eq!(parse(&v(&["--json", "groups"])), Ok((Cmd::Groups, true)));
        assert_eq!(
            parse(&v(&["join", "me/team"])),
            Ok((Cmd::Join(GroupId::new("me", "team")), false))
        );
        assert_eq!(
            parse(&v(&["leave", "me/team", "--json"])),
            Ok((Cmd::Leave(GroupId::new("me", "team")), true))
        );
        assert_eq!(
            parse(&v(&["personal", "enable"])),
            Ok((Cmd::Personal(Personal::Enable), false))
        );
        assert_eq!(
            parse(&v(&["ssh", "enable", "--json"])),
            Ok((Cmd::Ssh(Ssh::Enable), true))
        );
        assert_eq!(
            parse(&v(&["ssh", "alias", "tower"])),
            Ok((Cmd::Ssh(Ssh::Alias("tower".into())), false))
        );
        assert_eq!(
            parse(&v(&["review", "accept", "md:a/b:x1"])),
            Ok((Cmd::Review(Review::Accept("md:a/b:x1".into())), false))
        );
        assert_eq!(
            parse(&v(&["review", "undo", "job:a/b:nightly"])),
            Ok((Cmd::Review(Review::Undo("job:a/b:nightly".into())), false))
        );
        assert_eq!(
            parse(&v(&["service", "install"])),
            Ok((Cmd::Service(Service::Install), false))
        );
        assert_eq!(parse(&v(&["daemon"])), Ok((Cmd::Daemon, false)));
        assert_eq!(
            parse(&v(&["update", "--check"])),
            Ok((Cmd::Update { check: true }, false))
        );
        assert_eq!(
            parse(&v(&["update"])),
            Ok((Cmd::Update { check: false }, false))
        );
    }

    #[test]
    fn run_job_passes_its_arguments_through() {
        assert_eq!(
            parse(&v(&["run-job", "a/b", "nightly", "--now"])),
            Ok((Cmd::RunJob(v(&["a/b", "nightly", "--now"])), false))
        );
    }

    #[test]
    fn wrong_usage_gets_the_usage_text() {
        for bad in [
            v(&[]),
            v(&["nope"]),
            v(&["join"]),
            v(&["join", "not-a-group"]),
            v(&["review"]),
            v(&["ssh"]),
            v(&["ssh", "alias"]),
            v(&["ssh", "x"]),
            v(&["review", "accept"]),
            v(&["service", "restart"]),
            v(&["status", "extra"]),
            v(&["login", "--check"]),
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
        assert!(parse(&v(&["join", "x"]))
            .unwrap_err()
            .contains("owner/repo"));
        assert!(parse(&[]).unwrap_err().starts_with("usage:"));
    }
}
