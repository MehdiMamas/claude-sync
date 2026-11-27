//! The tray (Parity 56, 71): [`items`] decides what the menu holds, from a snapshot alone, so
//! the order, the notifications item and the groups toggle are tested without a window system.
//! The rest of the file only draws those items and sends the commands they stand for.
use crate::window;
use claude_sync_actions::{Command, GroupStatus, Handle, Snapshot};
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Id {
    Status,
    Sync,
    Open,
    Notifications,
    Pause,
    Groups,
    Feedback,
    Quit,
}

impl Id {
    const ALL: [Id; 8] = [
        Id::Status,
        Id::Sync,
        Id::Open,
        Id::Notifications,
        Id::Pause,
        Id::Groups,
        Id::Feedback,
        Id::Quit,
    ];

    fn key(self) -> &'static str {
        match self {
            Id::Status => "status",
            Id::Sync => "sync",
            Id::Open => "open",
            Id::Notifications => "notifications",
            Id::Pause => "pause",
            Id::Groups => "groups",
            Id::Feedback => "feedback",
            Id::Quit => "quit",
        }
    }

    fn from_key(k: &str) -> Option<Id> {
        Id::ALL.into_iter().find(|i| i.key() == k)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: Id,
    pub text: String,
    pub enabled: bool,
}

fn item(id: Id, text: impl Into<String>, enabled: bool) -> Item {
    Item {
        id,
        text: text.into(),
        enabled,
    }
}

fn ago(secs: i64) -> String {
    match secs {
        ..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86400),
    }
}

/// The first menu line (`now` in Unix seconds).
pub fn status_line(s: &Snapshot, now: i64) -> String {
    if s.session_expired.is_some() {
        return "Sign-in expired".into();
    }
    if s.login.is_none() {
        return "Not signed in".into();
    }
    if s.paused {
        return "Paused".into();
    }
    if s.syncing {
        return "Syncing...".into();
    }
    match s.groups.iter().filter_map(|g| g.last_sync).max() {
        Some(t) => format!("Synced {}", ago((now - t).max(0))),
        None => "Waiting to sync".into(),
    }
}

/// Every joined group is off here (and there is at least one).
fn all_off(s: &Snapshot) -> bool {
    !s.groups.is_empty()
        && s.groups
            .iter()
            .all(|g| g.off || g.status == GroupStatus::Off)
}

/// The menu, in order: status, Sync now, Open, Notifications (N) while some are unread, Pause or
/// Resume, Turn groups off or on, Send feedback, Quit.
pub fn items(s: &Snapshot, now: i64) -> Vec<Item> {
    let mut v = vec![
        item(Id::Status, status_line(s, now), false),
        item(Id::Sync, "Sync now", true),
        item(Id::Open, "Open claude-sync", true),
    ];
    if s.unread > 0 {
        v.push(item(
            Id::Notifications,
            format!("Notifications ({})", s.unread),
            true,
        ));
    }
    v.push(item(
        Id::Pause,
        if s.paused {
            "Resume syncing"
        } else {
            "Pause syncing"
        },
        true,
    ));
    v.push(item(
        Id::Groups,
        if all_off(s) {
            "Turn groups on"
        } else {
            "Turn groups off"
        },
        true,
    ));
    v.push(item(Id::Feedback, "Send feedback...", true));
    v.push(item(Id::Quit, "Quit", true));
    v
}

/// What the groups item sends: all groups on only if every group is off, otherwise all off
/// (also when some are on and some off).
pub fn groups_commands(s: &Snapshot) -> Vec<Command> {
    let off = !all_off(s);
    s.groups
        .iter()
        .map(|g| Command::SetGroupOff {
            repo: g.repo.clone(),
            off,
        })
        .collect()
}

/// What the item stands for, run on the async runtime.
fn run(app: &AppHandle, id: Id) {
    let h = app.state::<Handle>().inner().clone();
    let s = h.snapshot();
    let cmds = match id {
        Id::Sync => vec![Command::SyncNow],
        Id::Pause => vec![if s.paused {
            Command::Resume
        } else {
            Command::Pause
        }],
        Id::Groups => groups_commands(&s),
        Id::Open => return window::open(app, None),
        Id::Notifications => return window::open(app, Some("notifications")),
        Id::Feedback => return window::open(app, Some("feedback")),
        // shortcut: no pending update to install on quit, step 9 (updater) adds it.
        Id::Quit => return app.exit(0),
        Id::Status => return,
    };
    tauri::async_runtime::spawn(async move {
        for c in cmds {
            if let Err(e) = h.send(c).await {
                eprintln!("claude-sync: {e}");
            }
        }
    });
}

/// The menu on screen, to rebuild only when it changed.
#[derive(Default)]
pub struct Shown(Mutex<Vec<Item>>);

const TRAY: &str = "main";

fn menu(app: &AppHandle, list: &[Item]) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for i in list {
        menu.append(&MenuItem::with_id(
            app,
            i.id.key(),
            &i.text,
            i.enabled,
            None::<&str>,
        )?)?;
        if matches!(i.id, Id::Status | Id::Groups) {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
        }
    }
    Ok(menu)
}

