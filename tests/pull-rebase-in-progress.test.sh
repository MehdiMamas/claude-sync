#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

rebase_in_progress_here() {
  local gd
  gd=$(git -C "$CLAUDE_SYNC_DIR" rev-parse --absolute-git-dir)
  [ -d "$gd/rebase-merge" ] || [ -d "$gd/rebase-apply" ]
}

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
git -C "$CLAUDE_SYNC_DIR" pull --rebase >/dev/null 2>&1 || true
if ! rebase_in_progress_here; then
  fail "fixture did not leave a rebase in progress"
fi
file_before=$(cat "$CLAUDE_SYNC_DIR/CLAUDE.md")

run_sync pull
assert_code 4 "pull leaves a manual rebase alone"
assert_err_contains "rebase"
file_after=$(cat "$CLAUDE_SYNC_DIR/CLAUDE.md")
assert_eq "$file_after" "$file_before" "rebase conflict file changed"
gd=$(git -C "$CLAUDE_SYNC_DIR" rev-parse --absolute-git-dir)
if [ ! -d "$gd/rebase-merge" ] && [ ! -d "$gd/rebase-apply" ]; then
  fail "pull aborted the user's rebase"
fi
