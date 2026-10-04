#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_work
custom="$TEST_HOME/custom sync"
printf 'in-dir\n' > "$CLAUDE_HOME/CLAUDE.md"
run_sync init --dir "$custom"
assert_code 0 "init --dir"
[ -d "$custom/.git" ] || fail "init --dir did not create the repo"
[ ! -e "$HOME/.claude-sync" ] || fail "init --dir also used the default repo path"
assert_eq "$(cat "$custom/CLAUDE.md")" "in-dir"
assert_out_contains "no remote yet"
