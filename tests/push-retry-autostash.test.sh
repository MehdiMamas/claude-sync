#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here
add_second_home

enter_home "$TEST_HOME_B"
printf 'from-remote\n' > "$CLAUDE_SYNC_DIR/README.md"
git -C "$CLAUDE_SYNC_DIR" add README.md
git -C "$CLAUDE_SYNC_DIR" commit -m "remote readme" >/dev/null
git -C "$CLAUDE_SYNC_DIR" push >/dev/null

enter_home "$TEST_HOME"
printf 'instructions\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
printf 'from-local\n' > "$CLAUDE_SYNC_DIR/README.md"
remote_head=$(git --git-dir="$TEST_BARE" rev-parse refs/heads/main)

run_sync push
assert_code 4 "retry stops when local edits clash"
assert_err_contains "clashed"
assert_eq "$(cat "$CLAUDE_SYNC_DIR/README.md")" "from-local" "local edit was not restored"
case "$(git -C "$CLAUDE_SYNC_DIR" log -1 --format=%s)" in
  sync:*) ;;
  *) fail "local sync commit was not kept" ;;
esac
assert_eq "$(git --git-dir="$TEST_BARE" rev-parse refs/heads/main)" "$remote_head" "remote head changed"
if git -C "$CLAUDE_SYNC_DIR" rev-parse --verify refs/stash >/dev/null 2>&1; then
  fail "the retry left a stash entry"
fi
if [ -n "$(git -C "$CLAUDE_SYNC_DIR" ls-files -u)" ]; then
  fail "the retry left unmerged files"
fi
