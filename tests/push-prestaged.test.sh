#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

printf 'instructions\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
printf 'note\n' > "$CLAUDE_SYNC_DIR/notes.txt"
git -C "$CLAUDE_SYNC_DIR" add notes.txt

run_sync push
assert_code 0 "pre-staged notes.txt does not block the allowlisted commit"

names=$(git -C "$CLAUDE_SYNC_DIR" show --pretty=format: --name-only HEAD)
case "$names" in
  *CLAUDE.md*) ;;
  *) fail "commit missed CLAUDE.md: [$names]" ;;
esac
case "$names" in
  *notes.txt*) fail "commit included pre-staged notes.txt: [$names]" ;;
esac

staged=$(git -C "$CLAUDE_SYNC_DIR" diff --cached --name-only)
case "$staged" in
  *notes.txt*) ;;
  *) fail "pre-staged notes.txt was dropped: [$staged]" ;;
esac
