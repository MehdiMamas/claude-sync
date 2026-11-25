//! Packs: validation, host carry-over, the local-only plugin override, jobs, rule lines, placed
//! files, and the command line. Sample `pack.json` files are built here; nothing reads a real
//! group repo.
use super::ops::{self, Env};
use super::*;
use crate::jobs::tests::{Env as JobsEnv, SCOUT};
use crate::watch;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Mutex;

const PB: &str = "passbridge-signin@claude-mods";
const REFLECT: &str = "reflect@claude-mods";

/// A check that exits with `code` on every OS the tests run on.
fn exit_cmd(code: i32) -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/c".into(), format!("exit {code}")]
    } else {
        vec!["sh".into(), "-c".into(), format!("exit {code}")]
    }
}

fn manifest(id: &str, extra: Value) -> Value {
    let mut m = json!({
        "schema": 1,
        "id": id,
        "title": format!("Pack {id}"),
        "summary": format!("What {id} does."),
        "needs": [{"name": "An app", "url": "https://example.com/app"}],
        "os": ["windows", "macos", "linux"],
        "plugins": [],
        "jobs": [],
        "files": [],
        "claudeMd": "CLAUDE.md",
        "setup": "setup.md",
        "check": {"run": exit_cmd(0), "timeoutSeconds": 60},
        "hostCarryOver": true
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        if v.is_null() {
            m.as_object_mut().unwrap().remove(&k);
        } else {
            m[k] = v;
        }
    }
    m
}

/// The group repo's files: the packs, `settings.shared.json`, `schedules.json` and a skill.
fn repo(packs: &[(&str, Value)], settings: Value) -> FileSet {
    let mut f = FileSet::new();
    for (id, m) in packs {
        f.insert(format!("packs/{id}/pack.json"), m.to_string().into_bytes());
        f.insert(
            format!("packs/{id}/setup.md"),
            format!("Set up {id}.\n").into_bytes(),
        );
        if m.get("claudeMd").is_some() {
            f.insert(
                format!("packs/{id}/CLAUDE.md"),
                format!("- Rule of {id}.\n").into_bytes(),
            );
        }
    }
    f.insert(
        "settings.shared.json".into(),
        settings.to_string().into_bytes(),
    );
    f.insert("schedules.json".into(), SCOUT.as_bytes().to_vec());
    f.insert("skills/klawly/SKILL.md".into(), b"klawly\n".to_vec());
    f
}

fn plugins_settings(passbridge: Option<bool>) -> Value {
    let mut p = serde_json::Map::new();
    if let Some(v) = passbridge {
        p.insert(PB.into(), json!(v));
    }
    p.insert(REFLECT.into(), json!(false));
    json!({ "enabledPlugins": p })
}

/// The files the engine hands the apply layer: no repo-only paths.
fn apply_files(f: &FileSet) -> FileSet {
    f.iter()
        .filter(|(p, _)| !crate::group::is_repo_only(p))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}

struct Fx {
    e: JobsEnv,
    g: GroupId,
}

impl Fx {
    fn new() -> Self {
        let e = JobsEnv::new();
        let g = GroupId::new("acme", "team");
        Self { e, g }
    }
    fn paths(&self) -> &Paths {
        &self.e.paths
    }
    fn env(&self) -> Env<'_> {
        Env {
            home: &self.e.home,
            os: "linux",
            now_ms: 1_000,
        }
    }
    fn prepare(&self, f: &FileSet, host: bool) -> PrepareReport {
        let p = Prepare {
            home: &self.e.home,
            host: Some(host),
            now_ms: 1_000,
        };
        prepare(self.paths(), &self.g, f, &p).unwrap()
    }
    /// What the engine does for one sync: prepare, then apply.
    fn sync(&self, f: &FileSet, host: bool) {
        self.join(f);
        self.prepare(f, host);
        crate::apply::apply_group(
            self.paths(),
            &self.g,
            std::slice::from_ref(&self.g),
            &apply_files(f),
        )
        .unwrap();
    }
    fn join(&self, f: &FileSet) {
        let sched = f
            .get("schedules.json")
            .map(|b| String::from_utf8(b.clone()).unwrap());
        self.e.groups(&[(&self.g, sched.as_deref())]);
    }
    fn local(&self) -> Value {
        std::fs::read(self.paths().claude_home.join("settings.json"))
            .ok()
            .and_then(|b| crate::settings::parse_object(&b))
            .unwrap_or(json!({}))
    }
    fn write_local(&self, v: Value) {
        let p = self.paths().claude_home.join("settings.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
    fn plugin(&self, name: &str) -> Option<bool> {
        self.local()["enabledPlugins"][name].as_bool()
    }
    fn record(&self, id: &str) -> Option<Record> {
        state::record(self.paths(), &self.g, id).unwrap()
    }
    fn claude_md(&self) -> String {
        std::fs::read_to_string(self.paths().claude_home.join("CLAUDE.md")).unwrap_or_default()
    }
    fn state_of(&self, id: &str) -> String {
        ops::list(self.paths(), &self.g, &self.env())
            .unwrap()
            .packs
            .into_iter()
            .find(|p| p.id == id)
            .unwrap()
            .state
            .to_string()
    }
}

fn catalog_of(f: &FileSet) -> Catalog {
    parse_catalog(&GroupId::new("acme", "team"), f, Path::new("/home/me"))
}

fn problem_of(f: &FileSet, id: &str) -> String {
    catalog_of(f)
        .problems
        .into_iter()
        .find(|p| p.id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("no problem for {id}"))
        .message
}

// ---------------------------------------------------------------- step 1: validation

#[test]
fn a_valid_pack_is_read_with_its_defaults() {
    let f = repo(
        &[(
            "passbridge",
            manifest(
                "passbridge",
                json!({"plugins": [PB], "os": null, "check": {"run": ["node", "check.mjs"]}, "hostCarryOver": null}),
            ),
        )],
        plugins_settings(Some(false)),
    );
    let c = catalog_of(&f);
    assert!(c.problems.is_empty(), "{:?}", c.problems);
    let p = c.pack("passbridge").unwrap();
    assert_eq!(p.title, "Pack passbridge");
    assert_eq!(p.os, ["windows", "macos", "linux"]);
    assert_eq!(p.check.timeout_seconds, DEFAULT_TIMEOUT);
    assert!(p.host_carry_over);
    assert_eq!(p.plugins, [PB]);
    assert!(p.group_enabled.is_empty());
    assert_eq!(p.hash.len(), 64);
}

#[test]
fn every_validation_rule_skips_the_pack_with_a_reason() {
    let ok = |id: &str, extra: Value| (id.to_string(), manifest(id, extra));
    let cases: Vec<(&str, (String, Value), &str)> = vec![
        (
            "bad folder id",
            ok("Bad", json!({"id": "Bad"})),
            "lowercase",
        ),
        (
            "id differs",
            ok("a", json!({"id": "b"})),
            "must equal the folder",
        ),
        ("schema", ok("a", json!({"schema": 2})), "schema 2"),
        (
            "title long",
            ok("a", json!({"title": "x".repeat(41)})),
            "title",
        ),
        ("title empty", ok("a", json!({"title": " "})), "title"),
        (
            "summary long",
            ok("a", json!({"summary": "x".repeat(201)})),
            "summary",
        ),
        (
            "need url",
            ok("a", json!({"needs": [{"name": "x", "url": "http://x"}]})),
            "https://",
        ),
        ("os name", ok("a", json!({"os": ["beos"]})), "os \"beos\""),
        ("os empty", ok("a", json!({"os": []})), "os is empty"),
        (
            "plugin shape",
            ok("a", json!({"plugins": ["nope"]})),
            "name@marketplace",
        ),
        (
            "plugin value",
            ok("a", json!({"plugins": ["x@m"]})),
            "not a true or false",
        ),
        (
            "unknown job",
            ok("a", json!({"jobs": ["ghost"]})),
            "not a valid job",
        ),
        (
            "file shape",
            ok("a", json!({"files": ["commands/x.md"]})),
            "skills/<name>",
        ),
        (
            "file missing",
            ok("a", json!({"files": ["skills/none"]})),
            "not in the group",
        ),
        (
            "claudeMd name",
            ok("a", json!({"claudeMd": "RULES.md"})),
            "claudeMd",
        ),
        (
            "setup name",
            ok("a", json!({"setup": "x.md"})),
            "setup must be",
        ),
        (
            "check empty",
            ok("a", json!({"check": {"run": []}})),
            "1 to 8",
        ),
        (
            "check long",
            ok(
                "a",
                json!({"check": {"run": ["a","b","c","d","e","f","g","h","i"]}}),
            ),
            "1 to 8",
        ),
        (
            "timeout low",
            ok("a", json!({"check": {"run": ["x"], "timeoutSeconds": 0}})),
            "1 to 120",
        ),
        (
            "timeout high",
            ok("a", json!({"check": {"run": ["x"], "timeoutSeconds": 121}})),
            "1 to 120",
        ),
    ];
    for (name, (id, m), want) in cases {
        let mut f = repo(
            &[(&id.to_lowercase(), m.clone())],
            json!({"enabledPlugins": {"x@m": "yes"}}),
        );
        // `Bad` is a folder name here, not the lowercased one.
        if id == "Bad" {
            f.insert("packs/Bad/pack.json".into(), m.to_string().into_bytes());
            f.insert("packs/Bad/setup.md".into(), b"x".to_vec());
        }
        let folder = if id == "Bad" {
            "Bad".to_string()
        } else {
            id.to_lowercase()
        };
        let msg = problem_of(&f, &folder);
        assert!(
            msg.contains(want),
            "{name}: {msg:?} does not mention {want:?}"
        );
        assert!(catalog_of(&f).packs.is_empty(), "{name}");
    }
}

#[test]
fn setup_and_manifest_files_are_checked() {
    let m = manifest("a", json!({}));
    let mut f = repo(&[("a", m.clone())], json!({}));
    f.remove("packs/a/setup.md");
    assert!(problem_of(&f, "a").contains("setup.md"));
    f.insert("packs/a/setup.md".into(), vec![b'x'; MAX_SETUP + 1]);
    assert!(problem_of(&f, "a").contains("at most"));
    f.insert("packs/a/setup.md".into(), Vec::new());
    assert!(problem_of(&f, "a").contains("empty"));
    let mut f = repo(&[("a", m.clone())], json!({}));
    f.remove("packs/a/CLAUDE.md");
    assert!(problem_of(&f, "a").contains("not in the pack folder"));
    let mut f = repo(&[("a", m)], json!({}));
    f.insert("packs/a/pack.json".into(), b"{not json".to_vec());
    assert!(problem_of(&f, "a").contains("not valid"));
    f.remove("packs/a/pack.json");
    assert!(problem_of(&f, "a").contains("missing"));
}

#[test]
fn a_job_a_plugin_or_a_file_belongs_to_one_pack() {
    let f = repo(
        &[
            (
                "a",
                manifest(
                    "a",
                    json!({"jobs": ["setup-scout"], "plugins": ["p@m"], "files": ["skills/klawly"]}),
                ),
            ),
            ("b", manifest("b", json!({"jobs": ["setup-scout"]}))),
            ("c", manifest("c", json!({"plugins": ["p@m"]}))),
            ("d", manifest("d", json!({"files": ["skills/klawly"]}))),
            ("e", manifest("e", json!({}))),
        ],
        json!({"enabledPlugins": {"p@m": false}}),
    );
    let c = catalog_of(&f);
    let ids: Vec<&str> = c.packs.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["a", "e"], "{:?}", c.problems);
    assert!(problem_of(&f, "b").contains("job setup-scout"));
    assert!(problem_of(&f, "c").contains("plugin p@m"));
    assert!(problem_of(&f, "d").contains("file skills/klawly"));
}

#[test]
fn a_bad_pack_is_listed_and_the_others_load() {
    let f = repo(
        &[
            ("good", manifest("good", json!({}))),
            ("bad", manifest("bad", json!({"os": ["beos"]}))),
        ],
        json!({}),
    );
    let c = catalog_of(&f);
    assert_eq!(c.packs.len(), 1);
    assert_eq!(c.problems.len(), 1);
    assert_eq!(c.problems[0].id.as_deref(), Some("bad"));
}

#[test]
fn at_most_twenty_packs_and_they_sort_by_order_then_id() {
    let packs: Vec<(String, Value)> = (0..22)
        .map(|i| {
            let id = format!("p{i:02}");
            (
                id.clone(),
                manifest(&id, json!({"order": if i == 5 { -1 } else { 0 }})),
            )
        })
        .collect();
    let refs: Vec<(&str, Value)> = packs.iter().map(|(i, m)| (i.as_str(), m.clone())).collect();
    let c = catalog_of(&repo(&refs, json!({})));
    assert_eq!(c.packs.len(), MAX_PACKS);
    assert_eq!(c.packs[0].id, "p05");
    assert_eq!(c.packs[1].id, "p00");
    assert_eq!(c.problems.len(), 2);
    assert!(c.problems[0].message.contains("more than 20"));
}

#[test]
fn a_plugin_the_group_still_turns_on_is_listed_but_stays_on() {
    // The move to packs is not done until the group sets the plugins to false.
    let f = repo(
        &[("a", manifest("a", json!({"plugins": [PB]})))],
        plugins_settings(Some(true)),
    );
    let c = catalog_of(&f);
    assert!(c.problems.is_empty());
    assert_eq!(c.packs[0].group_enabled, [PB]);
}

#[test]
fn the_hash_changes_with_the_folder_the_files_and_the_plugin_entries() {
    let base = |settings: Value, skill: &[u8], setup: &str| {
        let mut f = repo(
            &[(
                "a",
                manifest("a", json!({"plugins": ["p@m"], "files": ["skills/klawly"]})),
            )],
            settings,
        );
        f.insert("skills/klawly/SKILL.md".into(), skill.to_vec());
        f.insert("packs/a/setup.md".into(), setup.as_bytes().to_vec());
        catalog_of(&f).packs[0].hash.clone()
    };
    let s = json!({"enabledPlugins": {"p@m": false}, "extraKnownMarketplaces": {"m": {"v": 1}}});
    let same = base(s.clone(), b"k", "s");
    assert_eq!(same, base(s.clone(), b"k", "s"));
    assert_ne!(same, base(s.clone(), b"k2", "s"));
    assert_ne!(same, base(s.clone(), b"k", "s2"));
    let s2 = json!({"enabledPlugins": {"p@m": false}, "extraKnownMarketplaces": {"m": {"v": 2}}});
    assert_ne!(same, base(s2, b"k", "s"));
    // An unrelated setting does not count.
    let s3 = json!({"enabledPlugins": {"p@m": false, "other@x": true}, "extraKnownMarketplaces": {"m": {"v": 1}}});
    assert_eq!(same, base(s3, b"k", "s"));
}

#[test]
fn pack_files_are_allowlisted_and_repo_only() {
    assert!(crate::group::is_allowlisted("packs/passbridge/pack.json"));
    assert!(crate::group::is_repo_only("packs/passbridge/pack.json"));
    assert!(!crate::group::is_repo_only("skills/klawly/SKILL.md"));
    let gf = crate::group::GroupFile::new("n", "h");
    assert!(gf.syncs("packs/a/pack.json"));
}

// ---------------------------------------------------------------- a group without packs

#[test]
fn a_group_without_packs_changes_nothing() {
    let fx = Fx::new();
    let f = repo(&[], plugins_settings(Some(true)));
    fx.sync(&f, true);
    assert_eq!(fx.plugin(PB), Some(true));
    assert!(!fx.paths().group_dir(&fx.g).join("packs").exists());
    assert!(!fx
        .paths()
        .group_dir(&fx.g)
        .join("packs.index.json")
        .exists());
    assert!(!state::file(fx.paths()).exists());
    assert!(pinned_pointers(fx.paths()).is_empty());
    assert!(skipped_files(fx.paths(), &fx.g).is_empty());
    assert!(import_files(fx.paths(), &fx.g).is_empty());
    assert_eq!(job_gate(fx.paths(), &fx.g, "setup-scout"), None);
    assert!(ops::list(fx.paths(), &fx.g, &fx.env())
        .unwrap()
        .packs
        .is_empty());
    // The group's own CLAUDE.md block is not touched either.
    assert_eq!(fx.claude_md(), "");
}

// ---------------------------------------------------------------- step 2: host carry-over

fn host_files(passbridge: Option<bool>) -> FileSet {
    repo(
        &[
            (
                "passbridge",
                manifest("passbridge", json!({"plugins": [PB]})),
            ),
            (
                "reflect",
                manifest("reflect", json!({"plugins": [REFLECT]})),
            ),
            ("blueprint", manifest("blueprint", json!({}))),
            (
                "klawly",
                manifest(
                    "klawly",
                    json!({"files": ["skills/klawly"], "claudeMd": null}),
                ),
            ),
            (
                "manual",
                manifest("manual", json!({"hostCarryOver": false})),
            ),
        ],
        plugins_settings(passbridge),
    )
}

#[test]
fn the_hosts_first_sync_that_sees_packs_and_false_values_together_ends_on() {
    let fx = Fx::new();
    // Before packs: the group turned the plugin on for everyone; reflect is off on this machine.
    fx.sync(&repo(&[], json!({"enabledPlugins": {PB: true}})), true);
    fx.write_local(json!({"enabledPlugins": {PB: true, REFLECT: false}}));
    // The switch: packs arrive together with false values.
    let f = host_files(Some(false));
    let r = {
        fx.join(&f);
        fx.prepare(&f, true)
    };
    assert_eq!(r.carried.len(), 4, "{:?}", r.carried);
    crate::apply::apply_group(
        fx.paths(),
        &fx.g,
        std::slice::from_ref(&fx.g),
        &apply_files(&f),
    )
    .unwrap();
    assert_eq!(fx.plugin(PB), Some(true), "passbridge stays on");
    assert_eq!(fx.plugin(REFLECT), Some(false), "reflect stays off");
    assert_eq!(fx.state_of("passbridge"), "on");
    assert_eq!(fx.record("passbridge").unwrap().by, By::CarryOver);
    assert_eq!(fx.state_of("reflect"), "off");
    assert_eq!(fx.record("reflect").unwrap().by, By::CarryOver);
    // No plugin and no job: in use through its rule lines and files.
    assert_eq!(fx.state_of("blueprint"), "on");
    assert_eq!(fx.state_of("klawly"), "on");
    // hostCarryOver false: nothing is decided.
    assert_eq!(fx.record("manual"), None);
    // The skill of an on pack stays where it is; no check ran.
    assert!(fx
        .paths()
        .claude_home
        .join("skills/klawly/SKILL.md")
        .is_file());
    assert!(fx.record("passbridge").unwrap().last_check.is_none());
    // The override is not a local edit, and the next sync keeps it.
    assert_eq!(watch::local_edits(fx.paths()).unwrap(), vec![]);
    fx.sync(&f, true);
    assert_eq!(fx.plugin(PB), Some(true));
    assert_eq!(watch::local_edits(fx.paths()).unwrap(), vec![]);
}

#[test]
fn a_member_machine_gets_every_pack_off_and_the_groups_false() {
    let fx = Fx::new();
    fx.sync(&repo(&[], json!({"enabledPlugins": {PB: true}})), false);
    assert_eq!(fx.plugin(PB), Some(true));
    let f = host_files(Some(false));
    fx.sync(&f, false);
    assert_eq!(fx.plugin(PB), Some(false), "the group's false applies");
    for id in ["passbridge", "reflect", "blueprint", "klawly", "manual"] {
        assert_eq!(fx.record(id), None, "{id}");
        assert_eq!(fx.state_of(id), "available");
    }
    // A pack's files are not placed, the folders are there for set up.
    assert!(!fx.paths().claude_home.join("skills/klawly").exists());
    assert!(folder(fx.paths(), &fx.g, "passbridge")
        .join("setup.md")
        .is_file());
    assert!(!fx.claude_md().contains("packs"));
}

#[test]
fn a_hosts_accepted_job_counts_as_in_use() {
    let fx = Fx::new();
    let f = repo(
        &[(
            "scout",
            manifest("scout", json!({"jobs": ["setup-scout"], "claudeMd": null})),
        )],
        json!({}),
    );
    fx.join(&f);
    let job = fx.e.jobs().find(&fx.g, "setup-scout").unwrap();
    fx.e.jobs()
        .accept(&fx.g, "setup-scout", &job.consent_hash())
        .unwrap();
    assert_eq!(
        fx.e.registered(),
        ["setup-scout"],
        "registered before packs"
    );
    fx.prepare(&f, true);
    let r = fx.record("scout").unwrap();
    assert_eq!(r.state, PackState::On);
    assert_eq!(r.job_hashes["setup-scout"], job.consent_hash());
    // Same consent hash: the task stays registered.
    fx.e.sched.calls.lock().unwrap().clear();
    fx.e.jobs().reconcile(false).unwrap();
    assert_eq!(fx.e.registered(), ["setup-scout"]);
    assert!(fx.e.sched.calls.lock().unwrap().is_empty());
}

#[test]
fn a_host_job_turned_off_is_not_in_use() {
    let fx = Fx::new();
    let f = repo(
        &[(
            "scout",
            manifest("scout", json!({"jobs": ["setup-scout"], "claudeMd": null})),
        )],
        json!({}),
    );
    fx.join(&f);
    let job = fx.e.jobs().find(&fx.g, "setup-scout").unwrap();
    fx.e.jobs()
        .accept(&fx.g, "setup-scout", &job.consent_hash())
        .unwrap();
    fx.e.jobs()
        .set_enabled(&fx.g, "setup-scout", false)
        .unwrap();
    fx.prepare(&f, true);
    assert_eq!(fx.record("scout").unwrap().state, PackState::Off);
}

#[test]
fn a_pack_that_arrives_later_on_the_host_is_decided_by_host_auto() {
    let fx = Fx::new();
    fx.write_local(json!({"enabledPlugins": {PB: true}}));
    let first = repo(
        &[("blueprint", manifest("blueprint", json!({})))],
        json!({}),
    );
    fx.sync(&first, true);
    assert_eq!(fx.record("blueprint").unwrap().by, By::CarryOver);
    let second = repo(
        &[
            ("blueprint", manifest("blueprint", json!({}))),
            (
                "passbridge",
                manifest("passbridge", json!({"plugins": [PB]})),
            ),
        ],
        plugins_settings(Some(false)),
    );
    fx.sync(&second, true);
    let r = fx.record("passbridge").unwrap();
    assert_eq!((r.state, r.by), (PackState::On, By::HostAuto));
}

#[test]
fn leaving_the_group_forgets_the_records_and_the_override() {
    let fx = Fx::new();
    fx.sync(&repo(&[], json!({"enabledPlugins": {PB: true}})), true);
    let f = host_files(Some(false));
    fx.sync(&f, true);
    assert_eq!(fx.plugin(PB), Some(true));
    forget(fx.paths(), &fx.g).unwrap();
    assert!(state::load(fx.paths()).unwrap().groups.is_empty());
    crate::apply::remove_group(fx.paths(), &fx.g).unwrap();
    assert_eq!(fx.plugin(PB), None, "removed like the group's other values");
}

// ---------------------------------------------------------------- steps 3 to 6: what applies

fn member_ready(fx: &Fx, id: &str, f: &FileSet) {
    fx.sync(f, false);
    ops::start_setup(fx.paths(), &fx.g, id, &fx.env()).unwrap();
    let r = ops::check_pack(fx.paths(), &fx.g, id, &fx.env()).unwrap();
    assert!(r.passed, "{r:?}");
}

#[test]
fn turning_a_pack_on_and_off_follows_the_whole_lifecycle() {
    let fx = Fx::new();
    let f = repo(
        &[(
            "passbridge",
            manifest("passbridge", json!({"plugins": [PB]})),
        )],
        plugins_settings(Some(false)),
    );
    fx.sync(&f, false);
    assert_eq!(fx.state_of("passbridge"), "available");
    ops::start_setup(fx.paths(), &fx.g, "passbridge", &fx.env()).unwrap();
    assert_eq!(fx.state_of("passbridge"), "tried");
    // Turning on before the check passed is refused.
    let none = BTreeMap::new();
    assert!(ops::turn_on(fx.paths(), &fx.g, "passbridge", &none, &fx.env()).is_err());
    let r = ops::check_pack(fx.paths(), &fx.g, "passbridge", &fx.env()).unwrap();
    assert_eq!((r.passed, r.state), (true, "ready"));
    // Nothing is applied while it is only ready.
    assert_eq!(fx.plugin(PB), Some(false));

    let on = ops::turn_on(fx.paths(), &fx.g, "passbridge", &none, &fx.env()).unwrap();
    assert!(on.plugins_changed);
    assert_eq!(fx.plugin(PB), Some(true));
    assert_eq!(fx.state_of("passbridge"), "on");
    assert_eq!(pinned_pointers(fx.paths()).len(), 1);

    // The apply keeps it, the watcher calls it no edit, personal sync does not carry it.
    crate::apply::apply_group(
        fx.paths(),
        &fx.g,
        std::slice::from_ref(&fx.g),
        &apply_files(&f),
    )
    .unwrap();
    assert_eq!(fx.plugin(PB), Some(true));
    assert_eq!(watch::local_edits(fx.paths()).unwrap(), vec![]);
    let shared_copy = crate::settings::parse_object(
        &std::fs::read(fx.paths().group_dir(&fx.g).join("settings.shared.json")).unwrap(),
    )
    .unwrap();
    let owned = crate::apply::load_owned(fx.paths())
        .unwrap()
        .remove(&fx.g.to_string())
        .unwrap();
    assert_eq!(
        crate::settings::reconstruct_with(
            &shared_copy,
            &fx.local(),
            &owned,
            &pinned_pointers(fx.paths())
        ),
        None,
        "not reconstructed for Share"
    );
    let skip = crate::personal::settings_file::skip_set(fx.paths()).unwrap();
    let view = crate::personal::settings_file::view(&fx.local(), &skip);
    assert!(view.pointer("/enabledPlugins").is_none(), "{view}");
    // The rule line of the pack is in the managed block.
    assert!(
        fx.claude_md().contains("packs/passbridge/CLAUDE.md"),
        "{}",
        fx.claude_md()
    );

    // Off: the group's false is back, no line, state off, never asked again.
    assert!(ops::turn_off(fx.paths(), &fx.g, "passbridge", &fx.env()).unwrap());
    crate::apply::apply_group(
        fx.paths(),
        &fx.g,
        std::slice::from_ref(&fx.g),
        &apply_files(&f),
    )
    .unwrap();
    assert_eq!(fx.plugin(PB), Some(false));
    assert_eq!(fx.state_of("passbridge"), "off");
    assert!(!fx.claude_md().contains("passbridge"), "{}", fx.claude_md());
    assert_eq!(watch::local_edits(fx.paths()).unwrap(), vec![]);
    assert!(pinned_pointers(fx.paths()).is_empty());
    assert!(ops::turn_off(fx.paths(), &fx.g, "passbridge", &fx.env()).is_err());
    // Turn on again checks first: ready, then on.
    let again = ops::check_pack(fx.paths(), &fx.g, "passbridge", &fx.env()).unwrap();
    assert_eq!(again.state, "ready");
    ops::turn_on(fx.paths(), &fx.g, "passbridge", &none, &fx.env()).unwrap();
    assert_eq!(fx.plugin(PB), Some(true));
}

#[test]
fn a_real_edit_next_to_a_pack_override_is_still_an_edit() {
    let fx = Fx::new();
    let settings = json!({"enabledPlugins": {PB: false, "other@claude-mods": true}});
    let f = repo(
        &[(
            "passbridge",
            manifest("passbridge", json!({"plugins": [PB]})),
        )],
        settings,
    );
    member_ready(&fx, "passbridge", &f);
    ops::turn_on(fx.paths(), &fx.g, "passbridge", &BTreeMap::new(), &fx.env()).unwrap();
    let mut local = fx.local();
    local["enabledPlugins"]["other@claude-mods"] = json!(false);
    fx.write_local(local);
    let edits = watch::local_edits(fx.paths()).unwrap();
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert!(edits[0].path.ends_with("other@claude-mods"));
}

#[test]
fn a_failed_check_leaves_the_pack_tried_and_a_later_pass_makes_it_ready() {
    let fx = Fx::new();
    let failing = manifest(
        "a",
        json!({"check": {"run": exit_cmd(3), "timeoutSeconds": 10}}),
    );
    let f = repo(&[("a", failing)], json!({}));
    fx.sync(&f, false);
    ops::start_setup(fx.paths(), &fx.g, "a", &fx.env()).unwrap();
    let r = ops::check_pack(fx.paths(), &fx.g, "a", &fx.env()).unwrap();
    assert_eq!((r.passed, r.exit, r.state), (false, 3, "tried"));
    let v = &ops::list(fx.paths(), &fx.g, &fx.env()).unwrap().packs[0];
    assert_eq!(v.state, "tried");
    assert_eq!(v.last_check.as_ref().unwrap().exit, 3);
    assert!(ops::turn_on(fx.paths(), &fx.g, "a", &BTreeMap::new(), &fx.env()).is_err());
    // The pack is fixed upstream: the next check passes.
    let fixed = repo(&[("a", manifest("a", json!({})))], json!({}));
    fx.sync(&fixed, false);
    assert_eq!(
        ops::recheck_due(fx.paths(), &fx.g, &fx.env(), true).unwrap(),
        ["a"]
    );
    assert_eq!(fx.state_of("a"), "ready");
}

#[test]
fn only_tried_packs_are_rechecked_and_only_once_asked() {
    let fx = Fx::new();
    let f = repo(
        &[
            ("never", manifest("never", json!({}))),
            ("tried", manifest("tried", json!({}))),
            ("off", manifest("off", json!({}))),
        ],
        json!({}),
    );
    fx.sync(&f, false);
    // `tried` failed once; `off` was turned off; `never` has no record.
    state::update(fx.paths(), |s| {
        let g = s.groups.entry(fx.g.to_string()).or_default();
        let rec = |state, exit| Record {
            state,
            by: By::Setup,
            since: 1,
            pack_hash: "x".into(),
            last_check: Some(CheckResult {
                at: 1,
                exit,
                tail: "no".into(),
                hash: "x".into(),
            }),
            job_hashes: BTreeMap::new(),
        };
        g.insert("tried".into(), rec(PackState::Tried, 1));
        g.insert("off".into(), rec(PackState::Off, 0));
        Ok(())
    })
    .unwrap();
    assert!(ops::recheck_due(fx.paths(), &fx.g, &fx.env(), false)
        .unwrap()
        .is_empty());
    assert_eq!(
        ops::recheck_due(fx.paths(), &fx.g, &fx.env(), true).unwrap(),
        ["tried"]
    );
    assert_eq!(fx.state_of("never"), "available");
    assert_eq!(fx.state_of("off"), "off");
}

#[test]
fn a_changed_pack_that_is_on_is_rechecked_and_fails_without_turning_off() {
    let fx = Fx::new();
    let f = repo(
        &[(
            "passbridge",
            manifest("passbridge", json!({"plugins": [PB]})),
        )],
        plugins_settings(Some(false)),
    );
    member_ready(&fx, "passbridge", &f);
    ops::turn_on(fx.paths(), &fx.g, "passbridge", &BTreeMap::new(), &fx.env()).unwrap();
    assert!(ops::recheck_due(fx.paths(), &fx.g, &fx.env(), false)
        .unwrap()
        .is_empty());
    // The pack changes upstream and its new check fails here.
    let changed = repo(
        &[(
            "passbridge",
            manifest(
                "passbridge",
                json!({"plugins": [PB], "check": {"run": exit_cmd(1), "timeoutSeconds": 10}}),
            ),
        )],
        plugins_settings(Some(false)),
    );
    fx.sync(&changed, false);
    let v = ops::list(fx.paths(), &fx.g, &fx.env())
        .unwrap()
        .packs
        .remove(0);
    assert!(v.changed && v.state == "on");
    assert_eq!(
        ops::recheck_due(fx.paths(), &fx.g, &fx.env(), false).unwrap(),
        ["passbridge"]
    );
    assert_eq!(fx.state_of("passbridge"), "failing");
    // It stays applied: the plugin, the line and the pin.
    assert_eq!(fx.plugin(PB), Some(true));
    assert!(fx.claude_md().contains("packs/passbridge/CLAUDE.md"));
    // Not checked again until it changes once more.
    assert!(ops::recheck_due(fx.paths(), &fx.g, &fx.env(), false)
        .unwrap()
        .is_empty());
    // Turn off works from failing.
    ops::turn_off(fx.paths(), &fx.g, "passbridge", &fx.env()).unwrap();
    assert_eq!(fx.state_of("passbridge"), "off");
}

#[test]
fn the_check_that_ran_against_an_older_pack_does_not_allow_turn_on() {
    let fx = Fx::new();
    let f = repo(&[("a", manifest("a", json!({})))], json!({}));
    member_ready(&fx, "a", &f);
    let newer = repo(
        &[("a", manifest("a", json!({"summary": "Changed."})))],
        json!({}),
    );
    fx.sync(&newer, false);
    // The summary is in pack.json, a pack file: the hash moved.
    let err = ops::turn_on(fx.paths(), &fx.g, "a", &BTreeMap::new(), &fx.env()).unwrap_err();
    assert!(err.to_string().contains("run it again"), "{err}");
}

#[test]
fn a_pack_for_another_os_cannot_be_set_up() {
    let fx = Fx::new();
    let f = repo(
        &[(
            "busy",
            manifest("busy", json!({"os": ["windows", "linux"]})),
        )],
        json!({}),
    );
    fx.sync(&f, false);
    let mac = Env {
        os: "macos",
        ..fx.env()
    };
    let v = ops::list(fx.paths(), &fx.g, &mac).unwrap().packs.remove(0);
    assert_eq!(v.state, "unavailable");
    assert!(ops::start_setup(fx.paths(), &fx.g, "busy", &mac).is_err());
    assert!(ops::check_pack(fx.paths(), &fx.g, "busy", &mac).is_err());
    assert_eq!(fx.state_of("busy"), "available");
}

#[test]
fn files_of_a_pack_that_is_off_are_not_placed_and_go_when_it_turns_off() {
    let fx = Fx::new();
    let f = repo(
        &[(
            "klawly",
            manifest(
                "klawly",
                json!({"files": ["skills/klawly"], "claudeMd": null}),
            ),
        )],
        json!({}),
    );
    let skill = fx.paths().claude_home.join("skills/klawly/SKILL.md");
    fx.sync(&f, false);
    assert!(!skill.exists(), "off: not placed");
    member_ready(&fx, "klawly", &f);
    assert!(!skill.exists(), "ready: not placed");
    ops::turn_on(fx.paths(), &fx.g, "klawly", &BTreeMap::new(), &fx.env()).unwrap();
    crate::apply::apply_group(
        fx.paths(),
        &fx.g,
        std::slice::from_ref(&fx.g),
        &apply_files(&f),
    )
    .unwrap();
    assert!(skill.is_file(), "on: placed");
    ops::turn_off(fx.paths(), &fx.g, "klawly", &fx.env()).unwrap();
    crate::apply::apply_group(
        fx.paths(),
        &fx.g,
        std::slice::from_ref(&fx.g),
        &apply_files(&f),
    )
    .unwrap();
    assert!(!skill.exists(), "off again: removed because unchanged");
}

#[test]
fn an_edited_file_of_a_pack_that_turns_off_is_kept() {
    let fx = Fx::new();
    let f = repo(
        &[(
            "klawly",
            manifest(
                "klawly",
                json!({"files": ["skills/klawly"], "claudeMd": null}),
            ),
        )],
        json!({}),
    );
    member_ready(&fx, "klawly", &f);
    ops::turn_on(fx.paths(), &fx.g, "klawly", &BTreeMap::new(), &fx.env()).unwrap();
    crate::apply::apply_group(
        fx.paths(),
        &fx.g,
        std::slice::from_ref(&fx.g),
        &apply_files(&f),
    )
    .unwrap();
    let skill = fx.paths().claude_home.join("skills/klawly/SKILL.md");
    std::fs::write(&skill, b"mine\n").unwrap();
    ops::turn_off(fx.paths(), &fx.g, "klawly", &fx.env()).unwrap();
    crate::apply::apply_group(
        fx.paths(),
        &fx.g,
        std::slice::from_ref(&fx.g),
        &apply_files(&f),
    )
    .unwrap();
    assert_eq!(std::fs::read(&skill).unwrap(), b"mine\n");
}

#[test]
fn the_rule_lines_of_packs_that_are_on_come_in_pack_order_after_the_groups_own() {
    let fx = Fx::new();
    let mut f = repo(
        &[
            ("b", manifest("b", json!({"order": 2}))),
            ("a", manifest("a", json!({"order": 1}))),
            ("c", manifest("c", json!({"order": 3, "claudeMd": null}))),
        ],
        json!({}),
    );
    f.insert("CLAUDE.md".into(), b"group rules\n".to_vec());
    fx.sync(&f, true);
    let lines = crate::apply::block_lines(fx.claude_md().as_bytes());
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[0].ends_with("acme__team/CLAUDE.md"));
    assert!(lines[1].ends_with("packs/a/CLAUDE.md"));
    assert!(lines[2].ends_with("packs/b/CLAUDE.md"));
    // Syncing again changes nothing (stable), and turning one off removes only its line.
    let before = fx.claude_md();
    fx.sync(&f, true);
    assert_eq!(fx.claude_md(), before);
    ops::turn_off(fx.paths(), &fx.g, "a", &fx.env()).unwrap();
    crate::apply::apply_group(
        fx.paths(),
        &fx.g,
        std::slice::from_ref(&fx.g),
        &apply_files(&f),
    )
    .unwrap();
    let lines = crate::apply::block_lines(fx.claude_md().as_bytes());
    assert_eq!(lines.len(), 2);
    assert!(lines[1].ends_with("packs/b/CLAUDE.md"));
}

