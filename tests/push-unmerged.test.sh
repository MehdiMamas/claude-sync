#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

printf 'base\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
git -C "$CLAUDE_SYNC_DIR" add CLAUDE.md
git -C "$CLAUDE_SYNC_DIR" commit -m base >/dev/null

git -C "$CLAUDE_SYNC_DIR" checkout -b side >/dev/null
printf 'side\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
git -C "$CLAUDE_SYNC_DIR" add CLAUDE.md
git -C "$CLAUDE_SYNC_DIR" commit -m side >/dev/null

git -C "$CLAUDE_SYNC_DIR" checkout main >/dev/null
printf 'main-line\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
git -C "$CLAUDE_SYNC_DIR" add CLAUDE.md
git -C "$CLAUDE_SYNC_DIR" commit -m mainline >/dev/null

git -C "$CLAUDE_SYNC_DIR" merge side >/dev/null 2>&1 || true
if [ -z "$(git -C "$CLAUDE_SYNC_DIR" ls-files -u)" ]; then
  fail "fixture did not leave an unmerged file"
fi
before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)

run_sync push
assert_code 4 "push refuses an unmerged file"
assert_err_lines 1
assert_err_contains "merge"
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)" "$before" "push committed despite a conflict"
if [ -z "$(git -C "$CLAUDE_SYNC_DIR" ls-files -u)" ]; then
  fail "push cleared the unmerged file"
fi
