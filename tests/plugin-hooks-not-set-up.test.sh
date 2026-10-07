#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# The exact hook commands from hooks/hooks.json, run the way Claude Code runs
# them, print nothing and exit 0 when claude-sync is not set up.
need_jq
make_work

hook_command() {
  "$TEST_JQ" -r ".hooks.$1[0].hooks[0].command" < "$ROOT/hooks/hooks.json" | tr -d '\r'
}

run_hook() {
  local cmd
  cmd=$(hook_command "$1")
  set +e
  CLAUDE_PLUGIN_ROOT="$ROOT" bash -c "$cmd" >"$TEST_WORK/stdout" 2>"$TEST_WORK/stderr" </dev/null
  RUN_CODE=$?
  set -e
  RUN_OUT=$(<"$TEST_WORK/stdout")
  RUN_ERR=$(<"$TEST_WORK/stderr")
}

[ ! -e "$CLAUDE_SYNC_DIR" ] || fail "the test home already has a repo dir"
run_hook SessionStart
assert_code 0 "SessionStart with no repo"
assert_eq "$RUN_OUT" "" "SessionStart stdout"
assert_eq "$RUN_ERR" "" "SessionStart stderr"

run_hook SessionEnd
assert_code 0 "SessionEnd with no repo"
assert_eq "$RUN_OUT" "" "SessionEnd stdout"
assert_eq "$RUN_ERR" "" "SessionEnd stderr"
log="$XDG_STATE_HOME/claude-sync/log"
grep -q 'repo not ready' "$log" || fail "no 'repo not ready' line in the log"
if grep -q 'push started in the background' "$log"; then
  fail "SessionEnd started a background push with no repo"
fi
[ ! -e "$XDG_STATE_HOME/claude-sync/last-push" ] || fail "a push ran with no repo"
[ ! -e "$XDG_STATE_HOME/claude-sync/last-pull" ] || fail "a pull ran with no repo"
