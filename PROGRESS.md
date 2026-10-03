# claude-sync build progress

Read this file **before** any work. It is the single source of truth for which phases are active and what blocks the rest. Rules: `.cursor/rules/05-phase-discipline.mdc`. Roadmap: `.cursor/plans/00-roadmap.plan.md`. Spec: `SPEC.md`.

**Active phases:** 3 – Init and join (not started) → `.cursor/plans/phase-03-init-join.plan.md`
**Next:** 4 – Plugin (blocked: Needs Phase 3 gate passed; Needs the name decision)

| Phase | Plan | Status | Gate evidence | Date |
|---|---|---|---|---|
| 1 | `phase-01-sync-engine.plan.md` | completed | `bash tests/run.sh` on Windows Git Bash (MINGW64_NT-10.0-26200, uname -r 3.6.9-b4195d69.x86_64): 26 passed, 0 failed, exit 0. `shellcheck -x bin/claude-sync lib/*.sh tests/*.sh` with shellcheck 0.11.0 on that same machine: exit 0, no findings. Three-OS CI for this commit is the pull request checks (ubuntu-latest, macos-latest, windows-latest). | 2026-10-03 |
| 2 | `phase-02-link-layer.plan.md` | completed | `bash tests/run.sh` on Windows Git Bash (MINGW64_NT-10.0-26200, uname -r 3.6.9-b4195d69.x86_64): 34 passed, 0 failed, exit 0. `shellcheck -x bin/claude-sync lib/*.sh tests/*.sh` with shellcheck 0.11.0 on that same machine: exit 0, no findings. Smoke on that OS: a temp repo's skill `claude-sync-phase2-smoke` was linked into the real `~/.claude/skills/` (`bin/claude-sync link`, exit 0); `claude -p "List the names of the skills available to you, one per line"` exited 0 and its first line was `claude-sync-phase2-smoke`; `unlink` removed that link; `~/.claude/CLAUDE.md` matched a byte copy taken before the smoke (`cmp`). Three-OS CI for this commit is the pull request checks (ubuntu-latest, macos-latest, windows-latest). | 2026-10-03 |
| 3 | `phase-03-init-join.plan.md` | not started | | |
| 4 | `phase-04-plugin.plan.md` | blocked: Needs Phase 3 gate passed; Needs the name decision | | |
| 5 | `phase-05-doctor-release.plan.md` | blocked: Needs Phase 4 gate passed; Needs the license decision | | |
| 6 | `phase-06-cursor-sync.plan.md` | blocked: Needs Phase 3 gate passed | | |
| 7 | `phase-07-shared-settings.plan.md` | blocked: Needs Phase 3 gate passed | | |

Status is `not started`, `in progress`, `completed`, or `blocked: <the unmet Needs line from the plan>`. A phase is ready to start when every Needs line in its plan's Depends on is met, whatever its number.

## Handoff (2026-10-03)

**Where we are**
- Phase 2 (link layer) is done on this branch: `lib/link.sh` plus `link`, `unlink` and `status` on `bin/claude-sync`. Pull still does not call link; that is Phase 3.
- After this pull request merges, Phase 3 (init and join) is next.
- The private repo is https://github.com/MehdiMamas/claude-sync.

**Who does what**
- Cursor: code changes, using `/phase` (next phase), `/phase <N>`, or `/phase resume`.
- Claude Code: orchestration. Checks PR, CI and review state, decides the next step, and updates this file and the plans.
- User: approves pushes, posts anything public, decides the name (before Phase 4) and license (before Phase 5), runs the real-machine checks in Phases 4 and 5, and can stop the automatic hand-off with `PHASE_AUTOCHAIN=0` (after each merge `next-phase.sh` starts the next phase in a new headless Cursor CLI session; log in `.git/phase-runs/`).

**Prompt to give Cursor**
```
/phase
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
- 2026-10-03: A path already staged outside the allowlist stays staged and is not part of the sync commit. A rename or copy is committed only when the new path is allowlisted; a rename out of the allowlist commits the deletion and leaves the new path staged. Push exits 4 when a merge, rebase, or unmerged index is present. Pull does too, and leaves that in-progress state untouched. A pull or push-retry whose autostash reapply conflicts is undone only when that command created the stash, and exits 4.
- 2026-10-03: Untracked non-allowlisted paths are reported once per path. The state dir file `reported-untracked` remembers them.
- 2026-10-03: Hook mode sends other stderr to the log (or `/dev/null` if the log cannot be opened), prints at most one line, and exits 0. Git output in the log has credentials stripped from URLs. `GCM_INTERACTIVE=never` is set only in hook mode.
- 2026-10-03: Phase 1 plan gate items 5, 10, and 12 now name the pre-staged path, empty-lock, and no-prompt cases the tests cover.
- 2026-10-03: Left open (review follow-ups, not this round): whether `push` scans commits the user made by hand, and whether never-sync names inside allowlisted dirs are refused by the engine as well as by the Phase 3 gitignore.
- 2026-10-03: `status` prints one stdout line per item (`CLAUDE.md`, `skills/<name>`, `agents`, `commands`, `output-styles`) as `linked`, `missing`, or `conflict`, then `branch`, `ahead`, `behind`, `dirty`, `last-pull`, and `last-push`. Ahead/behind come from the local upstream ref; status does not fetch. `skills/synced` is not listed.
- 2026-10-03: Dangling-link cleanup looks at direct children of `~/.claude` and at names inside a real `skills/` directory. It does not walk real directories such as `projects/`. A link is removed only when its target path is inside the repo dir and the target is gone.
- 2026-10-03: On Windows, directory links are junctions: `cmd.exe //c mklink //J`. MSYS rewrites a single `/J` into a drive path before `cmd` sees it, so the bash source uses `//J` and `//c`. Junctions are removed with `cmd.exe //c rmdir`, never `rm`.
- 2026-10-03: The CLAUDE.md import path is `~/…` when the repo dir is under `$HOME`, and the absolute `pwd -L` path otherwise. The tilde is text in the file, not a shell expansion.
