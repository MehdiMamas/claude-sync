#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here
add_second_home

enter_home "$TEST_HOME"
printf 'from-a\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 0
remote_head=$(git --git-dir="$TEST_BARE" rev-parse refs/heads/main)

enter_home "$TEST_HOME_B"
printf 'from-b\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 4 "conflicting push"
assert_err_contains "Nothing was force-pushed."
assert_no_rebase "$CLAUDE_SYNC_DIR"
assert_eq "$(git --git-dir="$TEST_BARE" rev-parse refs/heads/main)" "$remote_head" "remote head changed"
assert_eq "$(<"$CLAUDE_SYNC_DIR/CLAUDE.md")" "from-b"
case "$(git -C "$CLAUDE_SYNC_DIR" log -1 --format=%s)" in
  sync:*) ;;
  *) fail "local sync commit was not kept" ;;
esac
