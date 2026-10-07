---
name: "claude-sync Phase 4 – Claude Code plugin"
overview: "Phase 4 of 7. Package claude-sync as a Claude Code plugin with its own marketplace: SessionStart/SessionEnd hooks, /config-sync:setup, :status and :sync commands (the plugin is named config-sync because Claude Code reserves claude-* plugin names), and install-cli. Scope is limited to this phase; finish when the gate passes, record evidence in PROGRESS.md and stop. Gate: the plugin validates, installs from the local marketplace, and its hooks run pull/push in a real Claude Code session on one named OS."
todos:
  - id: p04-0-preflight
    content: "Preflight: read PROGRESS.md, confirm Phase 3 gate evidence; read SPEC.md §3, §10, §11, §17; check the current Claude Code plugin docs (manifest, marketplace.json, hooks location, ${CLAUDE_PLUGIN_ROOT}, how hooks run on Windows, hook timeouts) and record any difference from §10 in Notes / decisions"
    status: pending
  - id: p04-1-name
    content: "Resolve the name question in §17 with the user before writing the manifest (stop and ask if not already decided in PROGRESS.md Notes)"
    status: pending
    dependencies: [p04-0-preflight]
  - id: p04-2-manifest
    content: ".claude-plugin/plugin.json and .claude-plugin/marketplace.json (this repo is its own marketplace); version matches claude-sync --version"
    status: pending
    dependencies: [p04-1-name]
  - id: p04-3-hooks
    content: "hooks/hooks.json: SessionStart → pull --hook, SessionEnd → push --hook --detach (SessionEnd hooks are cancelled after about 1.5 s, so the push runs on in the background), via bash \"${CLAUDE_PLUGIN_ROOT}/bin/claude-sync\"; a SessionStart timeout that covers CLAUDE_SYNC_TIMEOUT; silent when not set up"
    status: pending
    dependencies: [p04-2-manifest]
  - id: p04-4-commands
    content: "commands/setup.md, status.md, sync.md per §10; setup shows any conflict list and asks before --merge/--replace"
    status: pending
    dependencies: [p04-2-manifest]
  - id: p04-5-install-cli
    content: "claude-sync install-cli: write ~/.local/bin/claude-sync shim that execs the installed plugin's bin/claude-sync; idempotent; prints a PATH hint when ~/.local/bin is not on PATH"
    status: pending
    dependencies: [p04-0-preflight]
  - id: p04-6-tests
    content: "Tests: hooks.json and plugin.json parse and point at files that exist; hook commands exit 0 with no output when not set up; install-cli shim works and is idempotent; version strings agree"
    status: pending
    dependencies: [p04-3-hooks, p04-4-commands, p04-5-install-cli]
  - id: p04-8-gate
    content: "Verify gate: bash tests/run.sh and shellcheck pass on the local machine; plugin validated and installed from the local marketplace; real session evidence for SessionStart pull and SessionEnd push (name the OS)"
    status: pending
    dependencies: [p04-6-tests]
  - id: p04-9-handoff
    content: "Record gate evidence in PROGRESS.md, mark Phase 4 complete there, open the PR and run the review loop, then hand off with `next-phase.sh` (new headless session) and stop"
    status: pending
    dependencies: [p04-8-gate]
---

# Phase 4 – Claude Code plugin

> Part of the claude-sync roadmap: [`00-roadmap.plan.md`](00-roadmap.plan.md). Spec: `SPEC.md` §3, §10, §11, §17. Rules in `.cursor/rules/` are binding, especially `05-phase-discipline.mdc`, `07-complete-search.mdc` (the plugin name is an identity string) and `10-shell.mdc`.

**Phase 4 of 7.** Previous: Phase 3 – init and join. Next: Phase 5 – doctor, uninstall, docs, v0.1.0.

## Depends on
- **Needs** Phase 3 gate passed (`/config-sync:setup` runs `init`/`join`; hooks rely on pull calling link). Check: `PROGRESS.md` row 3.
- **Needs** a decision on the product name (§17), recorded in `PROGRESS.md` Notes / decisions. Check: search Notes for "Name:". If it is missing, stop and ask the user.

## Before you start
- Every **Needs** line above must be met. If one is not, stop, quote it and say how you checked.
- Read the cited spec sections in full, and the current plugin docs (preflight todo).
- The plugin and marketplace are named `config-sync` and `config-sync-marketplace` (decided 2026-10-06, see `PROGRESS.md` Notes); the CLI, env vars, state dir, repo dir and repo keep `claude-sync`. If the product name itself ever changes from `claude-sync`, rename it everywhere first in its own commit, following `07-complete-search.mdc` (binary, env vars `CLAUDE_SYNC_*`, state dir, default repo dir `~/.claude-sync`, managed-block markers, backup dir, docs).

## Scope (do these, nothing else)
- `.claude-plugin/plugin.json`, `.claude-plugin/marketplace.json`.
- `hooks/hooks.json` (§10).
- `commands/setup.md`, `commands/status.md`, `commands/sync.md` (§10). They tell Claude which exact `claude-sync` command to run via `${CLAUDE_PLUGIN_ROOT}`; they must never tell Claude to edit `~/.claude` files directly.
- `install-cli` subcommand and its entry in `--help`.
- `push --hook --detach` (the SessionEnd hook runs it), and its entry in `--help`.
- Tests listed in the p04-6 todo.

## Out of scope (do NOT start these, even partially)
- Phase 5 – `doctor`, `uninstall`, the user-facing README, CHANGELOG, LICENSE, tagging → `phase-05-doctor-release.plan.md`
- Publishing to any public marketplace or making the repo public (the user does that).
- Phase 6 and 7 opt-ins → `phase-06-cursor-sync.plan.md`, `phase-07-shared-settings.plan.md`
- Everything in later phases (see `00-roadmap.plan.md`)

Do not create files, services, stubs, settings or UI that belong to a later phase. If something here seems to need later-phase work, write the smallest thing that unblocks this phase, then add a note under "Deferred" in `PROGRESS.md`.

## Gate
**`bash tests/run.sh` and `shellcheck` pass on the local machine, and on one real machine (name the OS):**
1. `claude plugin validate .` (or the current equivalent found in preflight) exits 0;
2. `/plugin marketplace add <path to this checkout>` and `/plugin install config-sync@config-sync-marketplace` (or `claude plugin marketplace add` / `claude plugin install`) succeed;
3. with a throwaway config repo set up by `/config-sync:setup` (use a temp `CLAUDE_SYNC_DIR` and a local bare remote, not the user's real config): starting a session updates `last-pull`, and ending a session after editing a synced skill creates a `sync:` commit on the bare remote;
4. with no config repo, starting and ending a session prints nothing from claude-sync.

Record the timestamps, `git log -1` from the bare remote, and the OS in `PROGRESS.md`. Steps that need an interactive Claude Code session are done by the user if Cursor cannot run them; say who ran them.

## When the gate passes
1. Mark every todo in this file `completed`.
2. In `PROGRESS.md`: mark Phase 4 done with the date and evidence, and update Active phases and Next from each remaining plan's Depends on.
3. Commit with a conventional message scoped `phase-04`, push branch `phase-04-plugin`, open the PR, and run the review loop in `.cursor/skills/phase/SKILL.md`.
4. **Hand off** after the merge: run `bash .cursor/skills/phase/next-phase.sh`, report the PR URL and its `NEXT:` line, and stop. Never start the next phase in the same chat.
