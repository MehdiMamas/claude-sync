#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_work

run_sync --help
assert_code 0
assert_out_contains "pull"
assert_out_contains "push"
assert_out_contains "--version"
case "$RUN_OUT$RUN_ERR" in
  *link*|*init*|*doctor*|*uninstall*|*status*) fail "help lists a later-phase command" ;;
esac

run_sync --version
assert_code 0
assert_eq "$RUN_OUT" "claude-sync 0.0.0"

run_sync
assert_code 1
assert_err_contains "Usage:"

run_sync nosuch
assert_code 1
assert_err_contains "Usage:"

run_sync pull extra
assert_code 1
