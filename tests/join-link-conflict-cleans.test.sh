#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")
mkdir -p "$CLAUDE_HOME/skills/one"
printf 'one\n' > "$CLAUDE_HOME/skills/one/SKILL.md"
run_sync init --remote "$url"
assert_code 0 "init a repo that has a skill"

add_second_home
enter_home "$TEST_HOME_B"
printf 'not-a-directory\n' > "$CLAUDE_HOME/skills"
run_sync join "$url"
assert_code 4 "join link conflict cleans a block it just wrote"
assert_err_contains "restored"
assert_eq "$(cat "$CLAUDE_HOME/skills")" "not-a-directory"
[ ! -e "$CLAUDE_HOME/CLAUDE.md" ] || fail "join left a CLAUDE.md it created during the failed link"
