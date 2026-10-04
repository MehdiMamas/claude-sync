#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")
mkdir -p "$CLAUDE_HOME/CLAUDE.md" "$CLAUDE_HOME/skills/one"
printf 'keep\n' > "$CLAUDE_HOME/CLAUDE.md/keep"
printf 'one\n' > "$CLAUDE_HOME/skills/one/SKILL.md"

run_sync init --remote "$url"
assert_code 4 "link conflict restores local files"
assert_err_contains "restored"
assert_eq "$(cat "$CLAUDE_HOME/CLAUDE.md/keep")" "keep"
assert_eq "$(cat "$CLAUDE_HOME/skills/one/SKILL.md")" "one"
if [ -L "$CLAUDE_HOME/skills/one" ]; then
  fail "link conflict left the skill as a link"
fi
[ ! -e "$CLAUDE_SYNC_DIR" ] || fail "link conflict left the repo dir"
