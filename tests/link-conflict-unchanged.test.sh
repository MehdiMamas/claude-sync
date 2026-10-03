#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
mkdir -p "$CLAUDE_HOME/agents" "$CLAUDE_HOME/skills" "$TEST_WORK/elsewhere/commands"
printf 'keep-agent\n' > "$CLAUDE_HOME/agents/keep.md"
printf 'not-a-skill\n' > "$CLAUDE_HOME/skills/alpha"
printf 'elsewhere\n' > "$TEST_WORK/elsewhere/commands/c.md"
make_platform_link "$CLAUDE_HOME/commands" "$TEST_WORK/elsewhere/commands"

cp "$CLAUDE_HOME/agents/keep.md" "$TEST_WORK/agent-before"
cp "$CLAUDE_HOME/skills/alpha" "$TEST_WORK/skill-before"
cp "$TEST_WORK/elsewhere/commands/c.md" "$TEST_WORK/else-before"
before_target=$(readlink "$CLAUDE_HOME/commands")

run_sync link
assert_code 4 "conflicts leave local files"
assert_err_contains "agents is a directory"
assert_err_contains "skills/alpha is a file"
assert_err_contains "commands points somewhere else"
assert_file_eq "$CLAUDE_HOME/agents/keep.md" "$TEST_WORK/agent-before"
assert_file_eq "$CLAUDE_HOME/skills/alpha" "$TEST_WORK/skill-before"
assert_file_eq "$TEST_WORK/elsewhere/commands/c.md" "$TEST_WORK/else-before"
if [ -L "$CLAUDE_HOME/agents" ]; then
  fail "agents directory was replaced"
fi
after_target=$(readlink "$CLAUDE_HOME/commands")
assert_eq "$after_target" "$before_target" "commands link changed"
assert_dir_link "$CLAUDE_HOME/output-styles"

run_sync link
assert_code 4 "second link still reports the conflict"
assert_file_eq "$CLAUDE_HOME/agents/keep.md" "$TEST_WORK/agent-before"
