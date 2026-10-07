#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

need_jq
seed_sync_repo
s="$CLAUDE_HOME/settings.json"
cat > "$s" <<'JSON'
{
  "model": "local-model",
  "localOnly": {"keep": true, "path": "C:\\Users\\me"},
  "permissions": {"allow": ["Bash(ls)", "Read"], "deny": ["WebFetch"]},
  "env": {"A": "1"},
  "theme": "dark"
}
JSON
cp "$s" "$TEST_WORK/settings.before"
enable_shared_settings '{"model": "shared-model", "permissions": {"allow": ["Read", "Bash(git status)", "Read"]}, "env": {"B": "2"}, "newKey": "v", "nested": {"deep": {"x": 1}}, "note": "caf\u00e9"}'

run_sync pull
assert_code 0 "pull merges"
assert_err_contains "merged settings.shared.json into settings.json"
assert_eq "$(json_get "$s" '.newKey')" '"v"' "new key"
assert_eq "$(json_get "$s" '.nested')" '{"deep":{"x":1}}' "new nested object"
assert_eq "$(json_get "$s" '.env')" '{"A":"1","B":"2"}' "nested objects merge"
assert_eq "$(json_get "$s" '.permissions.allow')" '["Bash(ls)","Read","Bash(git status)"]' "array union without duplicates"
assert_eq "$(json_get "$s" '.permissions.deny')" '["WebFetch"]' "local nested key kept"
assert_eq "$(json_get "$s" '.model')" '"shared-model"' "shared wins a scalar conflict"
assert_eq "$(json_get "$s" '.localOnly')" '{"keep":true,"path":"C:\\Users\\me"}' "local-only key kept"
assert_eq "$(json_get "$s" '.theme')" '"dark"' "local-only scalar kept"
assert_eq "$(json_get "$s" '.note | explode')" '[99,97,102,233]' "non-ASCII text survives"
assert_eq "$(json_get "$s" 'keys_unsorted | .[0:5]')" '["model","localOnly","permissions","env","theme"]' "local key order kept"
cr=$(printf '\r')
if grep -q "$cr" "$s"; then
  fail "settings.json was written with CRLF"
fi
assert_eq "$(count_backups)" "1" "one backup"
for b in "$CLAUDE_HOME/.claude-sync-backup"/*/settings.json; do
  assert_file_eq "$b" "$TEST_WORK/settings.before"
done
assert_dir_link "$CLAUDE_HOME/skills/alpha"

# Nothing new to merge: no write and no new backup.
cp "$s" "$TEST_WORK/settings.merged"
run_sync pull
assert_code 0 "second pull"
assert_file_eq "$s" "$TEST_WORK/settings.merged"
assert_eq "$(count_backups)" "1" "no backup without a change"
case "$RUN_ERR" in
  *merged*) fail "second pull reported a merge: [$RUN_ERR]" ;;
esac

# Hook mode: one line at most, exit 0, same merge.
printf '%s\n' '{"model": "hook-model"}' > "$CLAUDE_SYNC_DIR/settings.shared.json"
git -C "$CLAUDE_SYNC_DIR" commit -qam "hook model"
run_sync pull --hook
assert_code 0 "hook pull"
assert_err_at_most 1
assert_eq "$(json_get "$s" '.model')" '"hook-model"' "hook pull merged"
assert_eq "$(json_get "$s" '.env')" '{"A":"1","B":"2"}' "hook pull kept earlier keys"

# No settings.json yet: the shared keys create it.
rm -f "$s"
run_sync pull
assert_code 0 "pull with no settings.json"
assert_err_contains "created settings.json from settings.shared.json"
assert_eq "$(json_get "$s" '.')" '{"model":"hook-model"}' "created from shared"

# settings=1 with no settings.shared.json in the repo does nothing.
git -C "$CLAUDE_SYNC_DIR" rm -q settings.shared.json
git -C "$CLAUDE_SYNC_DIR" commit -qm "drop shared"
cp "$s" "$TEST_WORK/settings.created"
run_sync pull
assert_code 0 "pull without settings.shared.json"
assert_file_eq "$s" "$TEST_WORK/settings.created"
