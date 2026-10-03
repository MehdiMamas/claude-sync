#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here
add_second_home

enter_home "$TEST_HOME"
printf 'from-a\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 0 "push from the first clone"

enter_home "$TEST_HOME_B"
run_sync pull
assert_code 0 "pull on the second clone"
assert_eq "$(<"$CLAUDE_SYNC_DIR/CLAUDE.md")" "from-a" "pulled file"
