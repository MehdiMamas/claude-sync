---
description: Sync now - pull, then push this machine's claude-sync repo
---

1. Run `bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" pull` and show its full output and exit code.
2. Run `bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" push` and show its full output and exit code. Run it even if the pull failed: a push refuses on its own when it is not safe.
3. Say in one line per command whether it worked. When one failed, repeat the message claude-sync printed, which says what to do next.

Do not change any file yourself, and do not run git commands in the repo yourself. If there is no repo, say that `/config-sync:setup` sets claude-sync up.
