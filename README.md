# claude-sync

Keep your Claude Code setup (`CLAUDE.md`, skills, subagents, slash commands, output styles) in a private git repo you own, and have it sync across your Macs, Linux boxes and Windows PCs automatically: pull when a session starts, push when it ends.

**Status: planning.** Nothing is built yet. The design is in [`SPEC.md`](SPEC.md) and the build order in [`PROGRESS.md`](PROGRESS.md).

## Planned usage
```
/plugin marketplace add <owner>/claude-sync
/plugin install claude-sync@claude-sync
/claude-sync:setup
```
Run the same three steps on every other machine and point it at the same repo.

## How this repo is built
Numbered phases, one PR each:
- `SPEC.md`: the design, in numbered sections cited as §N.
- `.cursor/plans/`: the roadmap and one plan per phase.
- `PROGRESS.md`: phase status, gate evidence, handoff, deferred items and decisions.
- `.cursor/rules/`: binding rules for every change.
- `.cursor/skills/phase/`: the `/phase` runbook and the local review script.
