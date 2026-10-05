#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
mkdir -p "$CLAUDE_SYNC_DIR/cursor/rules"
printf 'rule\n' > "$CLAUDE_SYNC_DIR/cursor/rules/r.mdc"

rules_linked() {
  if [ -e "$CURSOR_HOME/rules" ] || [ -L "$CURSOR_HOME/rules" ]; then
    return 0
  fi
  return 1
}

printf '  cursor = 1 \r\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
run_sync link
assert_code 0 "spaces and CRLF enable cursor sync"
assert_dir_link "$CURSOR_HOME/rules"

run_sync unlink
assert_code 0 "unlink before the off case"
printf 'cursor=1\r\ncursor=0\r\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
run_sync link
assert_code 0 "last cursor=0 wins"
if rules_linked; then
  fail "last key cursor=0 still linked rules"
fi

printf 'cursor=0\n  cursor = 1 \r\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
run_sync link
assert_code 0 "last cursor=1 wins"
assert_dir_link "$CURSOR_HOME/rules"

run_sync unlink
assert_code 0 "unlink before the comment case"
printf 'cursor=0\ncursor=1 # on\r\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
run_sync link
assert_code 0 "inline comment still enables cursor sync"
assert_dir_link "$CURSOR_HOME/rules"

run_sync unlink
assert_code 0 "unlink before the comment-line case"
printf '# cursor=1\ncursor=0\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
run_sync link
assert_code 0 "a comment line is not a value"
if rules_linked; then
  fail "a comment line enabled cursor sync"
fi
