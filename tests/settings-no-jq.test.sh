#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# settings=1 without jq: one clear line, no write, and pull and link still finish.
seed_sync_repo
export CLAUDE_SYNC_JQ="$TEST_WORK/no-such-jq"
enable_shared_settings '{"model": "shared"}'
printf '{"model": "local"}\n' > "$CLAUDE_HOME/settings.json"
cp "$CLAUDE_HOME/settings.json" "$TEST_WORK/settings.before"

run_sync pull
assert_code 2 "pull without jq"
assert_err_contains "settings=1 needs jq to merge settings.shared.json"
assert_err_contains "settings.json was not changed"
assert_file_eq "$CLAUDE_HOME/settings.json" "$TEST_WORK/settings.before"
[ ! -d "$CLAUDE_HOME/.claude-sync-backup" ] || fail "a backup was made without jq"
assert_dir_link "$CLAUDE_HOME/skills/alpha"
[ -f "$XDG_STATE_HOME/claude-sync/last-pull" ] || fail "the pull itself was not recorded"

# Once everything is linked, the jq line is the only line.
run_sync pull
assert_code 2 "second pull without jq"
assert_err_lines 1
assert_err_contains "needs jq"

run_sync pull --hook
assert_code 0 "hook pull without jq"
assert_err_lines 1
assert_err_contains "needs jq"
assert_file_eq "$CLAUDE_HOME/settings.json" "$TEST_WORK/settings.before"

# No settings.json at all: still nothing is created.
rm -f "$CLAUDE_HOME/settings.json"
run_sync pull
assert_code 2 "pull without jq and without settings.json"
[ ! -e "$CLAUDE_HOME/settings.json" ] || fail "settings.json was created without jq"
