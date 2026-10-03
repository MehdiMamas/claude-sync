# Claude setup sync plan (PC ↔ Mac)

Status: **plan only, not started** (written 2026-10-03). Give this file to Claude when you want to build it.

## Problem
My Claude Code setup lives in `~/.claude/` on each machine and does not sync:
- `~/.claude/CLAUDE.md`: global instructions (my workflow, the ask-first rule, numbered phases).
- `~/.claude/skills/phasing/`: the phasing skill with every template (rules, `/phase` runbook, review prompt, `review.sh`, `PROGRESS.md`, plans, PR template).
- `~/.claude/projects/<path>/memory/`: per-project memory. The folder name is derived from the absolute path (`C--Users-Mehdi-Projects-cursify` on Windows vs `-Users-mehdi-Projects-cursify` on the Mac), so it doesn't line up across machines.

Edits made on the Windows PC never reach the Mac (or the second PC), and the other way round.

## What already exists and why it doesn't solve it
- `~/.claude/skills/synced/` is Anthropic's server-managed sync of built-in claude.ai skills (docx, pdf, docs…). It's overwritten by the service, so my own files can't go there.
- Project repos (cursify etc.) already sync through GitHub, but global config is not in any repo.

## Decision so far
- **Use GitHub** as the sync backend: a **private** repo, working name `MehdiMamas/claude-config`.
- **Sync mode: pull on session start, auto commit + push on session end** (chosen 2026-10-03).
- **No phasing setup** for this repo. It's just config.
- **No MCP needed.** Git plus Claude Code hooks covers it. MCP would only matter if the config had to be fetched live from a server; a local clone is simpler, works offline, and is already where Claude Code reads files.

## Design

### Repo layout
```
claude-config/
  CLAUDE.md                 # global instructions (moved out of ~/.claude/CLAUDE.md)
  skills/
    phasing/                # SKILL.md + templates/ (everything, self-contained)
    <future skills>/
  memory/                   # optional: cross-machine "user" memories (see Memory below)
  hooks/
    sync.sh                 # pull --rebase --autostash; commit + push if dirty
  install.sh                # macOS / Linux
  install.ps1               # Windows
  README.md
```

### How each machine uses it
1. Clone to `~/claude-config` (same relative location on every machine).
2. `~/.claude/CLAUDE.md` becomes a one-line import: `@~/claude-config/CLAUDE.md`. Claude Code supports `@path` imports with `~`. This avoids file symlinks, which need admin or Developer Mode on Windows.
3. Skills: link each `~/.claude/skills/<name>` to `~/claude-config/skills/<name>`.
   - macOS: `ln -s`.
   - Windows: directory junction, `cmd /c mklink /J`, which needs no admin.
   - Fallback if links are a problem: the sync script copies `skills/` into `~/.claude/skills/` after each pull and back before each push.
4. Hooks in `~/.claude/settings.json` (user-level, so they apply to every project):
   - `SessionStart` → `bash ~/claude-config/hooks/sync.sh pull`
   - `SessionEnd` → `bash ~/claude-config/hooks/sync.sh push`
   - On Windows, hooks run through Git Bash, which is already installed.

### `sync.sh` behavior
- `pull`: `git -C ~/claude-config pull --rebase --autostash --quiet`. On failure (offline, conflict) print one warning line and exit 0 so a session is never blocked.
- `push`: if `git status --porcelain` is non-empty, run `git add -A`, commit `sync: <hostname> <timestamp>`, then `git push`. On a non-fast-forward, pull with rebase and retry once. On a conflict, leave it and print a warning; never force-push.
- Never commit secrets: `.gitignore` excludes `*.credentials*`, `.env*`, and anything under `~/.claude` that isn't explicitly ours.

### Install scripts
- `install.sh` / `install.ps1`:
  1. Clone the repo if it's missing.
  2. Back up the existing `~/.claude/CLAUDE.md` and replace it with the import line.
  3. Create the skill links.
  4. Merge the two hooks into `~/.claude/settings.json` without clobbering other keys. Use `jq` on the Mac and `ConvertFrom-Json` on Windows.
  5. Run one `sync.sh pull`.
- Idempotent: safe to re-run.

### Memory
- Per-project memory folders are keyed by absolute path, so they won't match across machines. Options, to decide later:
  - (a) Keep project memory local, and put anything that should follow me everywhere into `CLAUDE.md` (current approach).
  - (b) Store a `memory/` folder in the repo for user-level facts and import it from `CLAUDE.md`.
  - (c) For project-specific state, rely on the project repo itself (`PROGRESS.md` Handoff), which already syncs through GitHub. This is what the phasing workflow does.
- Leaning: (a) + (c). Revisit if that turns out to be insufficient.

## Migration steps (when we build it)
1. Create the private repo `MehdiMamas/claude-config` on the PC, and copy in the current `~/.claude/CLAUDE.md` and `~/.claude/skills/phasing/`.
2. Write `sync.sh`, `install.sh`, `install.ps1`, `.gitignore` and `README.md`.
3. Run `install.ps1` on this PC. Verify: a new Claude Code session shows the workflow rules loaded (`/memory`), and the `phasing` skill is listed.
4. Push. On the Mac, run `git clone … ~/claude-config && bash ~/claude-config/install.sh`. On the second PC, run `install.ps1`.
5. Test the round trip: edit `CLAUDE.md` on the Mac, end the session, start a session on the PC, and check the edit is there.

## Open questions
- Repo name: `claude-config` OK?
- Should Cursor's global rules and skills (`~/.cursor/`) sync the same way? Probably yes, in the same repo under `cursor/`.
- Should hooks also sync `~/.claude/settings.json` itself (permissions, theme)? It's risky because it's machine-specific; maybe sync a `settings.shared.json` and merge it.
