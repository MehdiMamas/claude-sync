#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_work
printf 'local\n' > "$CLAUDE_HOME/CLAUDE.md"
run_sync init
assert_code 0 "local-only init"
assert_out_contains "pushed: no"
assert_out_contains "no remote yet"
assert_out_contains "git -C"
if git -C "$CLAUDE_SYNC_DIR" remote get-url origin >/dev/null 2>&1; then
  fail "local-only init added a remote"
fi
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" log -1 --format=%s)" "init: import from $(uname -n)"
assert_eq "$(cat "$CLAUDE_SYNC_DIR/CLAUDE.md")" "local"
