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
git -C "$CLAUDE_SYNC_DIR" add CLAUDE.md
git -C "$CLAUDE_SYNC_DIR" commit -m "local edit" >/dev/null
before=$(snapshot_repo "$CLAUDE_SYNC_DIR")

run_sync pull
assert_code 4 "pull rebase conflict"
assert_no_rebase "$CLAUDE_SYNC_DIR"
after=$(snapshot_repo "$CLAUDE_SYNC_DIR")
assert_eq "$after" "$before" "working tree and HEAD unchanged after aborted rebase"
assert_eq "$(<"$CLAUDE_SYNC_DIR/CLAUDE.md")" "from-b"