// ---------------------------------------------------------------- step 4: jobs

#[test]
fn a_packs_job_registers_only_where_the_pack_is_on() {
    let fx = Fx::new();
    let f = repo(
        &[(
            "scout",
            manifest("scout", json!({"jobs": ["setup-scout"], "claudeMd": null})),
        )],
        json!({}),
    );
    fx.sync(&f, false);
    assert_eq!(job_gate(fx.paths(), &fx.g, "setup-scout"), Some(false));
    // Even an accepted job of an off pack is not registered (an old acceptance, say).
    let job = fx.e.jobs().find(&fx.g, "setup-scout").unwrap();
    fx.e.jobs()
        .accept(&fx.g, "setup-scout", &job.consent_hash())
        .unwrap();
    assert!(fx.e.registered().is_empty());
    // The jobs card does not list it; the pack card does, with what the member accepts.
    assert!(fx.e.jobs().overview(&fx.g).unwrap().jobs.is_empty());
    let v = ops::list(fx.paths(), &fx.g, &fx.env())
        .unwrap()
        .packs
        .remove(0);
    assert_eq!(v.jobs[0].hash, job.consent_hash());

    member_ready(&fx, "scout", &f);
    assert!(fx.e.registered().is_empty(), "ready is not on");
    // A stale hash is refused: the member must see the job as it is.
    let stale: BTreeMap<_, _> = [("setup-scout".to_string(), "old".to_string())].into();
    assert!(ops::turn_on(fx.paths(), &fx.g, "scout", &stale, &fx.env()).is_err());
    let hashes: BTreeMap<_, _> = [("setup-scout".to_string(), job.consent_hash())].into();
    let on = ops::turn_on(fx.paths(), &fx.g, "scout", &hashes, &fx.env()).unwrap();
    for (id, hash) in &on.jobs {
        fx.e.jobs().accept(&fx.g, id, hash).unwrap();
    }
    assert_eq!(fx.e.registered(), ["setup-scout"]);
    let listed = fx.e.jobs().overview(&fx.g).unwrap().jobs;
    assert_eq!(listed[0].pack.as_deref(), Some("scout"));

    ops::turn_off(fx.paths(), &fx.g, "scout", &fx.env()).unwrap();
    fx.e.jobs().reconcile(false).unwrap();
    assert!(fx.e.registered().is_empty());
}

