#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
printf 'cursor=1\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
mkdir -p "$CLAUDE_SYNC_DIR/cursor/rules" "$CURSOR_HOME/rules"
printf 'repo-rule\n' > "$CLAUDE_SYNC_DIR/cursor/rules/r.mdc"
printf 'local-rule\n' > "$CURSOR_HOME/rules/r.mdc"
cp "$CURSOR_HOME/rules/r.mdc" "$TEST_WORK/rules-before"

run_sync link
assert_code 4 "a real rules directory is a conflict"
assert_err_contains "cursor/rules is a directory"
assert_file_eq "$CURSOR_HOME/rules/r.mdc" "$TEST_WORK/rules-before"
if [ -L "$CURSOR_HOME/rules" ]; then
  fail "rules directory was replaced"
fi
assert_eq "$(cat "$CLAUDE_SYNC_DIR/cursor/rules/r.mdc")" "repo-rule"

run_sync link
assert_code 4 "second link still reports the rules conflict"
assert_file_eq "$CURSOR_HOME/rules/r.mdc" "$TEST_WORK/rules-before"
