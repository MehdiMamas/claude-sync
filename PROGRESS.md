# claude-sync build progress

Read this file **before** any work. It is the single source of truth for which phases are active and what blocks the rest. Rules: `.cursor/rules/05-phase-discipline.mdc`. Roadmap: `.cursor/plans/00-roadmap.plan.md`. Spec: `SPEC.md`.

**Active phases:** 1 – Sync engine (not started) → `.cursor/plans/phase-01-sync-engine.plan.md`
**Next:** 2 – Link layer (not started, Needs Phase 1)

| Phase | Plan | Status | Gate evidence | Date |
|---|---|---|---|---|
| 1 | `phase-01-sync-engine.plan.md` | not started | | |
| 2 | `phase-02-link-layer.plan.md` | blocked: Needs Phase 1 gate passed | | |
| 3 | `phase-03-init-join.plan.md` | blocked: Needs Phase 1 and Phase 2 gates passed | | |
| 4 | `phase-04-plugin.plan.md` | blocked: Needs Phase 3 gate passed; Needs the name decision | | |
| 5 | `phase-05-doctor-release.plan.md` | blocked: Needs Phase 4 gate passed; Needs the license decision | | |
| 6 | `phase-06-cursor-sync.plan.md` | blocked: Needs Phase 3 gate passed | | |
| 7 | `phase-07-shared-settings.plan.md` | blocked: Needs Phase 3 gate passed | | |

Status is `not started`, `in progress`, `completed`, or `blocked: <the unmet Needs line from the plan>`. A phase is ready to start when every Needs line in its plan's Depends on is met, whatever its number.

## Handoff (2026-10-03)

**Where we are**
- Planning only. Branch `plan/initial-phases` holds the spec, the roadmap, seven phase plans, rules, the `/phase` skill and the PR template. No code yet.
- The repo has no GitHub remote yet. Creating it (and whether it starts private) is the user's call.

**Who does what**
- Cursor: code changes, using `/phase` (next phase), `/phase <N>`, or `/phase resume`.
- Claude Code: orchestration. Checks PR, CI and review state, decides the next step, and updates this file and the plans.
- User: approves pushes, posts anything public, decides the name (before Phase 4) and license (before Phase 5), runs the real-machine checks in Phases 4 and 5, and decides when the next phase starts.

**Prompt to give Cursor**
```
/phase 1

Build Phase 1 (sync engine) from .cursor/plans/phase-01-sync-engine.plan.md: bin/claude-sync with pull/push, lib/sync.sh, lib/secrets.sh, the lock and --hook mode, the plain-bash test runner, and the CI workflow (shellcheck + tests on ubuntu, macos, windows). Read SPEC.md §5, §6, §7, §11, §13, §14 and .cursor/rules first. Every test uses a temp HOME and a local bare remote; never touch the real ~/.claude. Run bash tests/run.sh and record the counts, exit code, OS and CI run URL in PROGRESS.md, open the PR, run the review loop until Ready to merge, merge, and stop.
```

## Deferred
- (none yet)

## Notes / decisions
- 2026-10-03: The personal PC ⇄ Mac sync plan (`docs/history/personal-sync-plan.md`) became a public tool for anyone. That plan said "no phasing"; superseded now that it is a product.
- 2026-10-03: Ship as a **Claude Code plugin** (own marketplace) so hooks need no `settings.json` edits and no JSON tooling. Core is bash 3.2 + git only; Windows runs it through Git Bash, which Claude Code already requires.
- 2026-10-03: Each user's config lives in a **private repo they own**, cloned to `~/.claude-sync`. Allowlist of synced items plus a secret scan before every commit; never force-push; hook mode never blocks a session.
- 2026-10-03: `CLAUDE.md` is connected with a managed `@import` block instead of replacing the user's file; directories use symlinks (Unix) or junctions (Windows, no admin needed).
- 2026-10-03: Open: Name (working name `claude-sync`, decide before Phase 4). License (MIT assumed, decide before Phase 5).
