//! Examples for the settings merge, the CLAUDE.md block and the review state.
use claude_sync_engine::claude_md::{compose_block, strip, GroupCopy};
use claude_sync_engine::review::{ReviewState, Step};
use claude_sync_engine::settings::{filter_blocked, machine_specific, merge, unapply, Owned};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn settings_merge_edit_drop_and_block() {
    let shared = json!({"model": "a", "env": {"K": "v"}, "list": [1, 2], "apiKeyHelper": "x"});
    let m = merge(
        &json!({"list": [0]}),
        &shared,
        &Owned::default(),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(
        m.settings,
        json!({"model": "a", "env": {"K": "v"}, "list": [0, 1, 2]})
    );
    assert!(m.notes.iter().any(|n| n.starts_with("apiKeyHelper")));
    // user edits model; group unchanged: edit stays
    let mut edited = m.settings.clone();
    edited["model"] = json!("mine");
    let again = merge(&edited, &shared, &m.owned, &BTreeMap::new()).unwrap();
    assert_eq!(again.settings["model"], "mine");
    // group drops everything: untouched values go, the edit stays
    let gone = unapply(&again.settings, &again.owned).unwrap();
    assert_eq!(gone.settings, json!({"model": "mine", "list": [0]}));
    // earlier group owns a key
    let taken = BTreeMap::from([("/model".to_string(), "org/one".to_string())]);
    let m2 = merge(
        &json!({}),
        &json!({"model": "b"}),
        &Owned::default(),
        &taken,
    )
    .unwrap();
    assert_eq!(m2.notes, ["/model: set by group org/one; not applied"]);
    assert!(merge(&json!([]), &shared, &Owned::default(), &BTreeMap::new()).is_err());
}

#[test]
fn blocked_and_machine_values() {
    let (v, notes) = filter_blocked(&json!({
        "env": {"T": "ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "OK": "1"},
        "hooks": {"Stop": [{"command": "/usr/local/bin/x"}, {"command": "echo hi"}]},
    }));
    assert_eq!(
        v,
        json!({"env": {"OK": "1"}, "hooks": {"Stop": [{"command": "echo hi"}]}})
    );
    assert_eq!(notes.len(), 2);
    let m = machine_specific(
        &json!({"apiKeyHelper": "x", "model": "m", "permissions": {"additionalDirectories": ["/a/b/"], "allow": ["Read"]}}),
    );
    assert_eq!(m, json!({"model": "m", "permissions": {"allow": ["Read"]}}));
}

#[test]
fn block_follows_group_order_and_strip_keeps_own_text() {
    let g = |f: &str, d: bool| GroupCopy {
        file: f.into(),
        disabled: d,
        present: true,
    };
    let groups = [
        g("/h/.claude-sync/groups/o__a/CLAUDE.md", false),
        g("/h/.claude-sync/groups/o__b/CLAUDE.md", true),
        g("/h/.claude-sync/groups/o__c/CLAUDE.md", false),
    ];
    let out = compose_block(Some(b"mine\n"), &groups, Some("/h")).unwrap();
    let s = String::from_utf8(out.clone()).unwrap();
    assert!(s.find("o__a").unwrap() < s.find("o__c").unwrap() && !s.contains("o__b"));
    assert_eq!(strip(&out).unwrap(), b"mine\n");
    assert!(compose_block(None, &[], None).is_none());
}

#[test]
fn review_accept_decline_and_own_changes() {
    let base = "# R\n- one\n- two\n";
    let mut st = ReviewState::new(base);
    let step = |t: &str, mine: bool, c: &str| Step {
        text: t.into(),
        mine,
        author: "ana".into(),
        commit: c.into(),
        message: "m".into(),
        at_unix: 1,
    };
    let p = st.pull(&[step("# R\n- one\n- two\n- three\n", false, "c1")]);
    assert_eq!(p.new.len(), 1);
    assert_eq!(st.accepted, base);
    let id = p.new[0].id.clone();
    assert_eq!(st.decline(Some(std::slice::from_ref(&id)), 5), 1);
    // same change again is not asked again
    let p = st.pull(&[
        step("# R\n- one\n- two\n", false, "c0"),
        step("# R\n- one\n- two\n- three\n", false, "c1"),
    ]);
    assert!(p.new.iter().all(|i| i.id != id));
    // own change applies at once
    let p = st.pull(&[step("# R\n- one\n- two\n- four\n", true, "c2")]);
    assert_eq!(p.applied.len(), 1);
    assert!(st.accepted.contains("- four"));
}
