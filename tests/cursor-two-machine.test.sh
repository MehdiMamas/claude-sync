#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")

# Machine A: set up, then opt in and add a Cursor skill.
printf 'from-a\n' > "$CLAUDE_HOME/CLAUDE.md"
run_sync init --remote "$url"
assert_code 0 "machine A init"
printf '# comment\n  cursor = 1 \r\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
mkdir -p "$CLAUDE_SYNC_DIR/cursor/skills/cs" "$CLAUDE_SYNC_DIR/cursor/commands" "$CLAUDE_SYNC_DIR/cursor/rules"
printf 'cursor-skill\n' > "$CLAUDE_SYNC_DIR/cursor/skills/cs/SKILL.md"
printf 'cmd\n' > "$CLAUDE_SYNC_DIR/cursor/commands/c.md"
printf 'rule\n' > "$CLAUDE_SYNC_DIR/cursor/rules/r.mdc"
run_sync push
assert_code 0 "machine A push"
git -C "$CLAUDE_SYNC_DIR" ls-files | grep -q '^cursor/skills/cs/SKILL.md$' || fail "skill not committed"
git -C "$CLAUDE_SYNC_DIR" ls-files | grep -q '^claude-sync.conf$' || fail "conf not committed"
run_sync link
assert_code 0 "machine A link"
assert_dir_link "$CURSOR_HOME/skills/cs"
assert_dir_link "$CURSOR_HOME/commands"
assert_dir_link "$CURSOR_HOME/rules"
run_sync status
assert_out_contains "cursor/skills/cs: linked"
assert_out_contains "cursor/commands: linked"
assert_out_contains "cursor/rules: linked"
assert_out_contains "cursor/agents: missing"

# Machine B: pull links the skill; the other Claude content is unaffected.
add_second_home
enter_home "$TEST_HOME_B"
run_sync pull
assert_code 0 "machine B pull"
assert_dir_link "$CURSOR_HOME/skills/cs"
assert_eq "$(cat "$CURSOR_HOME/skills/cs/SKILL.md")" "cursor-skill"

# An edit on B reaches A through the link.
printf 'edited\n' > "$CURSOR_HOME/skills/cs/SKILL.md"
run_sync push
assert_code 0 "machine B push"
enter_home "$TEST_HOME"
run_sync pull
assert_code 0 "machine A pull"
assert_eq "$(cat "$CURSOR_HOME/skills/cs/SKILL.md")" "edited"

# A skill removed on A disappears from B after pull, and unlink keeps the repo files.
git -C "$CLAUDE_SYNC_DIR" rm -rq cursor/skills/cs
run_sync push
assert_code 0 "machine A push removal"
enter_home "$TEST_HOME_B"
run_sync pull
assert_code 0 "machine B pull removal"
[ ! -e "$CURSOR_HOME/skills/cs" ] && [ ! -L "$CURSOR_HOME/skills/cs" ] || fail "dangling cursor link stayed"
run_sync unlink
assert_code 0 "unlink"
[ ! -L "$CURSOR_HOME/commands" ] || fail "unlink left the commands link"
[ ! -L "$CURSOR_HOME/rules" ] || fail "unlink left the rules link"
[ -f "$CLAUDE_SYNC_DIR/cursor/commands/c.md" ] || fail "unlink removed repo content"
[ -f "$CLAUDE_SYNC_DIR/cursor/rules/r.mdc" ] || fail "unlink removed the rules file"
