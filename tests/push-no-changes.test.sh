#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here
before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)

run_sync push
assert_code 0
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)" "$before" "push with nothing allowlisted made a commit"
assert_err_lines 0
