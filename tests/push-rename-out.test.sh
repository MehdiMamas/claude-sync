#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

mkdir -p "$CLAUDE_SYNC_DIR/skills/x"
printf 'a\n' > "$CLAUDE_SYNC_DIR/skills/x/a.md"
git -C "$CLAUDE_SYNC_DIR" add skills/x/a.md
git -C "$CLAUDE_SYNC_DIR" commit -m "track a skill" >/dev/null
git -C "$CLAUDE_SYNC_DIR" mv skills/x/a.md notes.txt

run_sync push
assert_code 0 "rename out of the allowlist still pushes the deletion"

names=$(git -C "$CLAUDE_SYNC_DIR" show --pretty=format: --name-only HEAD)
case "$names" in
  *notes.txt*) fail "commit included notes.txt: [$names]" ;;
esac
case "$names" in
  *skills/x/a.md*) ;;
  *) fail "commit missed the allowlisted deletion: [$names]" ;;
esac
if git -C "$CLAUDE_SYNC_DIR" ls-files --error-unmatch -- notes.txt >/dev/null 2>&1; then
  # ls-files lists the index. A staged new file is listed. HEAD must not have it.
  if git -C "$CLAUDE_SYNC_DIR" cat-file -e "HEAD:notes.txt" 2>/dev/null; then
    fail "HEAD contains notes.txt"
  fi
fi
staged=$(git -C "$CLAUDE_SYNC_DIR" diff --cached --name-only)
case "$staged" in
  *notes.txt*) ;;
  *) fail "notes.txt was not left staged: [$staged]" ;;
esac
