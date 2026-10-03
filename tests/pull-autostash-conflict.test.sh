#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here
add_second_home

enter_home "$TEST_HOME"
printf 'base\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 0

enter_home "$TEST_HOME_B"
run_sync pull
assert_code 0

enter_home "$TEST_HOME"
printf 'from-a\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 0

enter_home "$TEST_HOME_B"
printf 'from-b\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)

run_sync pull
assert_code 4 "pull undoes an autostash conflict"
assert_err_contains "local edits"
assert_no_rebase "$CLAUDE_SYNC_DIR"
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)" "$before" "autostash conflict kept the fast-forward"
assert_eq "$(<"$CLAUDE_SYNC_DIR/CLAUDE.md")" "from-b"
if [ -n "$(git -C "$CLAUDE_SYNC_DIR" ls-files -u)" ]; then
  fail "autostash conflict left unmerged entries"
fi
case "$(<"$CLAUDE_SYNC_DIR/CLAUDE.md")" in
  *'<<<<<<<'*) fail "conflict markers were left in CLAUDE.md" ;;
esac
