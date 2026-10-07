---
name: "claude-sync Phase 7 – Shared settings (opt-in)"
overview: "Phase 7 of 7. Opt-in settings.shared.json in the user repo, merged into ~/.claude/settings.json after each pull without removing local keys. Scope is limited to this phase; finish when the gate passes, record evidence in PROGRESS.md and stop. Gate: the merge tests pass on the local machine, and local keys always survive."
todos:
  - id: p07-0-preflight
    content: "Preflight: read PROGRESS.md, confirm Phase 3 gate evidence; read SPEC.md §4, §6, §13, §16; choose the JSON tool (options: require jq; use node/python when present; PowerShell ConvertFrom-Json on Windows) and record the choice and why in Notes / decisions before writing code"
    status: completed
  - id: p07-1-conf
    content: "claude-sync.conf key settings=1 (reuse the Phase 6 parser if merged, else add a key=value parser with no eval)"
    status: completed
    dependencies: [p07-0-preflight]
  - id: p07-2-merge
    content: "lib/settings.sh: after a successful pull with settings=1, deep-merge settings.shared.json into ~/.claude/settings.json (objects merged, arrays union without duplicates, shared wins on scalar conflicts, local-only keys never removed); back up settings.json first; invalid JSON on either side aborts with no write"
    status: completed
    dependencies: [p07-1-conf]
  - id: p07-3-tests
    content: "Tests: off by default; merge cases (new key, nested object, array union, scalar conflict, local-only key kept); invalid JSON leaves settings.json byte-identical; the JSON tool missing gives one clear message and no write"
    status: completed
    dependencies: [p07-2-merge]
  - id: p07-4-docs
    content: "README section: sharing settings, which keys make sense to share, and that settings.json itself never syncs"
    status: completed
    dependencies: [p07-2-merge]
  - id: p07-8-gate
    content: "Verify gate: bash tests/run.sh and shellcheck pass on the local machine; name that OS in PROGRESS.md"
    status: completed
    dependencies: [p07-3-tests, p07-4-docs]
  - id: p07-9-handoff
    content: "Record gate evidence in PROGRESS.md, mark Phase 7 complete there, open the PR and run the review loop, then hand off with `next-phase.sh` (new headless session) and stop"
    status: completed
    dependencies: [p07-8-gate]
---

# Phase 7 – Shared settings (opt-in)

> Part of the claude-sync roadmap: [`00-roadmap.plan.md`](00-roadmap.plan.md). Spec: `SPEC.md` §4, §6, §13, §16. Rules in `.cursor/rules/` are binding, especially `40-security.mdc`.

**Phase 7 of 7.** Previous: Phase 6 – Cursor config sync. Next: none.

## Depends on
- **Needs** Phase 3 gate passed (merge runs after `pull`, which by then calls `link`). Check: `PROGRESS.md` row 3.
- **After** Phase 6 (order only; shares `claude-sync.conf`. If Phase 6 isn't merged, add the parser here and Phase 6 reuses it).

## Before you start
- Every **Needs** line above must be met. If one is not, stop, quote it and say how you checked.
- This is the first feature that edits JSON and the first that writes `settings.json`. §13's "no jq, Python or Node" applies to the core; record how this opt-in departs from it.

## Scope (do these, nothing else)
- `settings=1` in `claude-sync.conf`, `settings.shared.json` in the push allowlist when on.
- `lib/settings.sh` merge, run after a successful pull.
- README section.

## Out of scope (do NOT start these, even partially)
- Syncing `settings.json` or `settings.local.json` themselves (never, §4).
- Removing keys from local settings, or a "pull settings from this machine into shared" command (record under Deferred if wanted).

Do not create files, services, stubs, settings or UI that belong to a later phase. If something here seems to need later-phase work, write the smallest thing that unblocks this phase, then add a note under "Deferred" in `PROGRESS.md`.

## Gate
**`bash tests/run.sh` and `shellcheck` pass on the local machine (name the OS)**, with the p07-3 tests, and a test proving a local-only key in `settings.json` survives ten merges.

Prove it: paste the commands and results (counts, exit codes, OS) into `PROGRESS.md`.

## When the gate passes
1. Mark every todo in this file `completed`.
2. In `PROGRESS.md`: mark Phase 7 done with the date and evidence.
3. Commit with a conventional message scoped `phase-07`, push branch `phase-07-shared-settings`, open the PR, and run the review loop in `.cursor/skills/phase/SKILL.md`.
4. **Stop** after the merge. Report the PR URL.
