#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# Without cursor=1, a cursor/ folder in the repo and a real ~/.cursor are left alone.
seed_sync_repo
mkdir -p "$CLAUDE_SYNC_DIR/cursor/skills/cs" "$CURSOR_HOME/skills/local" "$CURSOR_HOME/agents"
printf 'cs\n' > "$CLAUDE_SYNC_DIR/cursor/skills/cs/SKILL.md"
printf 'local\n' > "$CURSOR_HOME/skills/local/SKILL.md"
printf 'ag\n' > "$CURSOR_HOME/agents/a.md"
before=$(find "$CURSOR_HOME" -print | sort)

run_sync link
assert_code 0 "link"
run_sync pull
assert_code 0 "pull (no remote changes)"
run_sync status
assert_code 0 "status"
case "$RUN_OUT" in
  *cursor*) fail "status listed Cursor items while off: [$RUN_OUT]" ;;
esac
run_sync push
assert_code 0 "push"

after=$(find "$CURSOR_HOME" -print | sort)
assert_eq "$after" "$before" "cursor home changed while cursor sync is off"
[ ! -e "$CURSOR_HOME/skills/cs" ] || fail "cursor skill was linked while off"
if git -C "$CLAUDE_SYNC_DIR" ls-files | grep -q '^cursor/'; then
  fail "cursor/ was committed while off"
fi

# A conf with the key set to anything but 1 is also off.
printf '# comment\ncursor=0\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
run_sync link
assert_code 0 "link with cursor=0"
after=$(find "$CURSOR_HOME" -print | sort)
assert_eq "$after" "$before" "cursor home changed with cursor=0"
