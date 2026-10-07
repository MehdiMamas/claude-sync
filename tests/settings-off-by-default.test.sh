#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# Without settings=1, settings.shared.json is neither merged nor committed, and jq is not needed.
seed_sync_repo
export CLAUDE_SYNC_JQ="$TEST_WORK/no-such-jq"
printf '{"localOnly": true}\n' > "$CLAUDE_HOME/settings.json"
cp "$CLAUDE_HOME/settings.json" "$TEST_WORK/settings.before"
printf '{"model": "shared"}\n' > "$CLAUDE_SYNC_DIR/settings.shared.json"

check_off() {
  local label="$1"
  run_sync pull
  assert_code 0 "pull $label"
  assert_file_eq "$CLAUDE_HOME/settings.json" "$TEST_WORK/settings.before"
  case "$RUN_ERR" in
    *jq*|*settings*) fail "pull mentioned settings while off ($label): [$RUN_ERR]" ;;
  esac
  [ ! -d "$CLAUDE_HOME/.claude-sync-backup" ] || fail "a backup was made while off ($label)"
  run_sync push
  assert_code 0 "push $label"
  if git -C "$CLAUDE_SYNC_DIR" ls-files --error-unmatch settings.shared.json >/dev/null 2>&1; then
    fail "settings.shared.json was committed while off ($label)"
  fi
}

check_off "with no conf"
printf '# off\nsettings=0\ncursor=0\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
git -C "$CLAUDE_SYNC_DIR" add claude-sync.conf
git -C "$CLAUDE_SYNC_DIR" commit -qm conf
check_off "with settings=0"
printf 'settings = 2\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
check_off "with settings=2"
