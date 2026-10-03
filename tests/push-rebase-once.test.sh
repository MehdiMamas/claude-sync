#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here
add_second_home

enter_home "$TEST_HOME"
printf 'from-a\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 0 "first clone push"

enter_home "$TEST_HOME_B"
mkdir -p "$CLAUDE_SYNC_DIR/skills/demo"
printf 'from-b\n' > "$CLAUDE_SYNC_DIR/skills/demo/SKILL.md"
run_sync push
assert_code 0 "second clone rebases once and pushes"
assert_no_rebase "$CLAUDE_SYNC_DIR"
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)" "$(git -C "$CLAUDE_SYNC_DIR" rev-parse '@{u}')" "branch matches upstream after retry"
assert_eq "$(git --git-dir="$TEST_BARE" show main:CLAUDE.md)" "from-a"
assert_eq "$(git --git-dir="$TEST_BARE" show "main:skills/demo/SKILL.md")" "from-b"
