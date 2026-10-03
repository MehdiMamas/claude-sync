#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo

saved_repo=$CLAUDE_SYNC_DIR
CLAUDE_SYNC_DIR="$TEST_WORK/missing repo"
export CLAUDE_SYNC_DIR
run_sync link
assert_code 2 "link without a repo"
assert_err_contains "no repo"
CLAUDE_SYNC_DIR=$saved_repo
export CLAUDE_SYNC_DIR

run_sync link
assert_code 0 "first link"
assert_err_contains "linked skills/alpha"
assert_err_contains "linked agents"
assert_err_contains "linked commands"
assert_err_contains "linked output-styles"
assert_err_contains "created CLAUDE.md"
assert_eq "$RUN_OUT" ""

assert_dir_link "$CLAUDE_HOME/skills/alpha"
assert_dir_link "$CLAUDE_HOME/agents"
assert_dir_link "$CLAUDE_HOME/commands"
assert_dir_link "$CLAUDE_HOME/output-styles"

printf 'from-link\n' > "$CLAUDE_HOME/skills/alpha/from-link.txt"
assert_file_eq "$CLAUDE_HOME/skills/alpha/from-link.txt" "$CLAUDE_SYNC_DIR/skills/alpha/from-link.txt"
printf 'from-repo\n' > "$CLAUDE_SYNC_DIR/agents/from-repo.txt"
assert_file_eq "$CLAUDE_SYNC_DIR/agents/from-repo.txt" "$CLAUDE_HOME/agents/from-repo.txt"

cp "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/claude-after-link"
run_sync link
assert_code 0 "second link"
assert_eq "$RUN_ERR" "" "second link printed a change"
assert_eq "$RUN_OUT" ""
assert_file_eq "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/claude-after-link"
assert_file_eq "$CLAUDE_HOME/skills/alpha/from-link.txt" "$CLAUDE_SYNC_DIR/skills/alpha/from-link.txt"

# A directory the repo does not have is not created locally.
run_sync unlink
assert_code 0
rm -rf "$CLAUDE_SYNC_DIR/output-styles"
git -C "$CLAUDE_SYNC_DIR" add -A -- output-styles
git -C "$CLAUDE_SYNC_DIR" commit -m 'drop output-styles' >/dev/null
run_sync link
assert_code 0 "link without output-styles"
if [ -e "$CLAUDE_HOME/output-styles" ] || [ -L "$CLAUDE_HOME/output-styles" ]; then
  fail "link created output-styles when the repo has none"
fi
assert_dir_link "$CLAUDE_HOME/skills/alpha"
