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

other="$HOME/My Repo"
mkdir -p "$other"
CLAUDE_SYNC_DIR=$other
export CLAUDE_SYNC_DIR
run_sync link
assert_code 0 "update block in place"
grep -F -q -e '@~/My Repo/CLAUDE.md' "$CLAUDE_HOME/CLAUDE.md" || fail "import was not updated in place"
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
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    abs=$(cd "$outside" && pwd -L)
    abs=$(cygpath -m -- "$abs")
    case "$abs" in
      [A-Za-z]:/*) ;;
      *) fail "Windows absolute import is not a drive path: $abs" ;;
    esac
    ;;
  *)
    abs=$(cd "$outside" && pwd -L)
    ;;
esac
grep -F -q -e "@${abs}/CLAUDE.md" "$CLAUDE_HOME/CLAUDE.md" || fail "expected absolute import @$abs/CLAUDE.md"
if grep -F -q -e '@~/' "$CLAUDE_HOME/CLAUDE.md"; then
  fail "outside-home import used ~"
fi

CLAUDE_SYNC_DIR=$HOME/.claude-sync
export CLAUDE_SYNC_DIR
printf 'keep me\n' > "$CLAUDE_HOME/CLAUDE.md"
chmod 600 "$CLAUDE_HOME/CLAUDE.md"
file_mode() {
  stat -c '%a' "$1" 2>/dev/null || stat -f '%OLp' "$1"
}
mode_before=$(file_mode "$CLAUDE_HOME/CLAUDE.md")
run_sync link
assert_code 0 "link keeps CLAUDE.md mode"
mode_after=$(file_mode "$CLAUDE_HOME/CLAUDE.md")
assert_eq "$mode_after" "$mode_before" "CLAUDE.md mode changed"

printf 'notes mention <!-- claude-sync:begin (managed, do not edit inside) --> inside a sentence\n' > "$CLAUDE_HOME/CLAUDE.md"
run_sync link
assert_code 0 "quoted marker is not the block"
first=$(head -n 1 "$CLAUDE_HOME/CLAUDE.md")
assert_eq "$first" "<!-- claude-sync:begin (managed, do not edit inside) -->"
grep -F -q -e 'notes mention' "$CLAUDE_HOME/CLAUDE.md" || fail "quoted marker line was rewritten"

printf 'keep me\n' > "$CLAUDE_HOME/CLAUDE.md"
cp "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/original-claude"
bad_tmp="$TEST_WORK/not-a-dir"
: > "$bad_tmp"
saved_tmpdir=${TMPDIR-}
export TMPDIR="$bad_tmp"
run_sync link
assert_code 3 "temp file failure"
assert_err_contains "left unchanged"
assert_file_eq "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/original-claude"
if [ -n "$saved_tmpdir" ]; then
  export TMPDIR="$saved_tmpdir"
else
  unset TMPDIR
fi

printf '%s\nkeep me\n' '<!-- claude-sync:begin (managed, do not edit inside) -->' > "$CLAUDE_HOME/CLAUDE.md"
cp "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/broken-claude"
run_sync link
assert_code 4 "begin without end"
assert_err_contains "start without an end"
assert_file_eq "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/broken-claude"
run_sync unlink
assert_code 4 "unlink begin without end"
assert_err_contains "start without an end"
assert_file_eq "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/broken-claude"

printf '%s\n%s\nkeep me\n' '<!-- claude-sync:end -->' '<!-- claude-sync:begin (managed, do not edit inside) -->' > "$CLAUDE_HOME/CLAUDE.md"
cp "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/broken-claude"
run_sync link
assert_code 4 "markers out of order"
assert_err_contains "out of order"
assert_file_eq "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/broken-claude"
run_sync unlink
assert_code 4 "unlink markers out of order"
assert_err_contains "out of order"
assert_file_eq "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/broken-claude"

rm -f "$CLAUDE_HOME/CLAUDE.md"
printf 'keep me\n' > "$TEST_WORK/claude-target"
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    wlink=$(cygpath -w -- "$CLAUDE_HOME/CLAUDE.md")
    wtarget=$(cygpath -w -- "$TEST_WORK/claude-target")
    cmd.exe //c mklink "$wlink" "$wtarget" >/dev/null 2>&1 || true
    ;;
  *)
    ln -s -- "$TEST_WORK/claude-target" "$CLAUDE_HOME/CLAUDE.md"
    ;;
esac
if [ -L "$CLAUDE_HOME/CLAUDE.md" ]; then
  run_sync link
  assert_code 4 "CLAUDE.md symlink"
  assert_err_contains "not a regular file"
  assert_file_eq "$TEST_WORK/claude-target" "$TEST_WORK/original-claude"
  if [ ! -L "$CLAUDE_HOME/CLAUDE.md" ]; then
    fail "CLAUDE.md symlink was replaced"
  fi
  rm -f "$CLAUDE_HOME/CLAUDE.md"
else
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
      printf 'skip: this Windows shell cannot create a file symlink\n'
      rm -rf "$CLAUDE_HOME/CLAUDE.md"
      mkdir "$CLAUDE_HOME/CLAUDE.md"
      printf 'keep me\n' > "$CLAUDE_HOME/CLAUDE.md/inside"
      cp "$CLAUDE_HOME/CLAUDE.md/inside" "$TEST_WORK/dir-inside"
      run_sync link
      assert_code 4 "CLAUDE.md directory"
      assert_err_contains "not a regular file"
      assert_file_eq "$CLAUDE_HOME/CLAUDE.md/inside" "$TEST_WORK/dir-inside"
      ;;
    *)
      fail "could not create a CLAUDE.md symlink"
      ;;
  esac
fi
