#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
mkdir -p "$CLAUDE_HOME/skills/local-skill" "$CLAUDE_HOME/skills/synced/x"
printf 'local-skill\n' > "$CLAUDE_HOME/skills/local-skill/SKILL.md"
printf 'local-synced\n' > "$CLAUDE_HOME/skills/synced/x/SKILL.md"
cp "$CLAUDE_HOME/skills/local-skill/SKILL.md" "$TEST_WORK/local-before"
cp "$CLAUDE_HOME/skills/synced/x/SKILL.md" "$TEST_WORK/local-synced-before"
cp "$CLAUDE_SYNC_DIR/skills/synced/managed/SKILL.md" "$TEST_WORK/synced-before"

run_sync link
assert_code 0
assert_file_eq "$CLAUDE_HOME/skills/local-skill/SKILL.md" "$TEST_WORK/local-before"
assert_file_eq "$CLAUDE_HOME/skills/synced/x/SKILL.md" "$TEST_WORK/local-synced-before"
if [ -L "$CLAUDE_HOME/skills/local-skill" ] || [ -L "$CLAUDE_HOME/skills/synced" ]; then
  fail "local skill was replaced with a link"
fi
assert_file_eq "$CLAUDE_SYNC_DIR/skills/synced/managed/SKILL.md" "$TEST_WORK/synced-before"
assert_dir_link "$CLAUDE_HOME/skills/alpha"

run_sync unlink
assert_code 0
assert_file_eq "$CLAUDE_HOME/skills/synced/x/SKILL.md" "$TEST_WORK/local-synced-before"
if [ -L "$CLAUDE_HOME/skills/synced" ]; then
  fail "unlink replaced the local synced directory"
fi
