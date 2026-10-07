---
description: Set up claude-sync on this machine, with a new private repo or one you already use
argument-hint: "[new <repo-name> | <repo-url>]"
---

Set up claude-sync on this machine. claude-sync keeps the user's Claude Code setup (CLAUDE.md, skills, agents, commands, output styles) in a private git repo they own and syncs it when sessions start and end.

Rules for this command:
- Run claude-sync only as `bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" <arguments>`, exactly as written in the steps below.
- Never create, edit, move or delete files under `~/.claude` or the claude-sync repo yourself. claude-sync does every change and makes the backups.
- Never pass `--merge` or `--replace` unless the user chose it after seeing the conflict list (step 5).
- Never make a repo public.

Steps:

1. Check the tools. Run `git --version`. If git is missing, tell the user to install git and stop. Run `gh --version` and `gh auth status`. gh is needed only to create a new repo.

2. Check whether this machine is already set up. Run `bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" status`. If it exits 0, show its output, say claude-sync is already set up here, and stop.

3. Decide which repo to use. Arguments given: `$ARGUMENTS`
   - `new <repo-name>`: create a new private repo with that name.
   - a git URL (https, ssh, or a `file://` path): use that repo.
   - nothing: ask the user whether to create a new private GitHub repo (it needs gh logged in) or to use a repo URL they already have, and wait for the answer.

4. Run the setup command and show its full output.
   - New repo: `bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" init --create <repo-name>`.
   - Existing URL: first `bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" init --remote <url>`. If that fails with `remote already has commits`, the repo is already in use by another machine, so run `bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" join <url>` instead.

5. If `join` exits 4 with `conflict:` lines, nothing local was changed yet. Show the user every conflict line and explain the two choices:
   - merge: back up the local copies, keep the repo's copy where both exist, import local items the repo does not have, and add this machine's CLAUDE.md text under a `## From <hostname>` heading in the repo's CLAUDE.md;
   - replace: back up every local item, then use only the repo's.
   Ask which one they want, or whether to stop. Only after they answer, run `bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" join <url> --merge` or `... join <url> --replace`. If they want to stop, stop.

6. Any other failure: show the message claude-sync printed. Each message says what to do next. Stop.

7. On success, show the summary lines (`imported`, `backed up`, `linked`, `pushed`) and tell the user:
   - from now on a session start pulls and a session end pushes, with no command needed;
   - `/config-sync:status` shows the state and `/config-sync:sync` syncs now;
   - on another machine, install this plugin and run `/config-sync:setup <the same repo URL>`.
   Then stop.
