#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")
seed="$TEST_WORK/seed"
git init -b main "$seed" >/dev/null
harden_repo "$seed"
mkdir -p "$seed/skills/alpha"
printf 'alpha\n' > "$seed/skills/alpha/SKILL.md"
git -C "$seed" add skills
git -C "$seed" commit -m seed >/dev/null
git -C "$seed" remote add origin "$url"
git -C "$seed" push -u origin main >/dev/null 2>&1

mkdir -p "$CLAUDE_HOME/skills/beta"
printf 'beta\n' > "$CLAUDE_HOME/skills/beta/SKILL.md"
run_sync join --merge "$url"
assert_code 0 "join --merge does not require template files"
assert_eq "$(cat "$CLAUDE_SYNC_DIR/skills/beta/SKILL.md")" "beta"
assert_eq "$(cat "$CLAUDE_HOME/skills/alpha/SKILL.md")" "alpha"
assert_dir_link "$CLAUDE_HOME/skills/beta"
assert_out_contains "pushed: yes"
