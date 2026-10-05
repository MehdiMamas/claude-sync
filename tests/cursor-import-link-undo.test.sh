#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")
printf 'from-a\n' > "$CLAUDE_HOME/CLAUDE.md"
run_sync init --remote "$url"
assert_code 0 "machine A init"
printf 'cursor=1\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
mkdir -p "$CLAUDE_SYNC_DIR/cursor/skills/ok"
printf 'ok\n' > "$CLAUDE_SYNC_DIR/cursor/skills/ok/SKILL.md"
run_sync push
assert_code 0 "machine A push"

add_second_home
enter_home "$TEST_HOME_B"
mkdir -p "$CURSOR_HOME/rules" "$CURSOR_HOME/agents" "$CURSOR_HOME/commands"
printf 'not-a-directory\n' > "$CURSOR_HOME/skills"
printf 'local-rule\n' > "$CURSOR_HOME/rules/r.mdc"
printf 'agent\n' > "$CURSOR_HOME/agents/a.md"
printf 'cmd\n' > "$CURSOR_HOME/commands/c.md"
cp "$CURSOR_HOME/rules/r.mdc" "$TEST_WORK/rules-before"
cp "$CURSOR_HOME/agents/a.md" "$TEST_WORK/agents-before"
cp "$CURSOR_HOME/commands/c.md" "$TEST_WORK/commands-before"
cp "$CURSOR_HOME/skills" "$TEST_WORK/skills-before"
head_before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)

run_sync join --merge "$url"
assert_code 4 "link conflict during a Cursor import restores the machine"
assert_err_contains "restored"
assert_file_eq "$CURSOR_HOME/rules/r.mdc" "$TEST_WORK/rules-before"
assert_file_eq "$CURSOR_HOME/agents/a.md" "$TEST_WORK/agents-before"
assert_file_eq "$CURSOR_HOME/commands/c.md" "$TEST_WORK/commands-before"
assert_file_eq "$CURSOR_HOME/skills" "$TEST_WORK/skills-before"
if [ -L "$CURSOR_HOME/rules" ] || [ -L "$CURSOR_HOME/agents" ] || [ -L "$CURSOR_HOME/commands" ]; then
  fail "link conflict left a Cursor directory as a link"
fi
if [ -e "$CLAUDE_SYNC_DIR/cursor/rules" ] || [ -e "$CLAUDE_SYNC_DIR/cursor/agents" ] || [ -e "$CLAUDE_SYNC_DIR/cursor/commands" ]; then
  fail "link conflict left imported Cursor directories in the repo"
fi
assert_eq "$(cat "$CLAUDE_SYNC_DIR/cursor/skills/ok/SKILL.md")" "ok"
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)" "$head_before" "a commit was made"