pub fn build(app: &AppHandle, s: &Snapshot, now: i64) -> tauri::Result<()> {
    let list = items(s, now);
    let mut tray = TrayIconBuilder::with_id(TRAY)
        .tooltip("claude-sync")
        .menu(&menu(app, &list)?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| {
            if let Some(id) = Id::from_key(e.id().as_ref()) {
                run(app, id);
            }
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                window::open(tray.app_handle(), None);
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    let state = app.state::<Shown>();
    if let Ok(mut shown) = state.0.lock() {
        *shown = list;
    }
    Ok(())
}

/// Bring the tray in line with the snapshot; draws nothing when the items are the same.
pub fn apply(app: &AppHandle, s: &Snapshot, now: i64) {
    let list = items(s, now);
    let state = app.state::<Shown>();
    let Ok(mut shown) = state.0.lock() else {
        return;
    };
    if *shown == list {
        return;
    }
    let Some(tray) = app.tray_by_id(TRAY) else {
        return;
    };
    match menu(app, &list) {
        Ok(m) => {
            let _ = tray.set_menu(Some(m));
            let _ = tray.set_tooltip(Some(format!("claude-sync: {}", list[0].text)));
            *shown = list;
        }
        Err(e) => eprintln!("claude-sync: tray menu: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_sync_actions::{GroupView, SignInState};

    fn group(repo: &str, off: bool) -> GroupView {
        GroupView {
            repo: repo.into(),
            name: repo.into(),
            host: "acme".into(),
            is_host: false,
            status: if off {
                GroupStatus::Off
            } else {
                GroupStatus::Synced
            },
            error: None,
            last_sync: Some(1000),
            members: vec![],
            proposal_mode: false,
            auto_accept_claude_md: true,
            jobs_auto_accept: false,
            host_lock: None,
            review_choice: Default::default(),
            pending_review: 0,
            plugin_problems: vec![],
            syncing: false,
            failed_at: None,
            off,
            pending_invites: vec![],
        }
    }

    fn snap() -> Snapshot {
        Snapshot {
            login: Some("ann".into()),
            sign_in: SignInState::SignedOut,
            session_expired: None,
            app_mode: false,
            paused: false,
            interval_secs: 60,
            syncing: false,
            rate_limited_until: None,
            groups: vec![],
            invitations: vec![],
            discoverable: vec![],
            personal_on: false,
            personal: Default::default(),
            unread: 0,
            notifications: vec![],
            jobs: vec![],
            ssh_on: false,
            activity_on: false,
            update: String::new(),
            update_checked: None,
            app_uncovered: Vec::new(),
            desktop_notifications: true,
            start_at_login: None,
            install: None,
        }
    }

    fn texts(s: &Snapshot) -> Vec<String> {
        items(s, 1120).into_iter().map(|i| i.text).collect()
    }

    /// Plan step 7 check: the menu of Parity row 56, in order.
    #[test]
    fn the_menu_is_parity_row_56_in_order() {
        let mut s = snap();
        s.groups = vec![group("acme/a", false)];
        assert_eq!(
            texts(&s),
            [
                "Synced 2 min ago",
                "Sync now",
                "Open claude-sync",
                "Pause syncing",
                "Turn groups off",
                "Send feedback...",
                "Quit"
            ]
        );
        // unread notifications: the 4th item, with the count
        s.unread = 2;
        s.paused = true;
        assert_eq!(
            texts(&s),
            [
                "Paused",
                "Sync now",
                "Open claude-sync",
                "Notifications (2)",
                "Resume syncing",
                "Turn groups off",
                "Send feedback...",
                "Quit"
            ]
        );
        assert_eq!(items(&s, 0)[3].id, Id::Notifications);
        // read: gone again
        s.unread = 0;
        assert!(!items(&s, 0).iter().any(|i| i.id == Id::Notifications));
        // only the status line is not clickable
        assert!(items(&s, 0).iter().skip(1).all(|i| i.enabled) && !items(&s, 0)[0].enabled);
    }

    #[test]
    fn groups_toggle_turns_all_on_only_if_every_group_is_off() {
        let mut s = snap();
        let offs = |s: &Snapshot| -> Vec<bool> {
            groups_commands(s)
                .into_iter()
                .map(|c| match c {
                    Command::SetGroupOff { off, .. } => off,
                    _ => unreachable!(),
                })
                .collect()
        };
        // none: nothing to send, the label offers "off"
        assert!(groups_commands(&s).is_empty());
        assert_eq!(texts(&s)[4], "Turn groups off");
        // all on: all off
        s.groups = vec![group("acme/a", false), group("acme/b", false)];
        assert_eq!(offs(&s), [true, true]);
        // some on, some off: all off
        s.groups[0] = group("acme/a", true);
        assert_eq!(offs(&s), [true, true]);
        assert_eq!(texts(&s)[4], "Turn groups off");
        // every group off: all on
        s.groups[1] = group("acme/b", true);
        assert_eq!(offs(&s), [false, false]);
        assert_eq!(texts(&s)[4], "Turn groups on");
    }

    #[test]
    fn status_line_words() {
        let mut s = snap();
        assert_eq!(status_line(&s, 1000), "Waiting to sync");
        s.groups = vec![group("acme/a", false)];
        assert_eq!(status_line(&s, 1000), "Synced just now");
        assert_eq!(status_line(&s, 1000 + 3 * 3600), "Synced 3 h ago");
        s.syncing = true;
        assert_eq!(status_line(&s, 1000), "Syncing...");
        s.paused = true;
        assert_eq!(status_line(&s, 1000), "Paused");
        s.session_expired = Some("ann".into());
        assert_eq!(status_line(&s, 1000), "Sign-in expired");
        s.login = None;
        s.session_expired = None;
        assert_eq!(status_line(&s, 1000), "Not signed in");
    }

    #[test]
    fn ids_round_trip() {
        for i in Id::ALL {
            assert_eq!(Id::from_key(i.key()), Some(i));
        }
    }
}
