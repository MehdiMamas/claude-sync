# claude-sync specification

> **Historical.** This is the design of the v0 bash CLI in `bin/`, `lib/` and `tests/`. The desktop app replaces it; its design is [docs/architecture.md](docs/architecture.md). The plugin described below was never built.

Working name: **claude-sync** (see §17 for the naming question). Sections are cited as §N.

# 1. Overview and goals

Claude Code keeps a person's own setup in `~/.claude/`: global instructions (`CLAUDE.md`), skills, subagents, slash commands and output styles. None of it syncs between machines. claude-sync keeps that setup in a **git repo the user owns** (normally a private GitHub repo) and keeps every machine in step with it automatically.

Goals:
- **Anyone can set it up in a few minutes**: install the plugin, run one setup command, answer one question (which repo).
- **Automatic**: pull when a Claude Code session starts, commit and push when it ends. No daily commands.
- **Cross-platform**: macOS, Linux and Windows (native Windows with Git Bash, which Claude Code already requires).
- **Safe by default**: an allowlist of what syncs, a secret scan before every push, backups before anything local is replaced, never a force-push, and a session is never blocked by a sync failure.
- **No server, no account beyond git.** The user's repo is the only backend.

# 2. Non-goals (v1)

- Syncing credentials, OAuth tokens, history, transcripts, caches, `projects/` or per-project memory folders. Per-project memory is keyed by absolute path, so it can't line up across machines; project state belongs in the project's own repo.
- Real-time sync during a session.
- Merging conflicting edits automatically beyond what `git pull --rebase` does.
- Team or shared configs (one user, many machines). Teams already have project-level `.claude/` in their repos.
- A hosted service, GUI, or MCP server. Git plus hooks covers it, and a local clone works offline.

# 3. User flows

**First machine (`init`).** Install the plugin, run `/claude-sync:setup`. Claude asks whether to create a new private repo (needs `gh`) or use an existing remote URL, then runs `claude-sync init`. Existing `~/.claude` content is imported into the repo, backed up, linked back, and pushed.

**Another machine (`join`).** Install the plugin, run `/claude-sync:setup`, give the same repo. `claude-sync join <url>` clones it, deals with any local content already present (§9), links, done.

**Every day.** Nothing. SessionStart pulls (and links anything new), SessionEnd commits and pushes. `/claude-sync:status` shows state; `/claude-sync:sync` forces a round trip; `claude-sync doctor` explains problems.

# 4. What syncs

Allowlist, relative to `~/.claude/`. Anything not listed never enters the repo.

| Item | Local path | Repo path | How it's connected (§8) |
|---|---|---|---|
| Global instructions | `~/.claude/CLAUDE.md` | `CLAUDE.md` | import line inside a marked block |
| Skills | `~/.claude/skills/<name>/` | `skills/<name>/` | one directory link per skill |
| Subagents | `~/.claude/agents/` | `agents/` | directory link |
| Slash commands | `~/.claude/commands/` | `commands/` | directory link |
| Output styles | `~/.claude/output-styles/` | `output-styles/` | directory link |

