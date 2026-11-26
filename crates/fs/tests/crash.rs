//! The kill test (v2-Plan step 5): stop the executor before every mutation (between the
//! renames and the base commit included), run the pass again, and the machine, the base and
//! the remote end where an uninterrupted pass ends, with no user file lost.
use claude_sync_engine::formats::group::SyncFlags;
use claude_sync_engine::plan::FileSet;
use claude_sync_engine::traits::{Head, IoResult, Remote};
use claude_sync_fs::pass::{GroupIn, Pass};
use claude_sync_fs::Home;
use claude_sync_store::{Db, PERSONAL};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

struct Fake {
    state: RefCell<(u32, FileSet)>,
    pushes: RefCell<u32>,
}
impl Fake {
    fn new(files: &[(&str, &str)]) -> Self {
        Fake {
            state: RefCell::new((
                1,
                files
                    .iter()
                    .map(|(p, b)| (p.to_string(), b.as_bytes().to_vec()))
                    .collect(),
            )),
            pushes: RefCell::new(0),
        }
    }
    fn set(&self, p: &str, b: Option<&str>) {
        let mut s = self.state.borrow_mut();
        s.0 += 1;
        match b {
            Some(b) => s.1.insert(p.into(), b.as_bytes().to_vec()),
            None => s.1.remove(p),
        };
    }
}
impl Remote for Fake {
    fn head(&self, etag: Option<&str>) -> IoResult<Option<Head>> {
        let sha = format!("c{}", self.state.borrow().0);
        Ok((etag != Some(sha.as_str())).then(|| Head {
            etag: Some(sha.clone()),
            sha,
        }))
    }
    fn files(&self, _: &str, _: &BTreeMap<String, String>) -> IoResult<FileSet> {
        Ok(self.state.borrow().1.clone())
    }
    fn push(
        &self,
        _: &str,
        pushes: &BTreeMap<String, Option<Vec<u8>>>,
        _: &str,
    ) -> IoResult<String> {
        let mut s = self.state.borrow_mut();
        s.0 += 1;
        for (p, b) in pushes {
            match b {
                Some(b) => s.1.insert(p.clone(), b.clone()),
                None => s.1.remove(p),
            };
        }
        *self.pushes.borrow_mut() += 1;
        Ok(format!("c{}", s.0))
    }
}

fn put(p: &Path, t: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, t).unwrap();
}

fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(d: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel.starts_with(".claude-sync/journal")
                || rel.contains("claude-sync.db")
                || rel.ends_with("sync.lock")
            {
                continue;
            }
            if p.is_dir() {
                walk(&p, root, out)
            } else {
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

const GROUP: &str = "acme/team";

/// Machine with a settled first pass, then the world moves: the group edits, adds and deletes
/// files and one the user also edited; the user adds a personal note.
fn world(home: &Path) -> (Fake, Fake, Db) {
    let g = Fake::new(&[
        ("skills/a/SKILL.md", "a1"),
        ("skills/gone/SKILL.md", "g1"),
        ("agents/x.md", "x1"),
        ("settings.shared.json", r#"{"model":"m1"}"#),
        ("CLAUDE.md", "team rules\n"),
    ]);
    let p = Fake::new(&[("README.md", "personal repo\n")]);
    std::fs::create_dir_all(home.join(".claude-sync")).unwrap();
    let db = Db::open(&home.join(".claude-sync").join("claude-sync.db")).unwrap();
    let h = Home::under(home);
    let pass = Pass {
        home: &h,
        db: &db,
        now: 1_800_000_000,
        pinned: Default::default(),
        crash_at: None,
    };
    let r = pass.run(
        &[GroupIn {
            repo: GROUP,
            host: false,
            flags: SyncFlags::default(),
            off: false,
            remote: &g,
        }],
        Some(&p),
        None,
        None,
        Duration::ZERO,
    );
    assert!(r.groups[0].1.is_ok(), "{:?}", r.groups);
    assert!(home.join(".claude/skills/a/SKILL.md").is_file());
    *p.pushes.borrow_mut() = 0;
    // the world moves
    g.set("skills/a/SKILL.md", Some("a2"));
    g.set("skills/gone/SKILL.md", None);
    g.set("skills/new/SKILL.md", Some("n1"));
    g.set("agents/x.md", Some("x2"));
    put(&home.join(".claude/agents/x.md"), "my own x"); // the user edited it too: a conflict
    put(&home.join(".claude/personal-state/notes.txt"), "my note\n"); // a personal push
    (g, p, db)
}

fn pass_once(home: &Path, g: &Fake, p: &Fake, db: &Db, crash: Option<usize>) -> bool {
    let h = Home::under(home);
    let pass = Pass {
        home: &h,
        db,
        now: 1_800_000_100,
        pinned: Default::default(),
        crash_at: crash,
    };
    let r = pass.run(
        &[GroupIn {
            repo: GROUP,
            host: false,
            flags: SyncFlags::default(),
            off: false,
            remote: g,
        }],
        Some(p),
        None,
        None,
        Duration::ZERO,
    );
    let crashed = r.groups.iter().any(|(_, x)| x.is_err()) || matches!(r.personal, Some(Err(_)));
    if crash.is_none() {
        assert!(!crashed, "{r:?}");
    }
    crashed
}

#[test]
fn a_kill_anywhere_converges_to_the_uninterrupted_end_state() {
    let refd = tempfile::tempdir().unwrap();
    let (g, p, db) = world(refd.path());
    pass_once(refd.path(), &g, &p, &db, None);
    let want = snapshot(refd.path());
    let want_g = g.state.borrow().1.clone();
    let want_p = p.state.borrow().1.clone();
    let want_base = db
        .load_base(GROUP, &refd.path().join(".claude-sync"))
        .unwrap()
        .unwrap()
        .files;
    let want_pbase = db
        .load_base(PERSONAL, &refd.path().join(".claude-sync"))
        .unwrap()
        .unwrap()
        .files;
    assert_eq!(want[".claude/agents/x.md"], b"x2");
    assert!(
        want.keys()
            .any(|k| k.contains("conflicts/") && k.ends_with("agents/x.md")),
        "{:?}",
        want.keys().collect::<Vec<_>>()
    );
    assert!(!want.contains_key(".claude/skills/gone/SKILL.md"));
    assert_eq!(*p.pushes.borrow(), 1);

    let mut stops = 0;
    for k in 0..60 {
        let dir = tempfile::tempdir().unwrap();
        let (g, p, db) = world(dir.path());
        if !pass_once(dir.path(), &g, &p, &db, Some(k)) {
            break; // k is past the last mutation
        }
        stops += 1;
        // no user file lost at the kill: the user's x.md is on disk or saved
        let mid = snapshot(dir.path());
        assert!(
            mid.values().any(|v| v == b"my own x") && mid.values().any(|v| v == b"my note\n"),
            "k={k}: a user file is gone"
        );
        pass_once(dir.path(), &g, &p, &db, None);
        assert_eq!(snapshot(dir.path()), want, "k={k}: machine differs");
        assert_eq!(g.state.borrow().1, want_g, "k={k}");
        assert_eq!(p.state.borrow().1, want_p, "k={k}: remote differs");
        assert_eq!(*p.pushes.borrow(), 1, "k={k}: pushed twice");
        let st = dir.path().join(".claude-sync");
        assert_eq!(
            db.load_base(GROUP, &st).unwrap().unwrap().files,
            want_base,
            "k={k}"
        );
        assert_eq!(
            db.load_base(PERSONAL, &st).unwrap().unwrap().files,
            want_pbase,
            "k={k}"
        );
        // and the next tick has nothing to do
        let before = snapshot(dir.path());
        pass_once(dir.path(), &g, &p, &db, None);
        assert_eq!(snapshot(dir.path()), before, "k={k}: not stable");
    }
    assert!(
        stops >= 8,
        "the scenario should have many stop points, had {stops}"
    );
}
