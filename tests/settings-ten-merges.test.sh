#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# Machine B changes settings.shared.json ten times and pushes it with claude-sync.
# Each time, machine A pulls, and its local-only key comes through unchanged.
need_jq
seed_sync_repo
enable_shared_settings '{"model": "m0"}'
run_sync push
assert_code 0 "machine A push conf and shared settings"
git --git-dir="$TEST_BARE" cat-file -e main:settings.shared.json || fail "shared settings not pushed"
s="$CLAUDE_HOME/settings.json"
printf '{"localOnly": {"machine": "A", "list": [1, 2]}, "permissions": {"allow": ["Local"]}}\n' > "$s"
add_second_home

i=1
while [ "$i" -le 10 ]; do
  enter_home "$TEST_HOME_B"
  run_sync pull
  assert_code 0 "machine B pull $i"
  printf '{"model": "m%s", "permissions": {"allow": ["S%s"]}, "env": {"K%s": "%s"}}\n' "$i" "$i" "$i" "$i" > "$CLAUDE_SYNC_DIR/settings.shared.json"
  run_sync push
  assert_code 0 "machine B push $i"
  enter_home "$TEST_HOME"
  run_sync pull
  assert_code 0 "machine A pull $i"
  assert_eq "$(json_get "$s" '.localOnly')" '{"machine":"A","list":[1,2]}' "local-only key after merge $i"
  assert_eq "$(json_get "$s" '.model')" "\"m$i\"" "shared model after merge $i"
  assert_eq "$(json_get "$s" ".permissions.allow | index(\"S$i\") != null")" "true" "shared allow after merge $i"
  assert_eq "$(json_get "$s" '.permissions.allow[0]')" '"Local"' "local allow after merge $i"
  i=$((i + 1))
done

assert_eq "$(json_get "$s" '.permissions.allow | length')" "11" "Local plus S1..S10"
assert_eq "$(json_get "$s" '.env | keys | length')" "10" "every env key from the ten merges"
assert_eq "$(count_backups)" "10" "one backup per merge"
