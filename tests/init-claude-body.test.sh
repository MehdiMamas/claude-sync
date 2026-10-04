#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_work
printf '%s\n' \
  '<!-- claude-sync:begin (managed, do not edit inside) -->' \
  '@/tmp/old/CLAUDE.md' \
  '<!-- claude-sync:end -->' \
  'outside the block' > "$CLAUDE_HOME/CLAUDE.md"
run_sync init
assert_code 0 "init keeps only the body outside the managed block"
assert_eq "$(cat "$CLAUDE_SYNC_DIR/CLAUDE.md")" "outside the block"
case "$(cat "$CLAUDE_SYNC_DIR/CLAUDE.md")" in
  *claude-sync:begin*) fail "repo CLAUDE.md kept the managed block" ;;
esac
