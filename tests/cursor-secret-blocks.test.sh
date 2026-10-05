#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
printf 'cursor=1\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
mkdir -p "$CLAUDE_SYNC_DIR/cursor/skills/leaky"
secret="ghp_$(repeat_char a 36)"
printf 'token %s\n' "$secret" > "$CLAUDE_SYNC_DIR/cursor/skills/leaky/SKILL.md"
head_before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)

run_sync push
assert_code 5 "secret in cursor/ blocks the push"
assert_err_contains "cursor/skills/leaky/SKILL.md:1:ghp"
assert_output_lacks "$secret"
git -C "$CLAUDE_SYNC_DIR" diff --cached --quiet || fail "staged diff left behind"
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)" "$head_before" "a commit was made"
