#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")
mkdir -p "$CLAUDE_HOME/skills/one" "$CLAUDE_HOME/skills/two"
printf 'keep-me\n' > "$CLAUDE_HOME/CLAUDE.md"
printf 'one\n' > "$CLAUDE_HOME/skills/one/SKILL.md"
printf 'two\n' > "$CLAUDE_HOME/skills/two/SKILL.md"
cp "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/claude-before"
cp "$CLAUDE_HOME/skills/one/SKILL.md" "$TEST_WORK/one-before"
cp "$CLAUDE_HOME/skills/two/SKILL.md" "$TEST_WORK/two-before"

echo 0 > "$TEST_WORK/mv-count"
# Exported into the init process. That process calls mv; this shell does not.
# shellcheck disable=SC2317,SC2329
mv() {
  dest=""
  for dest in "$@"; do
    :
  done
  # Fail the second move into the repo. The undo moves files back home.
  case "$dest" in
    "$HOME/.claude-sync"/*)
      n=$(cat "$TEST_WORK/mv-count")
      n=$((n + 1))
      printf '%s\n' "$n" > "$TEST_WORK/mv-count"
      if [ "$n" -ge 2 ]; then
        return 1
      fi
      ;;
  esac
  command mv "$@"
}
export -f mv

run_sync init --remote "$url"
assert_code 1 "init stops when a move fails"
assert_file_eq "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/claude-before"
assert_file_eq "$CLAUDE_HOME/skills/one/SKILL.md" "$TEST_WORK/one-before"
assert_file_eq "$CLAUDE_HOME/skills/two/SKILL.md" "$TEST_WORK/two-before"
if [ -L "$CLAUDE_HOME/skills/one" ] || [ -L "$CLAUDE_HOME/skills/two" ]; then
  fail "failed init left a skill link"
fi
[ ! -e "$CLAUDE_SYNC_DIR" ] || fail "failed init left the repo dir"

unset -f mv
run_sync init --remote "$url"
assert_code 0 "init works after the failed move was undone"
assert_eq "$(cat "$CLAUDE_SYNC_DIR/skills/two/SKILL.md")" "two"
