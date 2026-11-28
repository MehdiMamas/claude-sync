//! Two devices that merge `memory/<key>/MEMORY.md` must settle: after a few passes nobody pushes,
//! whatever blank lines or line ends each side holds (a v1 field bug: a push every minute).
use claude_sync_engine::plan::{plan, FileSet, Opts, Scope};
use std::collections::BTreeSet;

const P: &str = "memory/home/MEMORY.md";

struct Dev {
    local: FileSet,
    base: FileSet,
}

fn opts() -> Opts {
    Opts {
        scope: Scope::Personal,
        push: true,
        links: BTreeSet::new(),
    }
}

fn set(t: &str) -> FileSet {
    FileSet::from([(P.to_string(), t.as_bytes().to_vec())])
}

/// One pass of one device; returns whether it pushed.
fn pass(d: &mut Dev, remote: &mut FileSet) -> bool {
    let pl = plan(remote, &d.base, &d.local, &opts());
    d.local.extend(pl.writes);
    for (p, v) in &pl.pushes {
        match v {
            Some(b) => remote.insert(p.clone(), b.clone()),
            None => remote.remove(p),
        };
    }
    d.base = pl.new_base;
    !pl.pushes.is_empty()
}

/// A and B take turns for `rounds` rounds; the number of pushes in the last 4 rounds.
fn late_pushes(a: &str, b: &str, base: &str, rounds: usize) -> (usize, FileSet, FileSet, FileSet) {
    let mut remote = set(base);
    let mut da = Dev {
        local: set(a),
        base: set(base),
    };
    let mut db = Dev {
        local: set(b),
        base: set(base),
    };
    let mut late = 0;
    for i in 0..rounds {
        let pa = pass(&mut da, &mut remote);
        let pb = pass(&mut db, &mut remote);
        if i >= rounds - 4 {
            late += usize::from(pa) + usize::from(pb);
        }
    }
    (late, da.local, db.local, remote)
}

const BASE: &str = "# Memory\n- one\n- two\n\n# Projects\n- p\n";

fn crlf(s: &str) -> String {
    s.replace('\n', "\r\n")
}

#[test]
fn a_blank_line_after_an_entry_before_the_projects_heading_settles() {
    let blank = "# Memory\n- one\n- two\n\n\n# Projects\n- p\n";
    let added = "# Memory\n- one\n- two\n- three\n\n# Projects\n- p\n";
    for (a, b) in [
        (blank.to_string(), added.to_string()),
        (added.to_string(), blank.to_string()),
        (crlf(blank), added.to_string()),
        (blank.to_string(), crlf(added)),
        (crlf(blank), crlf(added)),
        (blank.to_string(), BASE.to_string()),
        (crlf(blank), BASE.to_string()),
        (BASE.to_string(), crlf(blank)),
    ] {
        let (late, la, lb, remote) = late_pushes(&a, &b, BASE, 12);
        let strip = |m: &FileSet| {
            m[P].iter()
                .copied()
                .filter(|c| *c != b'\r')
                .collect::<Vec<u8>>()
        };
        assert_eq!(late, 0, "still pushing: a={a:?} b={b:?}");
        assert_eq!(strip(&la), strip(&lb), "a={a:?} b={b:?}");
        assert_eq!(strip(&la), strip(&remote), "a={a:?} b={b:?}");
    }
}

use proptest::prelude::*;

/// A text of lines from a small pool, with stray blank lines and either line end.
fn text() -> impl Strategy<Value = String> {
    (
        proptest::collection::vec((0..6usize, 0..3usize), 0..8),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(ls, crlf_end, final_nl)| {
            let pool = ["# Memory", "- one", "- two", "- three", "# Projects", "- p"];
            let mut t = String::new();
            for (n, blanks) in ls {
                t.push_str(pool[n]);
                t.push('\n');
                t.push_str(&"\n".repeat(blanks));
            }
            if !final_nl {
                t = t.trim_end_matches('\n').to_string();
            }
            if crlf_end {
                t = t.replace('\n', "\r\n");
            }
            t
        })
}

proptest! {
    /// Whatever two devices hold, after a few passes nobody pushes and both hold the same lines.
    #[test]
    fn two_devices_always_settle(a in text(), b in text()) {
        let (late, la, lb, remote) = late_pushes(&a, &b, BASE, 10);
        let norm = |m: &FileSet| String::from_utf8_lossy(&m[P]).replace('\r', "");
        prop_assert_eq!(late, 0, "a={:?} b={:?}", a, b);
        prop_assert_eq!(norm(&la), norm(&lb));
        prop_assert_eq!(norm(&la), norm(&remote));
    }
}

#[test]
fn one_devices_output_fed_to_the_other_comes_back_unchanged() {
    use claude_sync_engine::merge::merge_lines;
    let a = "# Memory\n- one\n- two\n\n\n# Projects\n- p\n"; // a blank line before the heading
    let b = "# Memory\n- one\n- two\n- three\n\n# Projects\n- p\n";
    let lf = |s: &str| s.replace("\r\n", "\n");
    for (a, b) in [
        (a.to_string(), b.to_string()),
        (crlf(a), b.to_string()),
        (a.to_string(), crlf(b)),
        (crlf(a), crlf(b)),
    ] {
        let from_a = merge_lines(BASE, &a, &b);
        let from_b = merge_lines(BASE, &b, &a);
        assert_eq!(lf(&from_a), lf(&from_b), "same bytes from either device");
        // each device, given the other's output as the remote, adds nothing
        assert_eq!(lf(&merge_lines(BASE, &b, &from_a)), lf(&from_a));
        assert_eq!(lf(&merge_lines(BASE, &a, &from_b)), lf(&from_b));
        assert_eq!(lf(&merge_lines(&from_a, &from_a, &from_b)), lf(&from_a));
    }
}
