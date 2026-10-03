#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here
add_second_home

enter_home "$TEST_HOME"
printf 'one\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 0

enter_home "$TEST_HOME_B"
lock="$XDG_STATE_HOME/claude-sync/lock"
mkdir -p "$lock"
run_sync pull --hook
assert_code 0 "a fresh empty lock is not taken over"
assert_err_lines 0
if [ -f "$CLAUDE_SYNC_DIR/CLAUDE.md" ]; then
  fail "hook pull ran while a fresh empty lock was held"
fi
rmdir "$lock"

epoch=$(($(date +%s) - 700))
case "$(uname -s)" in
  Darwin) stamp=$(date -r "$epoch" +%Y%m%d%H%M.%S) ;;
  *) stamp=$(date -d "@$epoch" +%Y%m%d%H%M.%S) ;;
esac
mkdir -p "$lock"
touch -t "$stamp" "$lock"
run_sync pull
assert_code 0 "an empty lock older than 10 minutes is taken over"
assert_eq "$(<"$CLAUDE_SYNC_DIR/CLAUDE.md")" "one"
for stale in "$lock".stale.*; do
  if [ -e "$stale" ]; then
    fail "stale lock directory was left behind"
  fi
done
