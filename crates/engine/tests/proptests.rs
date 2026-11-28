//! v2-EngRules properties of the planner and merges.
use claude_sync_engine::formats::group::SyncFlags;
use claude_sync_engine::formats::schedules::{validate, MAX_JOBS, MAX_PROMPT, MAX_TIMEOUT};
use claude_sync_engine::merge::merge_lines;
use claude_sync_engine::paths::{is_personal_path, is_planned_path};
use claude_sync_engine::plan::{plan, FileSet, Opts, Plan, Scope};
use claude_sync_engine::secrets::{scan, ALLOW_MARKER, PATTERNS};
use proptest::prelude::*;
use std::collections::BTreeSet;

fn flags() -> SyncFlags {
    SyncFlags::default()
}

fn opts(personal: bool, push: bool) -> Opts {
    Opts {
        scope: if personal {
            Scope::Personal
        } else {
            Scope::Group(flags())
        },
        push,
        links: BTreeSet::new(),
    }
}

const GROUP_PATHS: [&str; 5] = [
    "skills/a/SKILL.md",
    "skills/b/x.txt",
    "agents/x.md",
    "commands/y.md",
    "settings.shared.json",
];
const PERSONAL_PATHS: [&str; 4] = [
    "memory/home-proj/MEMORY.md",
    "memory/home/notes.md",
    "settings.json",
    "state/claude-sync/x.json",
];
const VALUES: [&str; 6] = [
    "one\ntwo\n",
    "one\r\ntwo\r\n",
    "one\nthree\n",
    "{\"a\":1}",
    "{\"a\":2,\"b\":1}",
    "{\"c\":{\"d\":1}}",
];

fn file_set(paths: &'static [&'static str]) -> impl Strategy<Value = FileSet> {
    proptest::collection::vec(proptest::option::of(0..VALUES.len()), paths.len()).prop_map(
        move |v| {
            paths
                .iter()
                .zip(v)
                .filter_map(|(p, i)| i.map(|i| (p.to_string(), VALUES[i].as_bytes().to_vec())))
                .collect()
        },
    )
}

fn apply(remote: &FileSet, local: &FileSet, p: &Plan) -> (FileSet, FileSet) {
    let mut local = local.clone();
    let mut remote = remote.clone();
    for (k, v) in &p.writes {
        local.insert(k.clone(), v.clone());
    }
    for k in &p.deletes {
        local.remove(k);
    }
    for (k, v) in &p.pushes {
        match v {
            Some(v) => remote.insert(k.clone(), v.clone()),
            None => remote.remove(k),
        };
    }
    (remote, local)
}

fn check_idempotent(personal: bool, push: bool, r: FileSet, b: FileSet, l: FileSet) {
    let o = opts(personal, push);
    let p1 = plan(&r, &b, &l, &o);
    let (r2, l2) = apply(&r, &l, &p1);
    let p2 = plan(&r2, &p1.new_base, &l2, &o);
    assert!(p2.is_noop(), "second plan not empty: {p2:?}\nfirst: {p1:?}");
}

