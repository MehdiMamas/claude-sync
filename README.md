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

## Sharing settings

`~/.claude/settings.json` and `~/.claude/settings.local.json` never sync. They hold machine paths, local permissions and, often, tokens. To share some settings anyway, turn on the opt-in: put `settings=1` in the config repo's `claude-sync.conf` (same `key=value` rules as `cursor=1`), and put the keys to share in `settings.shared.json` at the root of the config repo. With `settings=1`, `push` commits `settings.shared.json`. Without it, that file is not committed and not merged.

After each successful `pull`, claude-sync merges `settings.shared.json` into that machine's `settings.json`:

- Objects merge key by key, at every level.
- Arrays get the shared items they do not have yet, after the local items. Nothing is removed and no duplicate is added.
- When both sides have a plain value (string, number, true/false, null) for the same key, the shared value wins.
- Keys that only `settings.json` has are always kept. The merge never removes a key.
- If `settings.shared.json` has a value of another type where `settings.json` has an object or array, nothing is merged and the message names the key.
- If either file is not one valid JSON object, nothing is merged. `settings.json` is left byte for byte as it was.
- When the merge changes something, the old file is copied to `~/.claude/.claude-sync-backup/<UTC time>/settings.json` first. A merge that changes nothing writes nothing. A machine with no `settings.json` gets one with the shared keys.

Keys that make sense to share are the ones that are the same on every machine: `permissions.allow` and `permissions.deny` lists, `env` values that are not secrets, and `model`. Keep machine-specific paths, `apiKeyHelper`, and hooks with local paths in `settings.json`. Values in `settings.shared.json` are committed to your config repo. The secret scan checks it before every push and blocks the push if it finds a token, but do not put secrets there.

The merge needs [jq](https://jqlang.org/). macOS 15 and later ship it as `/usr/bin/jq`. On Linux install the `jq` package. On Windows run `winget install jqlang.jq`, or set `CLAUDE_SYNC_JQ` to the path of a `jq` binary. Without jq, `pull` still pulls and links, prints one line saying jq is needed, exits 2, and does not touch `settings.json`. A session-start hook still exits 0. `settings=1` is the only feature that needs a tool other than bash and git.

## How this repo is built
Numbered phases, one PR each:
- `SPEC.md`: the design, in numbered sections cited as §N.
- `.cursor/plans/`: the roadmap and one plan per phase.
- `PROGRESS.md`: phase status, gate evidence, handoff, deferred items and decisions.
- `.cursor/rules/`: binding rules for every change.
- `.cursor/skills/phase/`: the `/phase` runbook and the local review script.
