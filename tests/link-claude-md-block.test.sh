#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
printf 'keep me\n' > "$CLAUDE_HOME/CLAUDE.md"
cp "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/original-claude"

run_sync link
assert_code 0 "insert block"
assert_err_contains "updated the managed block"
first=$(head -n 1 "$CLAUDE_HOME/CLAUDE.md")
assert_eq "$first" "<!-- claude-sync:begin (managed, do not edit inside) -->"
grep -F -q -e '@~/.claude-sync/CLAUDE.md' "$CLAUDE_HOME/CLAUDE.md" || fail "missing home-relative import"
tail -n 1 "$CLAUDE_HOME/CLAUDE.md" > "$TEST_WORK/last-line"
assert_file_eq "$TEST_WORK/last-line" "$TEST_WORK/original-claude"
begins=$(grep -c -F -e 'claude-sync:begin' "$CLAUDE_HOME/CLAUDE.md" || true)
assert_eq "$begins" "1"

other="$HOME/other repo"
mkdir -p "$other"
CLAUDE_SYNC_DIR=$other
export CLAUDE_SYNC_DIR
run_sync link
assert_code 0 "update block in place"
grep -F -q -e '@~/other repo/CLAUDE.md' "$CLAUDE_HOME/CLAUDE.md" || fail "import was not updated in place"
begins=$(grep -c -F -e 'claude-sync:begin' "$CLAUDE_HOME/CLAUDE.md" || true)
assert_eq "$begins" "1" "block was duplicated"
tail -n 1 "$CLAUDE_HOME/CLAUDE.md" > "$TEST_WORK/last-line"
assert_file_eq "$TEST_WORK/last-line" "$TEST_WORK/original-claude"

CLAUDE_SYNC_DIR=$HOME/.claude-sync
export CLAUDE_SYNC_DIR
run_sync unlink
assert_code 0 "unlink removes the block"
assert_err_contains "removed the managed block"
assert_file_eq "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/original-claude"

rm -f "$CLAUDE_HOME/CLAUDE.md"
run_sync link
assert_code 0 "create missing CLAUDE.md"
test -f "$CLAUDE_HOME/CLAUDE.md" || fail "CLAUDE.md was not created"
grep -F -q -e '@~/.claude-sync/CLAUDE.md' "$CLAUDE_HOME/CLAUDE.md" || fail "created file has no import"
run_sync unlink
assert_code 0
if grep -F -q -e 'claude-sync:begin' "$CLAUDE_HOME/CLAUDE.md"; then
  fail "unlink left the managed block"
fi

outside="$TEST_WORK/outside"
mkdir -p "$outside"
CLAUDE_SYNC_DIR=$outside
export CLAUDE_SYNC_DIR
printf 'keep me\n' > "$CLAUDE_HOME/CLAUDE.md"
run_sync link
assert_code 0 "absolute import outside home"
abs=$(cd "$outside" && pwd -L)
grep -F -q -e "@${abs}/CLAUDE.md" "$CLAUDE_HOME/CLAUDE.md" || fail "expected absolute import @$abs/CLAUDE.md"
if grep -F -q -e '@~/' "$CLAUDE_HOME/CLAUDE.md"; then
  fail "outside-home import used ~"
fi
