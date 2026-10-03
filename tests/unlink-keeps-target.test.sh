#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
printf 'precious\n' > "$CLAUDE_SYNC_DIR/skills/alpha/precious.txt"
printf 'keep me\n' > "$CLAUDE_HOME/CLAUDE.md"
mkdir -p "$CLAUDE_HOME/skills/local-skill" "$TEST_WORK/elsewhere"
printf 'local\n' > "$CLAUDE_HOME/skills/local-skill/SKILL.md"
printf 'else\n' > "$TEST_WORK/elsewhere/x"
make_platform_link "$CLAUDE_HOME/skills/else-skill" "$TEST_WORK/elsewhere"

run_sync link
assert_code 0

find "$CLAUDE_SYNC_DIR" -type f | sort > "$TEST_WORK/repo-files"
: > "$TEST_WORK/repo-hashes"
while IFS= read -r f; do
  git hash-object -- "$f"
done < "$TEST_WORK/repo-files" > "$TEST_WORK/repo-hashes"

run_sync unlink
assert_code 0 "unlink"
assert_err_contains "unlinked skills/alpha"
assert_err_contains "unlinked agents"
assert_err_contains "removed the managed block"
if [ -e "$CLAUDE_HOME/skills/alpha" ] || [ -L "$CLAUDE_HOME/skills/alpha" ]; then
  fail "skill link survived unlink"
fi
if [ -L "$CLAUDE_HOME/agents" ]; then
  fail "agents link survived unlink"
fi
test -f "$CLAUDE_SYNC_DIR/skills/alpha/precious.txt" || fail "unlink deleted the skill target"
grep -F -q -e 'precious' "$CLAUDE_SYNC_DIR/skills/alpha/precious.txt" || fail "skill target changed"
test -f "$CLAUDE_SYNC_DIR/agents/a.md" || fail "unlink deleted agents"
grep -F -q -e 'keep me' "$CLAUDE_HOME/CLAUDE.md" || fail "unlink removed user CLAUDE.md text"
if grep -F -q -e 'claude-sync:begin' "$CLAUDE_HOME/CLAUDE.md"; then
  fail "unlink left the managed block"
fi
test -f "$CLAUDE_HOME/skills/local-skill/SKILL.md" || fail "unlink removed a local skill"
if [ ! -e "$CLAUDE_HOME/skills/else-skill" ] && [ ! -L "$CLAUDE_HOME/skills/else-skill" ]; then
  fail "unlink removed a link that points outside the repo"
fi
grep -F -q -e 'else' "$TEST_WORK/elsewhere/x" || fail "outside target changed"

find "$CLAUDE_SYNC_DIR" -type f | sort > "$TEST_WORK/repo-files-after"
assert_file_eq "$TEST_WORK/repo-files" "$TEST_WORK/repo-files-after"
: > "$TEST_WORK/repo-hashes-after"
while IFS= read -r f; do
  git hash-object -- "$f"
done < "$TEST_WORK/repo-files-after" > "$TEST_WORK/repo-hashes-after"
assert_file_eq "$TEST_WORK/repo-hashes" "$TEST_WORK/repo-hashes-after"

run_sync unlink
assert_code 0 "second unlink"
assert_eq "$RUN_ERR" "" "second unlink printed a change"
assert_file_eq "$TEST_WORK/repo-hashes" "$TEST_WORK/repo-hashes-after"
