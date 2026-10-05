#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")

printf 'from-a\n' > "$CLAUDE_HOME/CLAUDE.md"
run_sync init --remote "$url"
assert_code 0 "machine A init"
printf 'cursor=1\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
mkdir -p "$CLAUDE_SYNC_DIR/cursor/rules"
printf 'from-repo\n' > "$CLAUDE_SYNC_DIR/cursor/rules/r.mdc"
run_sync push
assert_code 0 "machine A push"

add_second_home
enter_home "$TEST_HOME_B"
rm -rf "$CLAUDE_SYNC_DIR"
mkdir -p "$CURSOR_HOME/rules" "$CURSOR_HOME/agents" "$CURSOR_HOME/commands"
printf 'from-local\n' > "$CURSOR_HOME/rules/r.mdc"
printf 'agent\n' > "$CURSOR_HOME/agents/a.md"
printf 'cmd\n' > "$CURSOR_HOME/commands/c.md"
before=$(find "$CURSOR_HOME" -print | sort)

run_sync join "$url"
assert_code 4 "join stops on real Cursor directories"
assert_out_contains "conflict: cursor/rules"
assert_out_contains "conflict: cursor/agents"
assert_out_contains "conflict: cursor/commands"
assert_eq "$(find "$CURSOR_HOME" -print | sort)" "$before" "join changed ~/.cursor before --merge"

run_sync join --merge "$url"
assert_code 0 "join --merge of rules, agents and commands"
backup=$(printf '%s\n' "$RUN_OUT" | sed -n 's/^backed up: //p')
assert_eq "$(cat "$backup/cursor/rules/r.mdc")" "from-local" "backup of replaced rules"
assert_dir_link "$CURSOR_HOME/rules"
assert_dir_link "$CURSOR_HOME/agents"
assert_dir_link "$CURSOR_HOME/commands"
assert_eq "$(cat "$CURSOR_HOME/rules/r.mdc")" "from-repo" "repo wins"
assert_eq "$(cat "$CURSOR_HOME/agents/a.md")" "agent"
assert_eq "$(cat "$CURSOR_HOME/commands/c.md")" "cmd"
git -C "$CLAUDE_SYNC_DIR" ls-files | grep -q '^cursor/agents/a.md$' || fail "imported agents not committed"
git -C "$CLAUDE_SYNC_DIR" ls-files | grep -q '^cursor/commands/c.md$' || fail "imported commands not committed"
