#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_work
git init -b main "$CLAUDE_SYNC_DIR" >/dev/null
harden_repo "$CLAUDE_SYNC_DIR"

run_sync status
assert_code 0 "status on a repo with no commits"
assert_out_contains "branch: main"
assert_out_contains "ahead: none"
assert_out_contains "behind: none"
assert_out_contains "CLAUDE.md: missing"

printf 'seed\n' > "$CLAUDE_SYNC_DIR/README.md"
git -C "$CLAUDE_SYNC_DIR" add README.md
git -C "$CLAUDE_SYNC_DIR" commit -m seed >/dev/null
git -C "$CLAUDE_SYNC_DIR" checkout --detach >/dev/null 2>&1

run_sync status
assert_code 0 "status on a detached HEAD"
assert_out_contains "branch: detached"
assert_out_contains "ahead: no-upstream"
assert_out_contains "behind: no-upstream"
