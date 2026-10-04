#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
url=$(file_url "$TEST_BARE")
printf 'local\n' > "$CLAUDE_HOME/CLAUDE.md"
cp "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/before"

run_sync init --remote "$url"
assert_code 2 "init refuses a remote that already has commits"
assert_err_contains "already has commits"
assert_err_contains "Nothing was changed"
assert_file_eq "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/before"
[ ! -e "$CLAUDE_SYNC_DIR" ] || fail "refused init created a repo dir"
