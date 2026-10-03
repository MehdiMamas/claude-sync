#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_work

run_sync pull --hook
assert_code 0 "missing repo in hook mode"
assert_err_lines 0
assert_eq "$RUN_OUT" "" "hook mode printed stdout"

run_sync pull
assert_code 2 "missing repo in cli mode"
assert_err_contains "no repo"

run_sync push --hook
assert_code 0 "missing repo push in hook mode"
assert_err_lines 0

run_sync push
assert_code 2 "missing repo push in cli mode"
