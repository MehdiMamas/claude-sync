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
run_sync push
assert_code 0 "machine A push"

add_second_home
enter_home "$TEST_HOME_B"
secret="sk-ant-$(repeat_char a 20)"
mkdir -p "$CURSOR_HOME/skills/leaky"
printf '%s\n' "$secret" > "$CURSOR_HOME/skills/leaky/SKILL.md"
cp "$CURSOR_HOME/skills/leaky/SKILL.md" "$TEST_WORK/secret-before"
printf 'keep\n' > "$CURSOR_HOME/notes.txt"
head_before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)

run_sync join --merge "$url"
assert_code 5 "secret in a Cursor import restores the machine"
assert_err_contains "sk-ant"
assert_err_contains "restored"
assert_output_lacks "$secret"
assert_file_eq "$CURSOR_HOME/skills/leaky/SKILL.md" "$TEST_WORK/secret-before"
if [ -L "$CURSOR_HOME/skills/leaky" ]; then
  fail "secret join left the skill as a link"
fi
if [ -e "$CLAUDE_SYNC_DIR/cursor/skills/leaky" ]; then
  fail "secret join left the skill in the repo"
fi
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)" "$head_before" "a commit was made"
assert_eq "$(cat "$CURSOR_HOME/notes.txt")" "keep"
