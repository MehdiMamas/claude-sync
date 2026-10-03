#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
mkdir -p "$CLAUDE_HOME/skills/local-skill"
printf 'local-skill\n' > "$CLAUDE_HOME/skills/local-skill/SKILL.md"
cp "$CLAUDE_HOME/skills/local-skill/SKILL.md" "$TEST_WORK/local-before"
cp "$CLAUDE_SYNC_DIR/skills/synced/managed/SKILL.md" "$TEST_WORK/synced-before"

run_sync link
assert_code 0
assert_file_eq "$CLAUDE_HOME/skills/local-skill/SKILL.md" "$TEST_WORK/local-before"
if [ -L "$CLAUDE_HOME/skills/local-skill" ]; then
  fail "local skill was replaced with a link"
fi
if [ -e "$CLAUDE_HOME/skills/synced" ] || [ -L "$CLAUDE_HOME/skills/synced" ]; then
  fail "skills/synced was linked"
fi
assert_file_eq "$CLAUDE_SYNC_DIR/skills/synced/managed/SKILL.md" "$TEST_WORK/synced-before"
assert_dir_link "$CLAUDE_HOME/skills/alpha"
