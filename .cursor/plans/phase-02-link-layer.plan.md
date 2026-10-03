---
name: "claude-sync Phase 2 – Link layer"
overview: "Phase 2 of 7. Link layer: link, unlink and status connecting ~/.claude to the repo on macOS, Linux and Windows (junctions). Scope is limited to this phase; finish when the gate passes, record evidence in PROGRESS.md and stop. Gate: link/unlink/status tests pass on ubuntu, macos and windows CI, including proof that unlink never deletes the target."
todos:
  - id: p02-0-preflight
    content: "Preflight: read PROGRESS.md, confirm Phase 1 gate evidence; read SPEC.md §4, §8, §11, §13 and .cursor/rules/*"
    status: pending
  - id: p02-1-platform
    content: "lib/link.sh platform helpers: make_dir_link / remove_dir_link / is_link / link_target, with ln -s on Unix and mklink /J + rmdir via cmd on Windows (cygpath -w at the boundary only)"
    status: pending
    dependencies: [p02-0-preflight]
  - id: p02-2-claude-md
    content: "CLAUDE.md managed block: insert at top or update in place, create the file if missing, unlink removes only the block"
    status: pending
    dependencies: [p02-0-preflight]
  - id: p02-3-link
    content: "link: per-skill links (skip skills/synced), whole-dir links for agents/commands/output-styles, conflict reporting without touching local content, dangling-link cleanup limited to links into the repo dir"
    status: pending
    dependencies: [p02-1-platform, p02-2-claude-md]
  - id: p02-4-unlink-status
    content: "unlink (removes only our links and the block) and status (per-item linked/missing/conflict + branch, ahead/behind, dirty, last pull/push); add the three to the dispatcher and --help"
    status: pending
    dependencies: [p02-3-link]
  - id: p02-5-tests
    content: "Tests for every behavior listed under Gate, run in CI on all three OSes"
    status: pending
    dependencies: [p02-4-unlink-status]
  - id: p02-8-gate
    content: "Verify gate: tests pass locally (name the OS) and in CI on all three OSes; shellcheck clean"
    status: pending
    dependencies: [p02-5-tests]
  - id: p02-9-handoff
    content: "Record gate evidence in PROGRESS.md, mark Phase 2 complete there, open the PR and run the review loop, then STOP and wait for the user"
    status: pending
    dependencies: [p02-8-gate]
---

# Phase 2 – Link layer

> Part of the claude-sync roadmap: [`00-roadmap.plan.md`](00-roadmap.plan.md). Spec: `SPEC.md` §4, §8, §11, §13. Rules in `.cursor/rules/` are binding, especially `05-phase-discipline.mdc`, `10-shell.mdc` and `40-security.mdc`.

**Phase 2 of 7.** Previous: Phase 1 – Sync engine. Next: Phase 3 – init and join.

## Depends on
- **Needs** Phase 1 gate passed (this phase adds subcommands to `bin/claude-sync` and uses `tests/lib.sh` and CI from Phase 1). Check: `PROGRESS.md` row 1 is `completed` with evidence.

## Before you start
- Every **Needs** line above must be met. If one is not, stop, quote it and say how you checked.
- Read the cited spec sections in full.
- Build on `bin/claude-sync`, `tests/lib.sh` and the CI workflow from Phase 1. If the code differs from this plan, follow the code and note the difference in `PROGRESS.md`.

## Scope (do these, nothing else)
- `lib/link.sh` with the platform helpers and `link`, `unlink`, `status` exactly as §8.
- The CLAUDE.md managed block (§8): the import path is the real repo dir written in `~/…` form when it is under `$HOME`, absolute otherwise.
- Add `link`, `unlink`, `status` to the dispatcher and `--help`.
- Tests (below), running in the existing CI matrix.

## Out of scope (do NOT start these, even partially)
- Phase 3 – importing local content, backups, `init`, `join`, pull calling link → `phase-03-init-join.plan.md`. In this phase a real local directory is a **conflict to report**, never something to move.
- Phase 4 – plugin, hooks, slash commands → `phase-04-plugin.plan.md`
- Phase 5 – `doctor`, `uninstall` → `phase-05-doctor-release.plan.md`
- Phase 6 – Cursor paths → `phase-06-cursor-sync.plan.md`
- Everything in later phases (see `00-roadmap.plan.md`)

Do not create files, services, stubs, settings or UI that belong to a later phase. If something here seems to need later-phase work, write the smallest thing that unblocks this phase, then add a note under "Deferred" in `PROGRESS.md`.

## Gate
**`bash tests/run.sh` passes on ubuntu-latest, macos-latest and windows-latest in CI and locally, `shellcheck` is clean, and the new tests cover at least:**
1. link creates every expected link; running it twice changes nothing;
2. on Windows CI the skill links are junctions (check with `cmd //c dir /AL` or `fsutil reparsepoint query`), not copies;
3. a file written through the link appears in the repo dir, and the other way round;
4. `skills/synced/` and an unrelated local skill are left untouched;
5. a real local `agents/` directory or a link pointing elsewhere is reported as a conflict and left byte-for-byte unchanged;
6. CLAUDE.md: block inserted at top of an existing file with content preserved below it; updated in place when the repo dir changes; file created when missing; unlink removes only the block;
7. a skill deleted from the repo leaves a dangling link that the next `link` removes; a dangling link pointing outside the repo is kept;
8. **unlink never deletes the target**: after unlink every repo file still exists with the same content (on all three OSes);
9. status reports linked / missing / conflict correctly and shows ahead/behind and dirty.

Also run one smoke check on the local machine and record the OS: link a test skill from a temp repo into the real `~/.claude/skills/`, run `claude -p "List the names of the skills available to you, one per line"`, confirm the test skill appears, then unlink it.

Prove it: paste the commands and results (counts, exit codes, OS, CI run URL, smoke output) into `PROGRESS.md`.

## When the gate passes
1. Mark every todo in this file `completed`.
2. In `PROGRESS.md`: mark Phase 2 done with the date and evidence, and update Active phases and Next from each remaining plan's Depends on.
3. Commit with a conventional message scoped `phase-02`, push branch `phase-02-link-layer`, open the PR, and run the review loop in `.cursor/skills/phase/SKILL.md`.
4. **Stop** after the merge. Report the PR URL. Do not start the next phase.
