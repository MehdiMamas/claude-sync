#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

state_file="$TEST_WORK/not-a-directory"
printf 'nope\n' > "$state_file"
XDG_STATE_HOME="$state_file"
export XDG_STATE_HOME
run_sync pull --hook
assert_code 0 "unwritable state dir still exits 0"
assert_err_at_most 1

enter_home "$TEST_HOME"
rm -rf "$CLAUDE_SYNC_DIR/.git"
printf 'gitdir: %s\n' "$TEST_WORK/missing-gitdir" > "$CLAUDE_SYNC_DIR/.git"
run_sync pull --hook
assert_code 0 "broken .git still exits 0"
assert_err_lines 1
assert_err_contains "pull failed"
case "$RUN_ERR" in
  *fatal*) fail "git's fatal line reached the terminal: [$RUN_ERR]" ;;
esac

enter_home "$TEST_HOME"
rm -rf "$CLAUDE_SYNC_DIR"
clone_here
rm -f "$XDG_STATE_HOME/claude-sync/log"
git -C "$CLAUDE_SYNC_DIR" remote set-url origin "https://user:supersecret@127.0.0.1:1/nope.git"
run_sync pull --hook
assert_code 0 "credentialed remote fails closed in hook mode"
assert_err_at_most 1
log=$(cat "$XDG_STATE_HOME/claude-sync/log")
case "$log" in
  *supersecret*) fail "log kept credentials from the remote URL" ;;
esac
case "$log" in
  *fatal*|*'Could not'*|*'unable to'*|*'Failed to'*) ;;
  *) fail "log is missing git's error: [$log]" ;;
esac
