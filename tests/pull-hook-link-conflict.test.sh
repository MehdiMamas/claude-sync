#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here
run_sync link
assert_code 0 "link writes the managed block before the conflict"

mkdir -p "$CLAUDE_SYNC_DIR/agents"
printf 'remote-agent\n' > "$CLAUDE_SYNC_DIR/agents/a.md"
git -C "$CLAUDE_SYNC_DIR" add agents
git -C "$CLAUDE_SYNC_DIR" commit -m agents >/dev/null
git -C "$CLAUDE_SYNC_DIR" push >/dev/null 2>&1

mkdir -p "$CLAUDE_HOME/agents"
printf 'local-agent\n' > "$CLAUDE_HOME/agents/keep.md"
cp "$CLAUDE_HOME/agents/keep.md" "$TEST_WORK/agent-before"

run_sync pull --hook
assert_code 0 "hook pull exits 0 when link conflicts"
assert_err_lines 1
assert_err_contains "conflict"
assert_file_eq "$CLAUDE_HOME/agents/keep.md" "$TEST_WORK/agent-before"
