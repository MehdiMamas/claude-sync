#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

need_jq
seed_sync_repo
enable_shared_settings '{"model": "shared"}'
s="$CLAUDE_HOME/settings.json"

# settings.json stays byte-identical, no backup is made, and link still runs.
expect_untouched() {
  local label="$1" want="$2"
  cp "$s" "$TEST_WORK/settings.before"
  run_sync pull
  assert_code 2 "$label"
  assert_err_contains "$want"
  assert_err_contains "settings.json was not changed"
  assert_file_eq "$s" "$TEST_WORK/settings.before"
  [ ! -d "$CLAUDE_HOME/.claude-sync-backup" ] || fail "$label made a backup"
  [ ! -e "$s.claude-sync-new" ] || fail "$label left a temp file"
  assert_dir_link "$CLAUDE_HOME/skills/alpha"
  run_sync pull --hook
  assert_code 0 "$label in hook mode"
  assert_err_lines 1
  assert_file_eq "$s" "$TEST_WORK/settings.before"
}

printf '{"model": "local",\r\n' > "$s"
expect_untouched "truncated settings.json" "settings.json is not one valid JSON object"
printf '{"a": 1}\n{"b": 2}\n' > "$s"
expect_untouched "two values in settings.json" "settings.json is not one valid JSON object"
printf '["model"]\n' > "$s"
expect_untouched "settings.json is an array" "settings.json is not one valid JSON object"
: > "$s"
expect_untouched "empty settings.json" "settings.json is not one valid JSON object"

printf '{"model": "local", "permissions": {"allow": ["Read"]}}\n' > "$s"
printf '{"model": \n' > "$CLAUDE_SYNC_DIR/settings.shared.json"
git -C "$CLAUDE_SYNC_DIR" commit -qam "broken shared"
expect_untouched "invalid settings.shared.json" "settings.shared.json is not one valid JSON object"
printf '"model"\n' > "$CLAUDE_SYNC_DIR/settings.shared.json"
git -C "$CLAUDE_SYNC_DIR" commit -qam "string shared"
expect_untouched "settings.shared.json is a string" "settings.shared.json is not one valid JSON object"

# A local object or array is never replaced by a shared value of another type.
printf '{"permissions": "Read"}\n' > "$CLAUDE_SYNC_DIR/settings.shared.json"
git -C "$CLAUDE_SYNC_DIR" commit -qam "type clash object"
expect_untouched "shared string over a local object" "for permissions,"
printf '{"permissions": {"allow": {"Read": true}}}\n' > "$CLAUDE_SYNC_DIR/settings.shared.json"
git -C "$CLAUDE_SYNC_DIR" commit -qam "type clash array"
expect_untouched "shared object over a local array" "for permissions.allow,"

# A local scalar may become an object: that is a scalar conflict, and shared wins.
printf '{"model": {"name": "m"}}\n' > "$CLAUDE_SYNC_DIR/settings.shared.json"
git -C "$CLAUDE_SYNC_DIR" commit -qam "object over scalar"
run_sync pull
assert_code 0 "shared object over a local scalar"
assert_eq "$(json_get "$s" '.')" '{"model":{"name":"m"},"permissions":{"allow":["Read"]}}' "object replaced the scalar"

# A settings.json that is a directory is left alone.
rm -f "$s"
mkdir "$s"
run_sync pull
assert_code 2 "settings.json is a directory"
assert_err_contains "settings.json is a link or not a regular file"
[ -d "$s" ] || fail "settings.json directory was replaced"
