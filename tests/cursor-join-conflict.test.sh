#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")

run_sync init --remote "$url"
assert_code 0 "machine A init"
printf 'cursor=1\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
mkdir -p "$CLAUDE_SYNC_DIR/cursor/skills/shared"
printf 'repo-shared\n' > "$CLAUDE_SYNC_DIR/cursor/skills/shared/SKILL.md"
run_sync push
assert_code 0 "machine A push"

# Machine B has real Cursor content: one skill the repo has, one it lacks.
add_second_home
enter_home "$TEST_HOME_B"
rm -rf "$CLAUDE_SYNC_DIR"
mkdir -p "$CURSOR_HOME/skills/shared" "$CURSOR_HOME/skills/mine"
printf 'local-shared\n' > "$CURSOR_HOME/skills/shared/SKILL.md"
printf 'local-mine\n' > "$CURSOR_HOME/skills/mine/SKILL.md"
before=$(find "$CURSOR_HOME" -print | sort)

run_sync join "$url"
assert_code 4 "join stops on a real Cursor skill"
assert_out_contains "conflict: cursor/skills/shared"
assert_out_contains "conflict: cursor/skills/mine"
assert_eq "$(find "$CURSOR_HOME" -print | sort)" "$before" "join changed ~/.cursor before --merge"

run_sync join --merge "$url"
assert_code 0 "join --merge"
backup=$(printf '%s\n' "$RUN_OUT" | sed -n 's/^backed up: //p')
assert_eq "$(cat "$backup/cursor/skills/shared/SKILL.md")" "local-shared" "backup of replaced skill"
assert_dir_link "$CURSOR_HOME/skills/shared"
assert_dir_link "$CURSOR_HOME/skills/mine"
assert_eq "$(cat "$CURSOR_HOME/skills/shared/SKILL.md")" "repo-shared" "repo wins"
assert_eq "$(cat "$CURSOR_HOME/skills/mine/SKILL.md")" "local-mine"
git -C "$CLAUDE_SYNC_DIR" ls-files | grep -q '^cursor/skills/mine/SKILL.md$' || fail "imported skill not committed"
