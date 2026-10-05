# Claude config

Private git repo for one person's Claude Code setup, written by claude-sync.

Synced: `CLAUDE.md`, `skills/`, `agents/`, `commands/`, `output-styles/`.
Left on the machine: credentials, history, projects, settings, and `skills/synced/`.

## Another machine

Install claude-sync, then:

    claude-sync join <url-of-this-repo>

If that machine already has local Claude config, join lists it and changes nothing.
Re-run with `--merge` or `--replace` after you choose which copy to keep.

## Cursor (optional)

Cursor skills, rules, agents, and commands stay on the machine until this repo
contains `claude-sync.conf` with `cursor=1` (`#` starts a comment). Then they sync under `cursor/`.