proptest! {
    // 1. a plan applied twice equals once
    #[test]
    fn group_plan_is_idempotent(r in file_set(&GROUP_PATHS), b in file_set(&GROUP_PATHS), l in file_set(&GROUP_PATHS), push in any::<bool>()) {
        check_idempotent(false, push, r, b, l);
    }

    #[test]
    fn personal_plan_is_idempotent(r in file_set(&PERSONAL_PATHS), b in file_set(&PERSONAL_PATHS), l in file_set(&PERSONAL_PATHS)) {
        check_idempotent(true, true, r, b, l);
    }

    // 2. never deletes a file the user changed
    #[test]
    fn never_deletes_user_changes(r in file_set(&GROUP_PATHS), b in file_set(&GROUP_PATHS), l in file_set(&GROUP_PATHS), push in any::<bool>()) {
        let p = plan(&r, &b, &l, &opts(false, push));
        for d in &p.deletes {
            prop_assert_eq!(l.get(d), b.get(d));
        }
    }

    // 3. MEMORY.md merge keeps every added line; CRLF vs LF alone gives no push
    #[test]
    fn memory_merge_keeps_added_lines(
        base in proptest::collection::vec("[a-z]{3,8}", 0..5),
        mine in proptest::collection::vec("[A-Z]{3,8}", 0..4),
        theirs in proptest::collection::vec("[0-9]{3,8}", 0..4),
    ) {
        let join = |extra: &[String]| base.iter().chain(extra).map(|s| format!("{s}\n")).collect::<String>();
        let merged = merge_lines(&join(&[]), &join(&mine), &join(&theirs));
        for x in mine.iter().chain(&theirs) {
            prop_assert!(merged.lines().any(|l| l == x), "{x} lost in {merged:?}");
        }
    }

    #[test]
    fn crlf_alone_pushes_nothing(text in "([a-z]{1,6}\n){1,6}", base in proptest::option::of("([a-z]{1,6}\n){1,6}")) {
        let path = "memory/home-proj/MEMORY.md".to_string();
        let one = |s: &str| FileSet::from([(path.clone(), s.as_bytes().to_vec())]);
        let b = base.map(|s| one(&s)).unwrap_or_default();
        let p = plan(&one(&text), &b, &one(&text.replace('\n', "\r\n")), &opts(true, true));
        prop_assert!(p.pushes.is_empty() && p.writes.is_empty() && p.conflicts.is_empty());
    }

    // 4. every secret pattern is found in random text; the allow marker passes
    #[test]
    fn secrets_found_and_allowed(
        which in 0..PATTERNS.len(),
        pre in "[a-z ]{0,20}", post in "[a-z ]{0,20}",
        seed in 0u64..1000,
    ) {
        let sample = sample_for(PATTERNS[which].0, seed);
        let line = format!("{pre}{sample}{post}");
        let hits = scan(&format!("ok\n{line}"));
        prop_assert_eq!(hits.len(), 1);
        prop_assert_eq!(hits[0].line, 2);
        let allowed = scan(&format!("{} {}", line, ALLOW_MARKER));
        prop_assert!(allowed.is_empty());
    }

    // 5. schedule limits hold for random input
    #[test]
    fn schedule_limits(n in 0usize..40, prompt_len in 0usize..6000, timeout in 0u64..1000) {
        let jobs: Vec<_> = (0..n).map(|i| serde_json::json!({
            "id": format!("job-{i}"), "cron": "0 9 * * *",
            "prompt": "x".repeat(prompt_len), "timeoutMinutes": timeout,
        })).collect();
        let (ok, _) = validate(&serde_json::json!({ "jobs": jobs }));
        prop_assert!(ok.len() <= MAX_JOBS);
        for j in &ok {
            prop_assert!(j.prompt.len() <= MAX_PROMPT);
            prop_assert!(j.timeout_minutes.is_none_or(|t| t <= u64::from(MAX_TIMEOUT)));
        }
    }

    // 6. no planned write leaves the managed folders
    #[test]
    fn writes_stay_in_managed_folders(
        paths in proptest::collection::vec("(skills|agents|commands|plugins|\\.\\.|/etc|[a-z]{1,3})(/(\\.\\.|[a-z]{1,3}|\\.|C:|a\\\\b|[a-z]{1,3}\\.md)){0,3}", 1..6),
        personal in any::<bool>(),
    ) {
        let r: FileSet = paths.iter().map(|p| (p.clone(), b"x".to_vec())).collect();
        let mut o = opts(personal, true);
        o.links.insert("skills/a".into());
        let p = plan(&r, &FileSet::new(), &FileSet::new(), &o);
        let all = p.writes.keys().chain(p.deletes.iter()).chain(p.pushes.keys());
        for k in all {
            let ok = if personal { is_personal_path(k) } else { is_planned_path(k) };
            prop_assert!(ok);
            prop_assert!(!k.split('/').any(|s| s == ".." || s == "." || s.is_empty()) && !k.starts_with('/'));
            prop_assert!(!k.starts_with("skills/a/"));
        }
    }
}

fn sample_for(name: &str, n: u64) -> String {
    let alnum = |len: usize| {
        (0..len)
            .map(|i| (b'a' + ((n as usize + i) % 26) as u8) as char)
            .collect::<String>()
    };
    match name {
        "github_pat" => format!("github_pat_{}", alnum(55)),
        "sk-ant" => format!("sk-ant-{}", alnum(24)),
        "ghp" => format!("ghp_{}", alnum(38)),
        "gh-token" => format!("ghs_{}", alnum(38)),
        "aws-access-key" => format!("AKIA{}", alnum(16).to_uppercase()),
        "slack-token" => format!("xoxb-{}", alnum(14)),
        "private-key" => "-----BEGIN RSA PRIVATE KEY-----".into(),
        _ => "\"refresh_token\": \"v\"".into(),
    }
}