#[test]
fn jobs_in_no_pack_are_unchanged() {
    let fx = Fx::new();
    let f = repo(&[("a", manifest("a", json!({})))], json!({}));
    fx.sync(&f, false);
    assert_eq!(job_gate(fx.paths(), &fx.g, "setup-scout"), None);
    let job = fx.e.jobs().find(&fx.g, "setup-scout").unwrap();
    fx.e.jobs()
        .accept(&fx.g, "setup-scout", &job.consent_hash())
        .unwrap();
    assert_eq!(fx.e.registered(), ["setup-scout"]);
}

// ---------------------------------------------------------------- step 7: command line

struct FakeHost {
    claude: Option<PathBuf>,
    yes: bool,
    applied: Mutex<Vec<(String, Vec<String>)>>,
}

impl FakeHost {
    fn new(yes: bool) -> Self {
        Self {
            claude: Some(PathBuf::from("claude")),
            yes,
            applied: Mutex::new(Vec::new()),
        }
    }
}

impl cli::Host for FakeHost {
    fn claude(&self) -> Option<PathBuf> {
        self.claude.clone()
    }
    fn apply(&self, group: &GroupId, jobs: &[(String, String)]) -> crate::Result<()> {
        self.applied.lock().unwrap().push((
            group.to_string(),
            jobs.iter().map(|(j, _)| j.clone()).collect(),
        ));
        Ok(())
    }
    fn group_name(&self, _: &GroupId) -> String {
        "Team".into()
    }
    fn chat(&self, _: &Path, folder: &Path, message: &str) -> std::io::Result<i32> {
        assert!(folder.join("setup.md").is_file());
        assert!(message.starts_with("Read setup.md"));
        Ok(0)
    }
    fn confirm(&self, _: &str, out: &mut dyn std::io::Write) -> bool {
        let _ = writeln!(out, "(asked)");
        self.yes
    }
}

