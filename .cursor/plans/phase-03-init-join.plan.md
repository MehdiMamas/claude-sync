---
name: "claude-sync Phase 3 – init and join"
overview: "Phase 3 of 7. init (first machine: create or attach the repo, import ~/.claude content with backups, link, push) and join (other machines: clone, resolve local content with --merge/--replace, link), and pull calling link. Scope is limited to this phase; finish when the gate passes, record evidence in PROGRESS.md and stop. Gate: the two-machine test passes on ubuntu, macos and windows CI."
todos:
  - id: p03-0-preflight
    content: "Preflight: read PROGRESS.md, confirm Phase 1 and Phase 2 gate evidence; read SPEC.md §3, §4, §7, §8, §9, §11 and .cursor/rules/*"
    status: pending
  - id: p03-1-templates
    content: "templates/user-repo/: .gitignore (never-sync list from §4), .gitattributes, README.md (what the repo is, how to join another machine)"
    status: pending
    dependencies: [p03-0-preflight]
  - id: p03-2-backup-import
    content: "lib/init.sh backup + import helpers: back up to ~/.claude/.claude-sync-backup/<UTC ts>/ preserving paths, move allowlisted items into the repo (CLAUDE.md body outside the managed block, skills except synced/ and links, agents, commands, output-styles)"
    status: pending
    dependencies: [p03-0-preflight]
  - id: p03-3-init
    content: "init per §9: refuse if already set up, --create via gh (private only), --remote, local-only; write templates; import; link; commit 'init: import from <host>'; push if remote"
    status: pending
    dependencies: [p03-1-templates, p03-2-backup-import]
  - id: p03-4-join
    content: "join per §9: clone; default stops and lists conflicts before changing anything; --merge; --replace; link; commit merged content; push"
    status: pending
    dependencies: [p03-2-backup-import]
  - id: p03-5-visibility
    content: "Public-repo guard (§7): init --create is always private; init --remote and join warn when gh reports the remote is public; skip quietly when gh is absent"
    status: pending
    dependencies: [p03-3-init, p03-4-join]
  - id: p03-6-pull-link
    content: "pull runs link after a successful pull (§6 step 4); link errors in hook mode stay one stderr line and exit 0"
    status: pending
    dependencies: [p03-0-preflight]
  - id: p03-7-tests
    content: "Tests for every behavior listed under Gate, including the two-machine test; gh is stubbed with a fake on PATH, never called for real"
    status: pending
    dependencies: [p03-3-init, p03-4-join, p03-5-visibility, p03-6-pull-link]
  - id: p03-8-gate
    content: "Verify gate: tests pass locally (name the OS) and in CI on all three OSes; shellcheck clean"
    status: pending
    dependencies: [p03-7-tests]
  - id: p03-9-handoff
    content: "Record gate evidence in PROGRESS.md, mark Phase 3 complete there, open the PR and run the review loop, then STOP and wait for the user"
    status: pending
    dependencies: [p03-8-gate]
---

# Phase 3 – init and join

> Part of the claude-sync roadmap: [`00-roadmap.plan.md`](00-roadmap.plan.md). Spec: `SPEC.md` §3, §4, §7, §8, §9, §11. Rules in `.cursor/rules/` are binding, especially `05-phase-discipline.mdc`, `10-shell.mdc` and `40-security.mdc`.

**Phase 3 of 7.** Previous: Phase 2 – Link layer. Next: Phase 4 – Claude Code plugin.

## Depends on
- **Needs** Phase 1 gate passed (init/join push and pull through `lib/sync.sh`). Check: `PROGRESS.md` row 1.
- **Needs** Phase 2 gate passed (init/join finish by calling `link`). Check: `PROGRESS.md` row 2.

## Before you start
- Every **Needs** line above must be met. If one is not, stop, quote it and say how you checked.
- Read the cited spec sections in full.
- Build on `lib/sync.sh`, `lib/link.sh` and `tests/lib.sh`. If the code differs from this plan, follow the code and note the difference in `PROGRESS.md`.

## Scope (do these, nothing else)
- `templates/user-repo/` (§9 step 3).
- `lib/init.sh`: backup, import, `init`, `join` exactly as §9. Every choice has a flag; prompts only when stdin is a TTY.
- Public-repo guard (§7).
- `pull` calls `link` after success.
- Add `init` and `join` to the dispatcher and `--help`.
- Both commands print the summary from §9 (imported, backed up with path, linked, pushed).

## Out of scope (do NOT start these, even partially)
- Phase 4 – plugin, hooks, `/claude-sync:setup`, `install-cli` → `phase-04-plugin.plan.md`
- Phase 5 – `doctor`, `uninstall`, restoring backups → `phase-05-doctor-release.plan.md`
- Phase 6 – Cursor items in import → `phase-06-cursor-sync.plan.md`
- Phase 7 – settings in import → `phase-07-shared-settings.plan.md`
- Everything in later phases (see `00-roadmap.plan.md`)

Do not create files, services, stubs, settings or UI that belong to a later phase. If something here seems to need later-phase work, write the smallest thing that unblocks this phase, then add a note under "Deferred" in `PROGRESS.md`.

## Gate
**`bash tests/run.sh` passes on ubuntu-latest, macos-latest and windows-latest in CI and locally, `shellcheck` is clean, and the new tests cover at least:**
1. **two-machine round trip**: machine A (temp HOME with a CLAUDE.md, two skills, an agent) runs `init --remote <bare>`; machine B (temp HOME with its own different skill and CLAUDE.md) runs `join <bare>` and gets a conflict list with nothing changed (exit 4); then `join --merge` keeps B's extra skill, appends B's CLAUDE.md under `## From <host>`, backs up the rest; B edits a skill and pushes; A pulls and sees the edit **and** B's extra skill is linked on A;
2. `join --replace` backs up every local item and ends with the repo's content;
3. init imports exactly the allowlisted items; `skills/synced/`, `settings.json`, `.credentials.json`, `projects/` are never moved or copied;
4. every moved item exists under the backup dir with identical content;
5. init twice refuses with exit 2 and changes nothing;
6. `init --create` calls the fake `gh repo create <name> --private` and never `--public`; a fake `gh` reporting a public remote makes `join` print the warning;
7. a skill holding a secret at init time blocks the init commit (exit 5) and leaves the user's files where they were (restored from the move);
8. pull after another machine deleted a skill removes its dangling link.

Prove it: paste the commands and results (counts, exit codes, OS, CI run URL) into `PROGRESS.md`.

## When the gate passes
1. Mark every todo in this file `completed`.
2. In `PROGRESS.md`: mark Phase 3 done with the date and evidence, and update Active phases and Next from each remaining plan's Depends on (Phases 4, 6 and 7 all become startable).
3. Commit with a conventional message scoped `phase-03`, push branch `phase-03-init-join`, open the PR, and run the review loop in `.cursor/skills/phase/SKILL.md`.
4. **Stop** after the merge. Report the PR URL. Do not start the next phase.
