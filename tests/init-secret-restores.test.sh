#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")
secret="sk-ant-$(repeat_char a 20)"
mkdir -p "$CLAUDE_HOME/skills/secreted"
printf '%s\n' "$secret" > "$CLAUDE_HOME/skills/secreted/SKILL.md"
cp "$CLAUDE_HOME/skills/secreted/SKILL.md" "$TEST_WORK/secret-before"
printf 'keep\n' > "$CLAUDE_HOME/settings.json"

run_sync init --remote "$url"
assert_code 5 "secret blocks init"
assert_err_contains "sk-ant"
assert_err_contains "restored"
assert_output_lacks "$secret"
assert_file_eq "$CLAUDE_HOME/skills/secreted/SKILL.md" "$TEST_WORK/secret-before"
if [ -L "$CLAUDE_HOME/skills/secreted" ]; then
  fail "secret init left the skill as a link"
fi
if [ -e "$CLAUDE_SYNC_DIR/skills/secreted" ]; then
  fail "secret init left the skill in the repo"
fi
if git -C "$CLAUDE_SYNC_DIR" rev-parse --verify HEAD >/dev/null 2>&1; then
  fail "secret init created a commit"
fi
assert_eq "$(cat "$CLAUDE_HOME/settings.json")" "keep"
if [ -e "$CLAUDE_HOME/CLAUDE.md" ]; then
  fail "secret init created CLAUDE.md"
fi
