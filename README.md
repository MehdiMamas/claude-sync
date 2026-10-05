# claude-sync

Keep your Claude Code setup (`CLAUDE.md`, skills, subagents, slash commands, output styles) in a private git repo you own, and have it sync across your Macs, Linux boxes and Windows PCs automatically: pull when a session starts, push when it ends.

**Status: the sync engine (`pull` / `push`) is implemented.** Linking, setup, and the plugin come in later phases. The design is in [`SPEC.md`](SPEC.md) and the build order in [`PROGRESS.md`](PROGRESS.md).

## Planned usage
```
/plugin marketplace add <owner>/claude-sync
/plugin install claude-sync@claude-sync
/claude-sync:setup
```
Run the same three steps on every other machine and point it at the same repo.

## Cursor sync

Cursor sync is off until the config repo has a `claude-sync.conf` file whose `cursor` value is `1`. The file is lines of `key=value`. Blank lines are ignored. `#` starts a comment, on its own line or after a value (`cursor=1 # on`). Spaces around `=` are ignored, and a repeated key uses the last value. Anything other than `cursor=1`, including a missing file or `cursor=0`, does not link `~/.cursor`. `unlink` still removes Cursor links that point into the repo.

With `cursor=1`, claude-sync links these directories and no others:

- `~/.cursor/skills/<name>/` ↔ `cursor/skills/<name>/`
- `~/.cursor/rules/` ↔ `cursor/rules/`
- `~/.cursor/agents/` ↔ `cursor/agents/`
- `~/.cursor/commands/` ↔ `cursor/commands/`

Push the conf file, then `claude-sync link` on a machine that does not already have those paths. If a path is already a real directory, `link` leaves it and reports a conflict. `claude-sync join --merge <url>` backs it up, keeps the repo copy when both sides have it, and imports local items the repo does not have yet. A secret inside `cursor/` blocks the push the same way one in `skills/` does.

## How this repo is built
Numbered phases, one PR each:
- `SPEC.md`: the design, in numbered sections cited as §N.
- `.cursor/plans/`: the roadmap and one plan per phase.
- `PROGRESS.md`: phase status, gate evidence, handoff, deferred items and decisions.
- `.cursor/rules/`: binding rules for every change.
- `.cursor/skills/phase/`: the `/phase` runbook and the local review script.
