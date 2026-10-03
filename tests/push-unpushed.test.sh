#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here
remote_before=$(git --git-dir="$TEST_BARE" rev-parse refs/heads/main)

printf 'queued\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
git -C "$CLAUDE_SYNC_DIR" remote set-url origin "$(file_url "$TEST_WORK/missing.git")"
run_sync push
assert_code 3 "push fails when the remote is unreachable"
assert_eq "$(git --git-dir="$TEST_BARE" rev-parse refs/heads/main)" "$remote_before" "failed push published a commit"
case "$(git -C "$CLAUDE_SYNC_DIR" log -1 --format=%s)" in
  sync:*) ;;
  *) fail "local commit was not created before the failed push" ;;
esac
local_head=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)

git -C "$CLAUDE_SYNC_DIR" remote set-url origin "$(file_url "$TEST_BARE")"
run_sync push
assert_code 0 "a later push sends the commit that is already local"
assert_eq "$(git --git-dir="$TEST_BARE" rev-parse refs/heads/main)" "$local_head"
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" status --porcelain)" "" "second push with a clean tree changed files"
