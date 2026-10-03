---
name: "claude-sync Phase 1 – Sync engine"
overview: "Phase 1 of 7. Sync engine: the bash dispatcher, pull/push against the user's repo, lock, hook mode, secret scan, the plain-bash test runner and CI. Scope is limited to this phase; finish when the gate passes, record evidence in PROGRESS.md and stop. Gate: bash tests/run.sh passes the pull/push/lock/secret/hook-mode tests on ubuntu, macos and windows CI, and shellcheck is clean."
todos:
  - id: p01-0-preflight
    content: "Preflight: read PROGRESS.md (no Needs for this phase); read SPEC.md §5, §6, §7, §11, §13, §14 and .cursor/rules/*"
    status: pending
  - id: p01-1-repo-hygiene
    content: "Extend .gitattributes (already LF for *.sh) to bin/* and tests/*; add .gitignore"
    status: pending
    dependencies: [p01-0-preflight]
  - id: p01-2-test-runner
    content: "tests/run.sh + tests/lib.sh: run every tests/*.test.sh, assertions, temp HOME + bare file:// remote helpers, 'N passed, M failed' summary, non-zero on failure"
    status: pending
    dependencies: [p01-0-preflight]
  - id: p01-3-dispatcher
    content: "bin/claude-sync: env defaults (CLAUDE_SYNC_DIR, CLAUDE_HOME, CLAUDE_SYNC_TIMEOUT, state dir), subcommand dispatch for pull, push, --version, --help; exit codes per §11"
    status: pending
    dependencies: [p01-0-preflight]
  - id: p01-4-sync
    content: "lib/sync.sh: pull and push per §6, including allowlist staging, rebase abort on failure, one retry on non-fast-forward, never force-push, network time limit, no git prompts"
    status: pending
    dependencies: [p01-3-dispatcher]
  - id: p01-5-lock-hook
    content: "Lock (mkdir + pid, stale after 10 min or dead pid) and --hook mode (always exit 0, one stderr line, details in the log)"
    status: pending
    dependencies: [p01-4-sync]
  - id: p01-6-secrets
    content: "lib/secrets.sh: scan staged added lines for the §7 patterns, report file:line:pattern-name without the value, honor 'claude-sync: allow-secret'; push aborts and unstages on a hit"
    status: pending
    dependencies: [p01-4-sync]
  - id: p01-7-tests-ci
    content: "Tests for every behavior listed under Gate, plus .github/workflows/ci.yml: shellcheck on ubuntu; bash tests/run.sh on ubuntu-latest, macos-latest, windows-latest (shell: bash)"
    status: pending
    dependencies: [p01-2-test-runner, p01-5-lock-hook, p01-6-secrets]
  - id: p01-8-gate
    content: "Verify gate: tests pass locally (name the OS) and in CI on all three OSes; shellcheck clean"
    status: pending
    dependencies: [p01-7-tests-ci]
  - id: p01-9-handoff
    content: "Record gate evidence in PROGRESS.md, mark Phase 1 complete there, open the PR and run the review loop, then hand off with `next-phase.sh` (new headless session) and stop"
    status: pending
    dependencies: [p01-8-gate]
---

# Phase 1 – Sync engine

> Part of the claude-sync roadmap: [`00-roadmap.plan.md`](00-roadmap.plan.md). Spec: `SPEC.md` §5, §6, §7, §11, §13, §14. Rules in `.cursor/rules/` are binding, especially `05-phase-discipline.mdc`, `10-shell.mdc` and `40-security.mdc`.

**Phase 1 of 7.** Previous: none. Next: Phase 2 – Link layer.

## Depends on
- No Needs. This is the first phase.

## Before you start
- Read the cited spec sections in full.
- The repo holds only planning files so far. There is no code to build on.

## Scope (do these, nothing else)
- Extend `.gitattributes` (it already forces LF on `*.sh`) to `bin/*` and `tests/*`, so Windows checkouts still run.
- `tests/run.sh` and `tests/lib.sh` (§14): assertions, a helper that makes a temp `HOME` with `CLAUDE_HOME`/`CLAUDE_SYNC_DIR` inside it, and a helper that makes a bare remote plus a clone of it. Tests never touch the real `~/.claude` or the network.
- `bin/claude-sync` dispatcher with `pull`, `push`, `--version`, `--help`. Unknown subcommand exits 1 with usage. Only the subcommands in this phase appear in `--help`.
- `lib/sync.sh`: `pull` and `push` exactly as §6, with the allowlist from §4 (`CLAUDE.md skills agents commands output-styles`). `pull` does **not** call `link` yet (Phase 3 wires that).
- Lock and hook mode (§6). State dir `${XDG_STATE_HOME:-$HOME/.local/state}/claude-sync/` holds `lock/`, `log`, `last-pull`, `last-push`.
- `lib/secrets.sh` (§7), called by `push` before committing.
- `.github/workflows/ci.yml` (§14).

## Out of scope (do NOT start these, even partially)
- Phase 2 – Link layer (`link`, `unlink`, `status`) → `phase-02-link-layer.plan.md`
- Phase 3 – `init`, `join`, user-repo templates, pull calling link → `phase-03-init-join.plan.md`
- Phase 4 – plugin manifest, hooks.json, slash commands, `install-cli` → `phase-04-plugin.plan.md`
- Phase 5 – `doctor`, `uninstall`, user-facing README, release → `phase-05-doctor-release.plan.md`
- Everything in later phases (see `00-roadmap.plan.md`)

Do not create files, services, stubs, settings or UI that belong to a later phase. If something here seems to need later-phase work, write the smallest thing that unblocks this phase, then add a note under "Deferred" in `PROGRESS.md`.

## Gate
**`bash tests/run.sh` passes on ubuntu-latest, macos-latest and windows-latest in CI and on the local machine, `shellcheck` reports nothing, and the tests cover at least:**
1. pull fast-forwards a change pushed by another clone;
2. pull while offline/unreachable remote: hook mode exits 0 with one stderr line, CLI mode exits 3;
3. pull hitting a rebase conflict leaves no rebase in progress and the working tree unchanged;
4. push with no allowlisted changes makes no commit;
5. push commits only allowlisted paths; an untracked `notes.txt` in the repo root stays untracked;
6. push after the remote moved ahead (non-conflicting) rebases once and succeeds;
7. push with a real conflict keeps the local commit, never force-pushes (remote head unchanged), CLI exits 4;
8. each §7 pattern blocks the push, prints file:line:pattern name, never the matched value, leaves nothing staged, CLI exits 5;
9. the `claude-sync: allow-secret` marker lets that line through;
10. a held fresh lock makes hook mode skip with exit 0; a stale lock (old or dead pid) is taken over;
11. missing repo dir: hook mode exits 0 silently, CLI exits 2;
12. git never prompts (a remote needing credentials fails fast, not hangs).

Prove it: paste `bash tests/run.sh` output (counts and exit code, with the OS) and the CI run URL into `PROGRESS.md`.

## When the gate passes
1. Mark every todo in this file `completed`.
2. In `PROGRESS.md`: mark Phase 1 done with the date and evidence, and update Active phases and Next from each remaining plan's Depends on.
3. Commit with a conventional message scoped `phase-01`, push branch `phase-01-sync-engine`, open the PR, and run the review loop in `.cursor/skills/phase/SKILL.md`.
4. **Hand off** after the merge: run `bash .cursor/skills/phase/next-phase.sh`, report the PR URL and its `NEXT:` line, and stop. Never start the next phase in the same chat.
