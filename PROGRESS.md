# claude-sync build progress

Read this file **before** any work. It is the single source of truth for which phases are active and what blocks the rest. Rules: `.cursor/rules/05-phase-discipline.mdc`. Roadmap: `.cursor/plans/00-roadmap.plan.md`. Spec: `SPEC.md`.

**Active phases:** 2 – Link layer (not started) → `.cursor/plans/phase-02-link-layer.plan.md`
**Next:** 3 – Init and join (blocked: Needs Phase 2 gate passed)

| Phase | Plan | Status | Gate evidence | Date |
|---|---|---|---|---|
| 1 | `phase-01-sync-engine.plan.md` | completed | `bash tests/run.sh` on Windows Git Bash (MINGW64_NT-10.0-26200 3.6.9): 14 passed, 0 failed, exit 0. `shellcheck -x bin/claude-sync lib/*.sh tests/*.sh` exit 0 (shellcheck 0.10.0). CI success on ubuntu-latest, macos-latest, and windows-latest for `d0c8fff`: https://github.com/MehdiMamas/claude-sync/actions/runs/37106115937 | 2026-10-03 |
| 2 | `phase-02-link-layer.plan.md` | not started | | |
| 3 | `phase-03-init-join.plan.md` | blocked: Needs Phase 2 gate passed | | |
| 4 | `phase-04-plugin.plan.md` | blocked: Needs Phase 3 gate passed; Needs the name decision | | |
| 5 | `phase-05-doctor-release.plan.md` | blocked: Needs Phase 4 gate passed; Needs the license decision | | |
| 6 | `phase-06-cursor-sync.plan.md` | blocked: Needs Phase 3 gate passed | | |
| 7 | `phase-07-shared-settings.plan.md` | blocked: Needs Phase 3 gate passed | | |

Status is `not started`, `in progress`, `completed`, or `blocked: <the unmet Needs line from the plan>`. A phase is ready to start when every Needs line in its plan's Depends on is met, whatever its number.

## Handoff (2026-10-03)

**Paused — resume the review, do not rebuild Phase 1 and do not start Phase 2.**

Stopped 2026-10-03 02:26 America/Chicago, before the first Claude review. Implementation, the pull request, and CI are done. Review rounds used: 0.

- PR: https://github.com/MehdiMamas/claude-sync/pull/1 (open, not a draft, base `main`)
- Branch: `phase-01-sync-engine`, worktree `C:\Users\Mehdi\Projects\claude-config-sync-plan-phase-01-sync-engine`. Leave `C:\Users\Mehdi\Projects\claude-config-sync-plan` on `plan/initial-phases`.
- Code head before this pause note: `d0c8fff960c6cbd1661d23c5f1fba752ac427569` (`fix(phase-01): keep the secret scan working on bash 3.2`)
- CI on that code is green: https://github.com/MehdiMamas/claude-sync/actions/runs/37106115937 (ubuntu, macos, windows). The earlier run https://github.com/MehdiMamas/claude-sync/actions/runs/37105865438 failed and is already fixed in `d0c8fff`.
- `bash .cursor/skills/phase/review.sh 1` did not review. On Git Bash, `git show origin/main:.cursor/skills/phase/review-prompt.md` is rewritten to `origin\main;.cursor\...` unless `MSYS_NO_PATHCONV=1` is set. With that set, Claude Code returned `You've hit your session limit · resets 4:10am (America/Chicago)` and wrote no report. Do not merge.

**Next session**
1. Wait until after 4:10am America/Chicago on 2026-10-03 so the Claude Code limit has reset.
2. In the phase worktree, confirm the tree is clean and `git rev-parse HEAD` equals `gh pr view 1 --json headRefOid`.
3. Run `MSYS_NO_PATHCONV=1 bash .cursor/skills/phase/review.sh 1`. That is Review loop step 1. Then follow the verdict (max 4 fix rounds). Merge only on **Ready to merge**, green CI, `MERGEABLE`, and the same sha.

**Where we are**
- Phase 1 (sync engine) is implemented on `phase-01-sync-engine`: `bin/claude-sync`, `lib/sync.sh`, `lib/secrets.sh`, the test runner, and CI. Pull does not call link; that is Phase 3.
- The private repo is https://github.com/MehdiMamas/claude-sync. `main` is the planning commit.

**Who does what**
- Cursor: code changes, using `/phase` (next phase), `/phase <N>`, or `/phase resume`.
- Claude Code: orchestration. Checks PR, CI and review state, decides the next step, and updates this file and the plans.
- User: approves pushes, posts anything public, decides the name (before Phase 4) and license (before Phase 5), runs the real-machine checks in Phases 4 and 5, and decides when the next phase starts.

**Prompt to give Cursor**
```
/phase resume
```

## Deferred
- `--version` prints `claude-sync 0.0.0`. Phase 5 (doctor and release) owns the real version.
- `pull` does not call `link` after a successful pull. Phase 3 wires that.

## Notes / decisions
- 2026-10-03: The personal PC ⇄ Mac sync plan (`docs/history/personal-sync-plan.md`) became a public tool for anyone. That plan said "no phasing"; superseded now that it is a product.
- 2026-10-03: Ship as a **Claude Code plugin** (own marketplace) so hooks need no `settings.json` edits and no JSON tooling. Core is bash 3.2 + git only; Windows runs it through Git Bash, which Claude Code already requires.
- 2026-10-03: Each user's config lives in a **private repo they own**, cloned to `~/.claude-sync`. Allowlist of synced items plus a secret scan before every commit; never force-push; hook mode never blocks a session.
- 2026-10-03: `CLAUDE.md` is connected with a managed `@import` block instead of replacing the user's file; directories use symlinks (Unix) or junctions (Windows, no admin needed).
- 2026-10-03: Open: Name (working name `claude-sync`, decide before Phase 4). License (MIT assumed, decide before Phase 5).
- 2026-10-03: Secret-scan pattern names are `github_pat`, `sk-ant`, `ghp`, `gh-token`, `aws-access-key`, `slack-token`, `private-key`, and `token-field`. Output is `file:line:name` and never the matched text.
- 2026-10-03: A push with a clean work tree still publishes commits that are ahead of upstream, so a push that committed and then failed to reach the remote is not stuck.
- 2026-10-03: `git add` is run per allowlist path. A pathspec that matches nothing is skipped; `git add` would otherwise fail before the paths that do exist are staged.
- 2026-10-03: Current Git rejects a non-fast-forward with `fetch first`, not only the older `non-fast-forward` wording. Both count as the one allowed rebase retry.
- 2026-10-03: bash 3.2 (macOS) marks `BASH_REMATCH` readonly, so the secret scan never assigns it. The matched text is not printed.
