---
name: "claude-sync Phase 6 – Cursor config sync (opt-in)"
overview: "Phase 6 of 7. Opt-in sync of Cursor's file-based global config under cursor/ in the user repo, enabled by cursor=1 in claude-sync.conf. Scope is limited to this phase; finish when the gate passes, record evidence in PROGRESS.md and stop. Gate: with cursor=1 the Cursor files sync in the two-machine test on all three OSes; with it absent nothing Cursor-related is touched."
todos:
  - id: p06-0-preflight
    content: "Preflight: read PROGRESS.md, confirm Phase 3 gate evidence; read SPEC.md §4, §8, §9, §15; find out which ~/.cursor global paths are real files Cursor reads on macOS and Windows (docs + a real check), and record the list in Notes / decisions"
    status: pending
  - id: p06-1-conf
    content: "claude-sync.conf in the user repo: key=value parser (no eval), only key cursor for now; missing file means all opt-ins off"
    status: pending
    dependencies: [p06-0-preflight]
  - id: p06-2-allowlist
    content: "When cursor=1: add cursor/<item> to the push allowlist, link ~/.cursor/<item> per §8 rules, import in init/join per §9"
    status: pending
    dependencies: [p06-1-conf]
  - id: p06-3-tests
    content: "Tests: off by default (nothing under ~/.cursor touched); on: two-machine round trip of a Cursor skill; conflict handling matches §8; secret scan covers cursor/"
    status: pending
    dependencies: [p06-2-allowlist]
  - id: p06-4-docs
    content: "README section: enabling Cursor sync"
    status: pending
    dependencies: [p06-2-allowlist]
  - id: p06-8-gate
    content: "Verify gate: tests pass locally (name the OS) and in CI on all three OSes"
    status: pending
    dependencies: [p06-3-tests, p06-4-docs]
  - id: p06-9-handoff
    content: "Record gate evidence in PROGRESS.md, mark Phase 6 complete there, open the PR and run the review loop, then hand off with `next-phase.sh` (new headless session) and stop"
    status: pending
    dependencies: [p06-8-gate]
---

# Phase 6 – Cursor config sync (opt-in)

> Part of the claude-sync roadmap: [`00-roadmap.plan.md`](00-roadmap.plan.md). Spec: `SPEC.md` §4, §8, §9, §15. Rules in `.cursor/rules/` are binding.

**Phase 6 of 7.** Previous: Phase 5 – doctor, uninstall, docs, v0.1.0. Next: Phase 7 – Shared settings.

## Depends on
- **Needs** Phase 3 gate passed (reuses link, import and the allowlist). Check: `PROGRESS.md` row 3.
- **After** Phase 5 (order only; if Phase 5 is merged, also add the Cursor items to `doctor`, otherwise list that under Deferred for Phase 5's owner).

## Before you start
- Every **Needs** line above must be met. If one is not, stop, quote it and say how you checked.
- If the preflight finds Cursor keeps its global rules or skills somewhere that is not plain files (for example a settings database), sync only the plain-file paths and record what was left out and why.

## Scope (do these, nothing else)
- `claude-sync.conf` parsing (only the `cursor` key).
- Cursor paths in allowlist, link, import, status.
- README section.

## Out of scope (do NOT start these, even partially)
- Phase 7 – `settings.shared.json` and any other `claude-sync.conf` key → `phase-07-shared-settings.plan.md`
- Cursor's settings database, extensions, keybindings, or project-level `.cursor/` folders.

Do not create files, services, stubs, settings or UI that belong to a later phase. If something here seems to need later-phase work, write the smallest thing that unblocks this phase, then add a note under "Deferred" in `PROGRESS.md`.

## Gate
**`bash tests/run.sh` passes on ubuntu-latest, macos-latest and windows-latest in CI and locally**, with tests proving: (1) without `cursor=1` nothing under the temp `~/.cursor` changes; (2) with it, a Cursor skill made on machine A appears linked on machine B after push/pull; (3) a real local Cursor directory on B is a conflict until `join --merge`; (4) a secret inside `cursor/` blocks the push.

Prove it: paste the commands and results (counts, exit codes, OS, CI run URL) into `PROGRESS.md`.

## When the gate passes
1. Mark every todo in this file `completed`.
2. In `PROGRESS.md`: mark Phase 6 done with the date and evidence, and update Active phases and Next.
3. Commit with a conventional message scoped `phase-06`, push branch `phase-06-cursor-sync`, open the PR, and run the review loop in `.cursor/skills/phase/SKILL.md`.
4. **Hand off** after the merge: run `bash .cursor/skills/phase/next-phase.sh`, report the PR URL and its `NEXT:` line, and stop. Never start the next phase in the same chat.
