#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# Links into the repo are removed after cursor=1 is gone. A link that points
# elsewhere is kept, and the repo files stay.
seed_sync_repo
printf 'cursor=1\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
mkdir -p "$CLAUDE_SYNC_DIR/cursor/skills/cs" "$CLAUDE_SYNC_DIR/cursor/rules"
printf 'skill\n' > "$CLAUDE_SYNC_DIR/cursor/skills/cs/SKILL.md"
printf 'rule\n' > "$CLAUDE_SYNC_DIR/cursor/rules/r.mdc"
mkdir -p "$TEST_WORK/outside/agents"
printf 'out\n' > "$TEST_WORK/outside/agents/a.md"

run_sync link
assert_code 0 "link with cursor=1"
assert_dir_link "$CURSOR_HOME/skills/cs"
assert_dir_link "$CURSOR_HOME/rules"
make_platform_link "$CURSOR_HOME/agents" "$TEST_WORK/outside/agents"

rm -rf "$CLAUDE_SYNC_DIR/cursor/skills/cs" "$TEST_WORK/outside/agents"
printf 'cursor=0\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
run_sync link
assert_code 0 "dangling cleanup runs with cursor off"
if [ -e "$CURSOR_HOME/skills/cs" ] || [ -L "$CURSOR_HOME/skills/cs" ]; then
  fail "dangling cursor skill link stayed"
fi
if [ ! -L "$CURSOR_HOME/agents" ] && [ ! -d "$CURSOR_HOME/agents" ]; then
  fail "dangling link outside the repo was removed"
fi
assert_dir_link "$CURSOR_HOME/rules"
assert_eq "$(cat "$CLAUDE_SYNC_DIR/cursor/rules/r.mdc")" "rule"

rm -f "$CLAUDE_SYNC_DIR/claude-sync.conf"
run_sync unlink
assert_code 0 "unlink with the conf file gone"
if [ -e "$CURSOR_HOME/rules" ] || [ -L "$CURSOR_HOME/rules" ]; then
  fail "unlink left the rules link"
fi
if [ ! -L "$CURSOR_HOME/agents" ] && [ ! -d "$CURSOR_HOME/agents" ]; then
  fail "unlink removed a link that points outside the repo"
fi
assert_eq "$(cat "$CLAUDE_SYNC_DIR/cursor/rules/r.mdc")" "rule"

printf 'cursor=1\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
run_sync link
assert_code 0 "link rules again"
assert_dir_link "$CURSOR_HOME/rules"
printf 'cursor=0\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
rm -rf "$CLAUDE_HOME/CLAUDE.md"
mkdir -p "$CLAUDE_HOME/CLAUDE.md"
printf 'keep\n' > "$CLAUDE_HOME/CLAUDE.md/keep"
url=$(git -C "$CLAUDE_SYNC_DIR" remote get-url origin)
run_sync join "$url"
assert_code 4 "failed join undoes cursor links while cursor is off"
assert_err_contains "restored"
if [ -e "$CURSOR_HOME/rules" ] || [ -L "$CURSOR_HOME/rules" ]; then
  fail "undo left the rules link"
fi
if [ ! -L "$CURSOR_HOME/agents" ] && [ ! -d "$CURSOR_HOME/agents" ]; then
  fail "undo removed a link that points outside the repo"
fi
assert_eq "$(cat "$CLAUDE_SYNC_DIR/cursor/rules/r.mdc")" "rule"
assert_eq "$(cat "$CLAUDE_HOME/CLAUDE.md/keep")" "keep"