Never synced, even if a user adds them to the repo by hand: `.credentials.json`, `*.credentials*`, `.env*`, `settings.json`, `settings.local.json`, `history*`, `projects/`, `todos/`, `statsig/`, `shell-snapshots/`, `plugins/`, `ide/`, and `skills/synced/` (managed by Anthropic's own service and overwritten by it). Opt-in extras come later: Cursor config (§15) and shared settings (§16).

# 5. Repository layouts

**This tool's repo** (public):
```
.claude-plugin/plugin.json        # plugin manifest (Phase 4)
.claude-plugin/marketplace.json   # this repo is its own marketplace (Phase 4)
bin/claude-sync                   # bash entry point, dispatches subcommands
lib/*.sh                          # one file per area: sync, secrets, link, init, doctor
hooks/hooks.json                  # SessionStart / SessionEnd (Phase 4)
commands/*.md                     # plugin slash commands (Phase 4)
templates/user-repo/              # .gitignore, .gitattributes, README for the user's repo (Phase 3)
tests/run.sh, tests/*.test.sh     # plain-bash test runner and tests
```

**The user's config repo** (private, created by `init`), cloned to `~/.claude-sync/` by default (override with `CLAUDE_SYNC_DIR`):
```
CLAUDE.md  skills/  agents/  commands/  output-styles/
.gitignore .gitattributes README.md
```

State that must not sync (lock, last-run times, log) lives in `${XDG_STATE_HOME:-$HOME/.local/state}/claude-sync/` on every OS.

# 6. Sync engine

`claude-sync pull` and `claude-sync push`, used by the hooks and by hand.

**pull**
1. Take the lock (§6 Lock). If the repo dir is missing, exit 0 silently in hook mode (the user hasn't set up yet).
2. `git -C "$DIR" pull --rebase --autostash --quiet`.
3. On failure (offline, auth, conflict): if a rebase is in progress, `git rebase --abort` so the working tree is left as it was. Print one warning line.
4. After a successful pull, run `link` so new skills from other machines appear and removed ones are cleaned up (wired in Phase 3).

**push**
1. Take the lock. If `git status --porcelain` shows nothing under the allowlist, exit 0.
2. Stage only allowlisted paths: `git add -A -- CLAUDE.md skills agents commands output-styles` (plus opt-ins from later phases). Untracked files elsewhere in the repo are reported once and left alone.
3. Run the secret scan (§7) on the staged diff. If it hits, unstage everything, print which file and pattern (never the value), and stop. Nothing is committed.
4. Commit `sync: <hostname> <UTC ISO timestamp>`, then `git push`.
5. On a non-fast-forward: `pull --rebase` once and push again. On a conflict, abort the rebase, keep the local commit, print a warning. **Never force-push.**

**Hook mode** (`--hook`): always exit 0, write at most one line to stderr, and append details to the log. Git must never prompt: `GIT_TERMINAL_PROMPT=0`, `GIT_SSH_COMMAND` with `-o BatchMode=yes` unless the user set their own. Each git network call has a time limit (default 20 s, `CLAUDE_SYNC_TIMEOUT`) so a dead network can't hang session start or end.

**CLI mode** (no `--hook`): same steps, but failures exit non-zero with a clear message.

**Lock.** `mkdir "$STATE/lock"` with a pid file. Two sessions ending at once must not both commit. A lock older than 10 minutes, or whose pid is gone, is stale and is taken over. In hook mode a held lock means "skip quietly"; in CLI mode wait up to 30 s, then fail.

# 7. Secret safety

- The allowlist (§4) is the first line. The user repo's `.gitignore` (§9) repeats the never-sync list as a second line.
- Before every commit, scan the staged diff's added lines for: `sk-ant-[A-Za-z0-9_-]{20,}`, `ghp_[A-Za-z0-9]{36}`, `github_pat_[A-Za-z0-9_]{50,}`, `gh[orsu]_[A-Za-z0-9]{36}`, `AKIA[0-9A-Z]{16}`, `xox[abpr]-[A-Za-z0-9-]{10,}`, `-----BEGIN [A-Z ]*PRIVATE KEY-----`, and `"(access|refresh)_token"\s*:`.
- A hit blocks the commit (§6 push step 3). The message names the file, line number and pattern name. It never prints the matched text.
- An escape hatch for false positives: a line containing `claude-sync: allow-secret` is skipped. Documented, not advertised.
- `init` refuses to create a **public** repo, and `join`/`init --remote` warn if `gh` reports the remote is public.

# 8. Link layer

`claude-sync link` connects `~/.claude/` to the repo. `claude-sync unlink` disconnects it. Both are idempotent.

**CLAUDE.md.** Never replace the user's file. Insert (or update) a marked block at the top:
```
<!-- claude-sync:begin (managed, do not edit inside) -->
@~/.claude-sync/CLAUDE.md
<!-- claude-sync:end -->
```
The path written is the actual repo dir in `~/` form. Content outside the block stays local to this machine. `unlink` removes only the block. Create the file if it is missing.

**Directories.** `skills/<name>` gets one link per skill (because `~/.claude/skills/` also holds `synced/` and other local skills). `agents/`, `commands/`, `output-styles/` are linked as whole directories when the repo has them.
- macOS/Linux: `ln -s`.
- Windows (Git Bash): a directory junction, `cmd //c mklink /J "<win path of link>" "<win path of target>"` via `cygpath -w`. Junctions need no admin or Developer Mode. File symlinks are never used.

**Rules.**
- If the local path is already a link to the right target: nothing to do.
- If it is a link pointing elsewhere, or a real file or directory: do **not** touch it. Report it as a conflict. (Importing local content is `init`/`join`'s job, §9.)
- After linking, remove dangling links under `~/.claude/` whose target is inside the repo dir (a skill deleted on another machine). Never remove any other link.
- **Removing a link must never delete the target's contents.** On Windows remove a junction with `cmd //c rmdir "<win path>"`, never `rm -rf`. On Unix use `rm` on the link path with no trailing slash. Tests must prove the target survives.

`claude-sync status` prints, per allowlisted item: linked / missing / conflict, plus repo state (branch, ahead/behind, dirty, last pull/push time).

# 9. init and join

**`claude-sync init [--create <name> | --remote <url>] [--dir <path>]`** (first machine)
1. Refuse if the repo dir already exists and is a git repo with a remote (suggest `join`).
2. `--create <name>`: `gh repo create <name> --private` (needs `gh` logged in). `--remote <url>`: use an existing empty repo. Neither: create a local repo only and print how to add a remote.
3. Write `templates/user-repo/` files: `.gitignore` (never-sync list, §4), `.gitattributes` (`* text=auto eol=lf`), `README.md` (what this repo is, how to join another machine).
4. Back up everything it will move to `~/.claude/.claude-sync-backup/<UTC timestamp>/`, preserving paths.
5. Import: the body of `~/.claude/CLAUDE.md` (outside any managed block) becomes the repo `CLAUDE.md`; each skill directory except `synced/` and existing links; `agents/`, `commands/`, `output-styles/`. Import means move into the repo, then link back (§8), so the user sees the same files at the same paths.
6. Commit `init: import from <hostname>` and push if there is a remote. If the secret scan blocks the commit, undo steps 4–5 (unlink, move items back to where they were) so the machine is as it was, and exit 5.

**`claude-sync join <url> [--merge | --replace] [--dir <path>]`** (other machines)
1. Clone to the repo dir.
2. For each allowlisted item that exists locally as real content:
   - default: stop before changing anything and list the conflicts, suggesting `--merge` or `--replace`;
   - `--merge`: import local items that the repo lacks (same as init step 5), back up and replace those it has (repo wins), and append the local CLAUDE.md body to the repo's under a `## From <hostname>` heading for the user to tidy;
   - `--replace`: back up all local items, then use the repo's.
3. Link, commit any merged content, push.

Both commands print a short summary: what was imported, backed up (with the path), linked, and pushed. Interactive prompts appear only when stdin is a TTY; every choice also has a flag, because `/claude-sync:setup` runs them through Claude.

# 10. Plugin packaging and hooks

claude-sync ships as a Claude Code **plugin**, and this repo is also its marketplace:
```
/plugin marketplace add <owner>/claude-sync
/plugin install claude-sync@claude-sync
/claude-sync:setup
```
- `hooks/hooks.json`: `SessionStart` → `bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" pull --hook`; `SessionEnd` → `... push --hook`. Plugin hooks need no edits to `~/.claude/settings.json`, so no JSON tooling is needed to install.
- `commands/setup.md`: instructs Claude to check prerequisites (`git`, `gh` optional), ask new-or-existing repo, run `init` or `join` with explicit flags, show the summary, and stop. It must show the user any conflict list and ask before `--merge`/`--replace`.
- `commands/status.md`, `commands/sync.md`: run `status`, or `pull` then `push`, and show the output.
- Terminal use: `claude-sync install-cli` writes a small shim to `~/.local/bin/claude-sync` that execs the installed plugin's `bin/claude-sync`.
- Before building, the Phase 4 preflight confirms the current plugin format (manifest fields, hooks file location, `${CLAUDE_PLUGIN_ROOT}`, how hooks run on Windows) against the Claude Code docs, and records any differences in Notes / decisions.

# 11. CLI reference

```
claude-sync pull   [--hook]
claude-sync push   [--hook]
claude-sync link | unlink | status
claude-sync init   [--create <name> | --remote <url>] [--dir <path>]
claude-sync join   <url> [--merge | --replace] [--dir <path>]
claude-sync doctor
claude-sync uninstall [--restore-backup <timestamp>]
claude-sync install-cli
claude-sync --version | --help
```
Env: `CLAUDE_SYNC_DIR` (repo dir, default `~/.claude-sync`), `CLAUDE_SYNC_TIMEOUT` (seconds, default 20), `CLAUDE_HOME` (default `~/.claude`, mostly for tests). Exit codes: 0 ok, 1 usage, 2 precondition not met, 3 git/network failure, 4 conflict, 5 secret found.

# 12. doctor and uninstall

**doctor** checks and explains, one line each with a fix: bash version (3.2 or later, §13); `git` present and ≥ 2.30; `gh` present (optional); repo dir exists, has a remote, is on a branch, not mid-rebase; remote reachable; remote visibility private; each item linked (§8); on Windows, junction creation works in `~/.claude`; hooks fired recently (last-run times in the state dir); lock not stale; untracked non-allowlisted files in the repo.

**uninstall**: `unlink`, optionally copy a backup back, and leave the repo dir and the remote untouched. It prints the `/plugin uninstall` command; it does not remove the plugin itself.

# 13. Platforms and dependencies

- Runtime: bash (3.2-compatible; Git Bash on Windows), git ≥ 2.30, coreutils that exist on macOS, Linux and Git for Windows. `gh` only for `init --create` and the visibility check.
- **No jq, Python or Node.** Nothing in v1 needs to edit JSON. One exception: the opt-in shared settings merge (§16, `settings=1`) needs jq. Without it, pull and link still work and only the merge is skipped.
- Scripts use `#!/usr/bin/env bash` and `set -euo pipefail`, quote every expansion, and use `$HOME`, never `~`, inside scripts. Every `.sh` and `bin/*` file is LF (`.gitattributes`).
- Windows paths: convert with `cygpath` only at the `cmd` boundary.

# 14. Testing

- `tests/run.sh` runs every `tests/*.test.sh` and prints `N passed, M failed`, exiting non-zero on any failure. A small `tests/lib.sh` holds assertions. No bats, so it runs anywhere bash does.
- Each test makes a temp `HOME`, a temp bare repo as the remote (`file://`), and sets `CLAUDE_SYNC_DIR`/`CLAUDE_HOME` into it. Two-machine tests use two temp HOMEs against one bare remote. Tests never touch the real `~/.claude` or the network.
- The gate is local: `bash tests/run.sh` and `shellcheck -x bin/claude-sync lib/*.sh tests/*.sh` on the machine doing the work. Name that OS in the PR. There is no GitHub Actions workflow.

# 15. Cursor config sync (opt-in, Phase 6)

Optionally sync Cursor's file-based global config (for example `~/.cursor/skills/`, `~/.cursor/rules/` if Cursor reads them) under `cursor/` in the user repo, with the same link layer. Enabled per repo by a `claude-sync.conf` file with `cursor=1`. The Phase 6 preflight must confirm which Cursor global paths are real files Cursor reads, on macOS and Windows, before building anything.

# 16. Shared settings (opt-in, Phase 7)

`settings.json` is machine-specific (paths, local permissions), so it never syncs whole. Optionally, a `settings.shared.json` in the user repo holds selected keys (for example `permissions.allow`, `env`, `model`) and is merged into `~/.claude/settings.json` after each pull, never removing local keys. This is the first feature that edits JSON; it uses jq (`CLAUDE_SYNC_JQ` overrides the path).

# 17. Open questions

- **Name.** `claude-sync` is a working name. Check for collisions on GitHub and the plugin marketplaces before a plugin manifest is published; renaming after release is costly.
- **License.** MIT (see `LICENSE`).
- **User-level memory.** Not synced in v1 (§2). Revisit after release if users ask.
