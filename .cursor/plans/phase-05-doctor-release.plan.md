---
name: "claude-sync Phase 5 – doctor, uninstall, docs, v0.1.0"
overview: "Phase 5 of 7. doctor and uninstall, the user-facing README (quickstart, what syncs, security, troubleshooting), CHANGELOG and LICENSE, and a v0.1.0 release candidate. Scope is limited to this phase; finish when the gate passes, record evidence in PROGRESS.md and stop. Gate: a real Windows ⇄ macOS round trip from a clean install following only the README, and a release candidate ready for the user to tag."
todos:
  - id: p05-0-preflight
    content: "Preflight: read PROGRESS.md, confirm Phase 4 gate evidence and that the license (§17) is decided in Notes; read SPEC.md §1–§4, §7, §11, §12, §13"
    status: pending
  - id: p05-1-doctor
    content: "lib/doctor.sh: every check in §12, one line each with a fix, exit 0 when all pass and 2 otherwise"
    status: pending
    dependencies: [p05-0-preflight]
  - id: p05-2-uninstall
    content: "uninstall per §12: unlink, optional --restore-backup <ts>, never touches the repo dir or remote, prints the /plugin uninstall command"
    status: pending
    dependencies: [p05-0-preflight]
  - id: p05-3-tests
    content: "Tests: each doctor check passes and fails as expected (stale lock, mid-rebase repo, missing link, conflict, no remote); uninstall leaves the repo intact and restores a backup exactly"
    status: pending
    dependencies: [p05-1-doctor, p05-2-uninstall]
  - id: p05-4-docs
    content: "README.md (user-facing): what it is, 3-step quickstart, second machine, what syncs and what never does, security model, CLI reference, troubleshooting (incl. doctor), uninstall; CHANGELOG.md; LICENSE"
    status: pending
    dependencies: [p05-1-doctor, p05-2-uninstall]
  - id: p05-5-version
    content: "Set version 0.1.0 in bin/claude-sync and the plugin manifests"
    status: pending
    dependencies: [p05-4-docs]
  - id: p05-8-gate
    content: "Verify gate: CI green; README-only clean install round trip on Windows and macOS recorded"
    status: pending
    dependencies: [p05-3-tests, p05-5-version]
  - id: p05-9-handoff
    content: "Record gate evidence in PROGRESS.md, mark Phase 5 complete there, open the PR and run the review loop, then STOP and wait for the user (the user tags and publishes)"
    status: pending
    dependencies: [p05-8-gate]
---

# Phase 5 – doctor, uninstall, docs, v0.1.0

> Part of the claude-sync roadmap: [`00-roadmap.plan.md`](00-roadmap.plan.md). Spec: `SPEC.md` §1–§4, §7, §11, §12, §13. Rules in `.cursor/rules/` are binding.

**Phase 5 of 7.** Previous: Phase 4 – Claude Code plugin. Next: Phase 6 – Cursor config sync (opt-in).

## Depends on
- **Needs** Phase 4 gate passed (the README documents the plugin install flow). Check: `PROGRESS.md` row 4.
- **Needs** a license decision (§17) recorded in `PROGRESS.md` Notes / decisions. Check: search Notes for "License:". If missing, stop and ask the user.

## Before you start
- Every **Needs** line above must be met. If one is not, stop, quote it and say how you checked.
- Read the cited spec sections in full.

## Scope (do these, nothing else)
- `doctor` and `uninstall` (§12), added to the dispatcher and `--help`.
- Replace the planning README with the user-facing README. Move build/contributor notes (phasing, tests, CI) into `CONTRIBUTING.md`.
- `CHANGELOG.md` with a 0.1.0 entry; `LICENSE`.
- Version 0.1.0 everywhere it appears.

## Out of scope (do NOT start these, even partially)
- Creating the git tag, the GitHub release, making the repo public, or submitting to any marketplace (the user does these).
- Phase 6 – Cursor config sync → `phase-06-cursor-sync.plan.md`
- Phase 7 – Shared settings → `phase-07-shared-settings.plan.md`

Do not create files, services, stubs, settings or UI that belong to a later phase. If something here seems to need later-phase work, write the smallest thing that unblocks this phase, then add a note under "Deferred" in `PROGRESS.md`.

## Gate
**CI is green on all three OSes, and a real round trip passes on the user's Windows PC and Mac following only the README**: install the plugin from this branch's marketplace, `/claude-sync:setup` creates a fresh private test repo on Windows, the Mac joins it, an edit to a synced skill on the Mac appears on Windows after one session end + one session start, and `claude-sync doctor` exits 0 on both. Afterwards `claude-sync uninstall` on both leaves `~/.claude` working and the test repo intact.

The user runs the real-machine steps. Record who ran each step, both OS versions, the test repo name, and the `sync:` commit sha in `PROGRESS.md`.

## When the gate passes
1. Mark every todo in this file `completed`.
2. In `PROGRESS.md`: mark Phase 5 done with the date and evidence, and update Active phases and Next from each remaining plan's Depends on.
3. Commit with a conventional message scoped `phase-05`, push branch `phase-05-doctor-release`, open the PR, and run the review loop in `.cursor/skills/phase/SKILL.md`.
4. **Stop** after the merge. Report the PR URL and remind the user that tagging `v0.1.0` and publishing are theirs.