fn run_cli(fx: &Fx, host: &FakeHost, line: &str) -> (i32, String, String) {
    let args: Vec<String> = line.split_whitespace().map(String::from).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = cli::run(fx.paths(), host, &fx.env(), &args, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn cli_fx() -> (Fx, FileSet) {
    let fx = Fx::new();
    let f = repo(
        &[
            (
                "passbridge",
                manifest("passbridge", json!({"plugins": [PB]})),
            ),
            (
                "busy",
                manifest(
                    "busy",
                    json!({"check": {"run": exit_cmd(1), "timeoutSeconds": 10}, "claudeMd": null}),
                ),
            ),
            (
                "scout",
                manifest("scout", json!({"jobs": ["setup-scout"], "claudeMd": null})),
            ),
        ],
        plugins_settings(Some(false)),
    );
    fx.sync(&f, false);
    (fx, f)
}

#[test]
fn the_command_line_lists_packs_in_text_and_json() {
    let (fx, _) = cli_fx();
    let h = FakeHost::new(false);
    let (code, out, _) = run_cli(&fx, &h, "packs");
    // `packs` itself is stripped by the caller; here the first word is the command.
    assert_eq!(code, 2, "{out}");
    let (code, out, err) = run_cli(&fx, &h, "list");
    assert_eq!((code, err.as_str()), (0, ""), "{err}");
    assert!(out.contains("Team (acme/team), on this machine:"), "{out}");
    assert!(out.contains("passbridge"));
    assert!(out.contains("not set up"));
    assert!(out.contains("(scheduled job)"));
    assert!(out.contains("Set one up: claude-sync packs setup <id>"));
    let (code, out, _) = run_cli(&fx, &h, "list --json");
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["group"], "acme/team");
    assert_eq!(v["packs"].as_array().unwrap().len(), 3);
}

#[test]
fn the_command_line_exit_codes_are_0_1_and_2() {
    let (fx, _) = cli_fx();
    let h = FakeHost::new(true);
    assert_eq!(run_cli(&fx, &h, "frobnicate").0, 2);
    assert_eq!(run_cli(&fx, &h, "on").0, 2);
    assert_eq!(run_cli(&fx, &h, "on Not-An-Id").0, 2);
    let (code, _, err) = run_cli(&fx, &h, "list --group other/repo");
    assert_eq!(code, 1);
    assert!(err.contains("not in other/repo"), "{err}");
    let (code, _, err) = run_cli(&fx, &h, "check ghost");
    assert_eq!(code, 1);
    assert!(err.contains("no pack"), "{err}");
    // A failing check is exit 1 and says why; a passing one is 0.
    let (code, out, _) = run_cli(&fx, &h, "check busy");
    assert_eq!(code, 1);
    assert!(out.contains("check failed (exit 1)"), "{out}");
    let (code, out, _) = run_cli(&fx, &h, "check passbridge");
    assert_eq!(code, 0);
    assert!(out.contains("check passed"));
    // `off` for a pack that is not on is a failure.
    assert_eq!(run_cli(&fx, &h, "off passbridge").0, 1);
}

#[test]
fn two_groups_with_packs_need_a_group_flag() {
    let (fx, f) = cli_fx();
    let other = GroupId::new("acme", "other");
    let mut st = crate::state::AppState::load(fx.paths()).unwrap();
    st.groups.push(other.clone());
    st.save(fx.paths()).unwrap();
    prepare(
        fx.paths(),
        &other,
        &f,
        &Prepare {
            home: &fx.e.home,
            host: Some(false),
            now_ms: 1,
        },
    )
    .unwrap();
    let (code, _, err) = run_cli(&fx, &FakeHost::new(false), "list");
    assert_eq!(code, 2);
    assert!(err.contains("--group"), "{err}");
    assert_eq!(
        run_cli(&fx, &FakeHost::new(false), "list --group acme/other").0,
        0
    );
}

#[test]
fn setup_runs_the_chat_then_the_check_then_asks() {
    let (fx, _) = cli_fx();
    // The check fails: busy stays off, exit 1, and the way back is named.
    let h = FakeHost::new(true);
    let (code, out, _) = run_cli(&fx, &h, "setup busy");
    assert_eq!(code, 1);
    assert!(out.contains("Starting Claude Code in"), "{out}");
    assert!(out.contains("Running the pack's check ..."));
    assert!(out.contains("check failed (exit 1)"));
    assert!(out.contains("busy stays off on this machine"));
    assert!(out.contains("claude-sync packs check busy"));
    assert_eq!(fx.state_of("busy"), "tried");
    assert!(h.applied.lock().unwrap().is_empty());

    // Passing and answered no: ready, nothing turned on.
    let no = FakeHost::new(false);
    let (code, out, _) = run_cli(&fx, &no, "setup passbridge");
    assert_eq!(code, 0);
    assert!(out.contains("check passed"));
    assert!(out.contains("(asked)"));
    assert_eq!(fx.state_of("passbridge"), "ready");
    assert_eq!(fx.plugin(PB), Some(false));

    // `on` right after: no second question, turned on, applied.
    let (code, out, _) = run_cli(&fx, &h, "on passbridge");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("passbridge is on for this machine."));
    assert!(out.contains("/reload-plugins"));
    assert_eq!(fx.plugin(PB), Some(true));
    assert_eq!(h.applied.lock().unwrap().len(), 1);
    assert_eq!(run_cli(&fx, &h, "on passbridge").0, 0);

    let (code, out, _) = run_cli(&fx, &h, "off passbridge");
    assert_eq!(code, 0);
    assert!(out.contains("off for this machine"));
    assert_eq!(fx.plugin(PB), Some(false));
}

#[test]
fn setup_that_ends_in_yes_turns_the_pack_on_and_shows_the_job() {
    let (fx, _) = cli_fx();
    let h = FakeHost::new(true);
    let (code, out, _) = run_cli(&fx, &h, "setup scout");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("scheduled job Claude setup refresh"), "{out}");
    assert!(out.contains("bypassPermissions"));
    assert!(out.contains("scout is on for this machine."));
    assert_eq!(fx.state_of("scout"), "on");
    let applied = h.applied.lock().unwrap();
    assert_eq!(
        applied[0].1,
        ["setup-scout"],
        "the job was handed over to accept"
    );
}

#[test]
fn setup_without_claude_code_says_so_and_exits_1() {
    let (fx, _) = cli_fx();
    let mut h = FakeHost::new(true);
    h.claude = None;
    let (code, out, _) = run_cli(&fx, &h, "setup passbridge");
    assert_eq!(code, 1);
    assert!(out.contains("Claude Code isn't installed"), "{out}");
    // Check again does not need Claude.
    assert_eq!(run_cli(&fx, &h, "check passbridge").0, 0);
}
