#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

marked="sk-ant-$(repeat_char b 20) claude-sync: allow-secret"
printf '%s\n' "$marked" > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 0 "allow-secret line is committed"
assert_output_lacks "sk-ant-"
if ! grep -F -q -e "claude-sync: allow-secret" "$CLAUDE_SYNC_DIR/CLAUDE.md"; then
  fail "marker missing from the work tree"
fi
git -C "$CLAUDE_SYNC_DIR" diff --cached --quiet || fail "allow-secret push left the index dirty"

# The marker applies to that line only.
bad="ghp_$(repeat_char c 36)"
printf '%s\n%s\n' "$marked" "$bad" > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 5 "unmarked line still blocks"
assert_err_contains "CLAUDE.md:2:ghp"
assert_output_lacks "$bad"
git -C "$CLAUDE_SYNC_DIR" diff --cached --quiet || fail "blocked push left the index dirty"
