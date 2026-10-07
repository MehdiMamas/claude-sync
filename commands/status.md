---
description: Show the claude-sync state of this machine
---

Run `bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" status` and show its full output.

Then explain it in a few lines:
- each item (`CLAUDE.md`, `skills/<name>`, `agents`, `commands`, `output-styles`) is `linked`, `missing` or `conflict`;
- `ahead` and `behind` count commits not yet pushed or pulled, and `dirty` means local edits not yet committed (the next session end pushes them);
- `last-pull` and `last-push` are the last successful sync times.

If it fails because there is no repo, say that claude-sync is not set up on this machine and that `/config-sync:setup` sets it up. Do not change any file yourself.
