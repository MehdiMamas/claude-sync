#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
git -C "$CLAUDE_SYNC_DIR" push origin HEAD >/dev/null 2>&1
add_second_home

enter_home "$TEST_HOME_B"
printf 'from-b\n' >> "$CLAUDE_SYNC_DIR/README.md"
git -C "$CLAUDE_SYNC_DIR" add README.md
git -C "$CLAUDE_SYNC_DIR" commit -m 'from b' >/dev/null
git -C "$CLAUDE_SYNC_DIR" push origin HEAD >/dev/null 2>&1

enter_home "$TEST_HOME"
git -C "$CLAUDE_SYNC_DIR" fetch origin >/dev/null 2>&1
printf 'from-a\n' >> "$CLAUDE_SYNC_DIR/skills/alpha/SKILL.md"
git -C "$CLAUDE_SYNC_DIR" add skills/alpha/SKILL.md
git -C "$CLAUDE_SYNC_DIR" commit -m 'from a' >/dev/null

mkdir -p "$XDG_STATE_HOME/claude-sync"
printf '2026-01-02T03:04:05Z\n' > "$XDG_STATE_HOME/claude-sync/last-pull"
printf '2026-06-07T08:09:10Z\n' > "$XDG_STATE_HOME/claude-sync/last-push"

mkdir -p "$CLAUDE_HOME/agents" "$CLAUDE_HOME/skills/local-only"
printf 'stay\n' > "$CLAUDE_HOME/agents/stay.md"
printf 'local\n' > "$CLAUDE_HOME/skills/local-only/SKILL.md"
rm -rf "$CLAUDE_SYNC_DIR/commands"

run_sync link
assert_code 4 "agents conflict does not stop status"

run_sync status
assert_code 0 "status"
assert_out_contains "CLAUDE.md: linked"
assert_out_contains "skills/alpha: linked"
assert_out_contains "agents: conflict"
assert_out_contains "commands: missing"
assert_out_contains "output-styles: linked"
assert_out_contains "branch: main"
assert_out_contains "ahead: 1"
assert_out_contains "behind: 1"
assert_out_contains "dirty: yes"
assert_out_contains "last-pull: 2026-01-02T03:04:05Z"
assert_out_contains "last-push: 2026-06-07T08:09:10Z"
case "$RUN_OUT" in
  *synced*|*local-only*) fail "status listed a skill it does not sync: $RUN_OUT" ;;
esac
grep -F -q -e 'stay' "$CLAUDE_HOME/agents/stay.md" || fail "status changed the conflict"
