#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

printf 'note\n' > "$CLAUDE_SYNC_DIR/notes.txt"
git -C "$CLAUDE_SYNC_DIR" add notes.txt
git -C "$CLAUDE_SYNC_DIR" commit -m "track a note" >/dev/null

mkdir -p "$CLAUDE_SYNC_DIR/skills/x"
git -C "$CLAUDE_SYNC_DIR" mv notes.txt skills/x/notes.md

run_sync push
assert_code 0 "rename into the allowlist is pushed"

if git -C "$CLAUDE_SYNC_DIR" ls-files --error-unmatch -- notes.txt >/dev/null 2>&1; then
  fail "old path is still tracked after the rename"
fi
git -C "$CLAUDE_SYNC_DIR" ls-files --error-unmatch -- skills/x/notes.md >/dev/null
show=$(git -C "$CLAUDE_SYNC_DIR" show --name-status --pretty=format: HEAD)
case "$show" in
  *skills/x/notes.md*) ;;
  *) fail "sync commit missed the new path: [$show]" ;;
esac
case "$show" in
  *notes.txt*) ;;
  *) fail "sync commit missed the old path of the rename: [$show]" ;;
esac
