#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")

mkdir -p "$CLAUDE_HOME/skills/one" "$CLAUDE_HOME/skills/two" \
  "$CLAUDE_HOME/skills/synced" "$CLAUDE_HOME/agents" \
  "$CLAUDE_HOME/commands" "$CLAUDE_HOME/output-styles" \
  "$CLAUDE_HOME/projects/demo"
printf 'instructions\n' > "$CLAUDE_HOME/CLAUDE.md"
printf 'one\n' > "$CLAUDE_HOME/skills/one/SKILL.md"
printf 'two\n' > "$CLAUDE_HOME/skills/two/SKILL.md"
printf 'stay\n' > "$CLAUDE_HOME/skills/synced/SKILL.md"
printf 'agent\n' > "$CLAUDE_HOME/agents/a.md"
printf 'command\n' > "$CLAUDE_HOME/commands/c.md"
printf 'style\n' > "$CLAUDE_HOME/output-styles/s.md"
printf 'settings\n' > "$CLAUDE_HOME/settings.json"
printf 'secret\n' > "$CLAUDE_HOME/.credentials.json"
printf 'project\n' > "$CLAUDE_HOME/projects/demo/note.txt"

run_sync init --remote "$url"
assert_code 0 "init imports allowlisted items"
assert_out_contains "imported:"
assert_out_contains "CLAUDE.md"
assert_out_contains "skills/one"
assert_out_contains "skills/two"
assert_out_contains "agents"
assert_out_contains "pushed: yes"

backup=$(printf '%s\n' "$RUN_OUT" | sed -n 's/^backed up: //p')
[ -n "$backup" ] || fail "summary did not name the backup"
[ -d "$backup" ] || fail "backup dir missing: $backup"

printf 'instructions\n' > "$TEST_WORK/expect-claude"
printf 'one\n' > "$TEST_WORK/expect-one"
assert_file_eq "$backup/CLAUDE.md" "$TEST_WORK/expect-claude"
assert_file_eq "$backup/skills/one/SKILL.md" "$TEST_WORK/expect-one"
assert_file_eq "$backup/skills/two/SKILL.md" "$CLAUDE_SYNC_DIR/skills/two/SKILL.md"
assert_file_eq "$backup/agents/a.md" "$CLAUDE_SYNC_DIR/agents/a.md"
assert_file_eq "$backup/commands/c.md" "$CLAUDE_SYNC_DIR/commands/c.md"
assert_file_eq "$backup/output-styles/s.md" "$CLAUDE_SYNC_DIR/output-styles/s.md"

assert_eq "$(cat "$CLAUDE_SYNC_DIR/CLAUDE.md")" "instructions"
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" log -1 --format=%s)" "init: import from $(uname -n)"
assert_dir_link "$CLAUDE_HOME/skills/one"
assert_dir_link "$CLAUDE_HOME/agents"

assert_eq "$(cat "$CLAUDE_HOME/skills/synced/SKILL.md")" "stay"
assert_eq "$(cat "$CLAUDE_HOME/settings.json")" "settings"
assert_eq "$(cat "$CLAUDE_HOME/.credentials.json")" "secret"
assert_eq "$(cat "$CLAUDE_HOME/projects/demo/note.txt")" "project"

tracked=$(git -C "$CLAUDE_SYNC_DIR" ls-files)
case "$tracked" in
  *settings.json*|*.credentials.json*|*projects/*|*skills/synced/*)
    fail "never-sync path was committed: $tracked"
    ;;
esac
if find "$backup" -name settings.json -o -name '.credentials.json' -o -name note.txt | grep -q .; then
  fail "never-sync path was copied into the backup"
fi
[ ! -e "$backup/skills/synced" ] || fail "skills/synced was backed up"
[ ! -e "$CLAUDE_SYNC_DIR/skills/synced" ] || fail "skills/synced was copied into the repo"

head_before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)
run_sync init --remote "$url"
assert_code 2 "init twice"
assert_err_contains "Nothing was changed"
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)" "$head_before" "second init changed the repo"
assert_eq "$(cat "$CLAUDE_HOME/settings.json")" "settings"
