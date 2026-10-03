#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")

mkdir -p "$CLAUDE_HOME/skills/one" "$CLAUDE_HOME/skills/two" "$CLAUDE_HOME/agents"
printf 'from-a\n' > "$CLAUDE_HOME/CLAUDE.md"
printf 'skill-one\n' > "$CLAUDE_HOME/skills/one/SKILL.md"
printf 'skill-two\n' > "$CLAUDE_HOME/skills/two/SKILL.md"
printf 'agent-a\n' > "$CLAUDE_HOME/agents/a.md"

run_sync init --remote "$url"
assert_code 0 "machine A init"

make_other_home
enter_home "$TEST_HOME_B"
mkdir -p "$CLAUDE_HOME/skills/extra"
printf 'from-b\n' > "$CLAUDE_HOME/CLAUDE.md"
printf 'skill-extra\n' > "$CLAUDE_HOME/skills/extra/SKILL.md"
cp "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/b-claude"
cp "$CLAUDE_HOME/skills/extra/SKILL.md" "$TEST_WORK/b-extra"
before=$(find "$CLAUDE_HOME" -print | sort)

run_sync join "$url"
assert_code 4 "join stops on local content"
assert_out_contains "conflict: CLAUDE.md"
assert_out_contains "conflict: skills/extra"
assert_err_contains "Nothing local was changed"
assert_err_contains "--merge"
after=$(find "$CLAUDE_HOME" -print | sort)
assert_eq "$after" "$before" "join changed the local tree"
assert_file_eq "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/b-claude"
assert_file_eq "$CLAUDE_HOME/skills/extra/SKILL.md" "$TEST_WORK/b-extra"
if [ -L "$CLAUDE_HOME/skills/extra" ]; then
  fail "join linked a skill before --merge"
fi
[ ! -d "$CLAUDE_HOME/.claude-sync-backup" ] || fail "join created a backup before --merge"

run_sync join --merge "$url"
assert_code 0 "join --merge"
assert_out_contains "skills/extra"
assert_out_contains "pushed: yes"
backup=$(printf '%s\n' "$RUN_OUT" | sed -n 's/^backed up: //p')
[ -n "$backup" ] || fail "merge did not name a backup"
assert_file_eq "$backup/CLAUDE.md" "$TEST_WORK/b-claude"
assert_dir_link "$CLAUDE_HOME/skills/extra"
assert_eq "$(cat "$CLAUDE_HOME/skills/extra/SKILL.md")" "skill-extra"
host=$(uname -n)
case "$(cat "$CLAUDE_SYNC_DIR/CLAUDE.md")" in
  *from-a*"## From $host"*from-b*) ;;
  *) fail "merged CLAUDE.md missing both bodies: [$(cat "$CLAUDE_SYNC_DIR/CLAUDE.md")]" ;;
esac

printf 'edited-one\n' > "$CLAUDE_HOME/skills/one/SKILL.md"
run_sync push
assert_code 0 "machine B push"

enter_home "$TEST_HOME"
run_sync pull
assert_code 0 "machine A pull"
assert_eq "$(cat "$CLAUDE_HOME/skills/one/SKILL.md")" "edited-one"
assert_dir_link "$CLAUDE_HOME/skills/extra"
assert_eq "$(cat "$CLAUDE_HOME/skills/extra/SKILL.md")" "skill-extra"
