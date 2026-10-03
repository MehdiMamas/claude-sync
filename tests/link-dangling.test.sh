#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
run_sync link
assert_code 0

rm -rf "$CLAUDE_SYNC_DIR/skills/alpha"
mkdir -p "$TEST_WORK/outside-target" "$CLAUDE_SYNC_DIR/old-hook"
printf 'out\n' > "$TEST_WORK/outside-target/x"
make_platform_link "$CLAUDE_HOME/skills/outside-skill" "$TEST_WORK/outside-target"
make_platform_link "$CLAUDE_HOME/old-hook" "$CLAUDE_SYNC_DIR/old-hook"
rm -rf "$TEST_WORK/outside-target" "$CLAUDE_SYNC_DIR/old-hook"

if [ ! -L "$CLAUDE_HOME/skills/alpha" ] && [ ! -d "$CLAUDE_HOME/skills/alpha" ]; then
  fail "deleted skill did not leave a link"
fi

run_sync link
assert_code 0 "link removes dangling repo links"
if [ -e "$CLAUDE_HOME/skills/alpha" ] || [ -L "$CLAUDE_HOME/skills/alpha" ]; then
  fail "dangling skill link was kept"
fi
if [ -e "$CLAUDE_HOME/old-hook" ] || [ -L "$CLAUDE_HOME/old-hook" ]; then
  fail "dangling link into the repo was kept"
fi
if [ ! -L "$CLAUDE_HOME/skills/outside-skill" ] && [ ! -d "$CLAUDE_HOME/skills/outside-skill" ]; then
  fail "dangling link outside the repo was removed"
fi
test -f "$CLAUDE_SYNC_DIR/agents/a.md" || fail "repo file disappeared during dangling cleanup"
grep -F -q -e 'agent' "$CLAUDE_SYNC_DIR/agents/a.md" || fail "repo file changed"
