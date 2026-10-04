#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")
mkdir -p "$CLAUDE_HOME/CLAUDE.md" "$CLAUDE_HOME/skills/one"
printf 'keep\n' > "$CLAUDE_HOME/CLAUDE.md/keep"
printf 'one\n' > "$CLAUDE_HOME/skills/one/SKILL.md"

# Exported into the init process. That process calls mv; this shell does not.
# shellcheck disable=SC2329
mv() {
  dest=""
  for dest in "$@"; do
    :
  done
  case "$dest" in
    "$HOME/.claude"/skills/*)
      return 1
      ;;
  esac
  command mv "$@"
}
export -f mv

run_sync init --remote "$url"
assert_code 4 "undo keeps the repo when the restore move fails"
assert_err_contains "could not put every file back"
assert_err_contains "$CLAUDE_SYNC_DIR"
case "$RUN_ERR" in
  *restored*) fail "undo claimed files were restored: [$RUN_ERR]" ;;
esac
[ -d "$CLAUDE_SYNC_DIR/skills/one" ] || fail "repo dir was removed while the skill was still in it"
assert_eq "$(cat "$CLAUDE_SYNC_DIR/skills/one/SKILL.md")" "one"
assert_eq "$(cat "$CLAUDE_HOME/CLAUDE.md/keep")" "keep"
