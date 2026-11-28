#!/usr/bin/env bash
# Prints env exports for an isolated dev device. Use: eval "$(scripts/dev-home.sh [dir])"
# Nothing touches the real ~/.claude, ~/.claude-sync or keychain. Debug builds only (dev token).
set -euo pipefail
dir="${1:-$(mktemp -d "${TMPDIR:-/tmp}/claude-sync-dev.XXXXXX")}"
mkdir -p "$dir/claude" "$dir/sync"
token="$(gh auth token)"
printf 'export CLAUDE_HOME=%q\n' "$dir/claude"
printf 'export CLAUDE_SYNC_HOME=%q\n' "$dir/sync"
printf 'export CLAUDE_SYNC_DEV_TOKEN=%q\n' "$token"
# a test home must not join the person's real personal repo by itself
printf 'export CLAUDE_SYNC_NO_AUTO_PERSONAL=1\n'
