#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
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
sleep 120 &
holder=$!
track_pid "$holder"
printf '%s\n' "$holder" > "$lock/pid"
printf '%s\n' "$(date +%s)" > "$lock/time"

run_sync pull --hook
assert_code 0 "fresh lock skips in hook mode"
assert_err_lines 0
if [ -f "$CLAUDE_SYNC_DIR/CLAUDE.md" ]; then
  fail "hook pull ran while a fresh lock was held"
fi

kill "$holder" 2>/dev/null || true
wait "$holder" 2>/dev/null || true
run_sync pull
assert_code 0 "dead pid lock is taken over"
assert_eq "$(<"$CLAUDE_SYNC_DIR/CLAUDE.md")" "one"

enter_home "$TEST_HOME"
printf 'two\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 0

enter_home "$TEST_HOME_B"
mkdir -p "$lock"
sleep 120 &
holder=$!
track_pid "$holder"
printf '%s\n' "$holder" > "$lock/pid"
printf '%s\n' "$(($(date +%s) - 601))" > "$lock/time"
run_sync pull
assert_code 0 "lock older than 10 minutes is taken over"
assert_eq "$(<"$CLAUDE_SYNC_DIR/CLAUDE.md")" "two"
kill "$holder" 2>/dev/null || true
wait "$holder" 2>/dev/null || true

enter_home "$TEST_HOME"
printf 'three\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 0

enter_home "$TEST_HOME_B"
mkdir -p "$lock"
sleep 90 &
holder=$!
track_pid "$holder"
printf '%s\n' "$holder" > "$lock/pid"
printf '%s\n' "$(date +%s)" > "$lock/time"
start=$(date +%s)
run_sync pull
end=$(date +%s)
assert_code 2 "cli waits, then fails when the lock is held"
assert_err_contains "another sync is running"
assert_eq "$(<"$CLAUDE_SYNC_DIR/CLAUDE.md")" "two"
elapsed=$((end - start))
if [ "$elapsed" -lt 25 ] || [ "$elapsed" -gt 50 ]; then
  fail "lock wait took ${elapsed}s, expected about 30s"
fi
