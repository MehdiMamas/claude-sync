#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here
git -C "$CLAUDE_SYNC_DIR" remote set-url origin "$(file_url "$TEST_WORK/missing.git")"

run_sync pull --hook
assert_code 0 "hook pull while unreachable"
assert_err_lines 1
assert_err_contains "pull failed"
log=$(cat "$XDG_STATE_HOME/claude-sync/log")
case "$log" in
  *fatal*) ;;
  *) fail "hook log is missing git's error: [$log]" ;;
esac

run_sync pull
assert_code 3 "cli pull while unreachable"
assert_no_rebase "$CLAUDE_SYNC_DIR"
