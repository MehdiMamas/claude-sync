#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# With settings=1, settings.shared.json is pushed. settings.json and
# settings.local.json never are, and the secret scan covers the shared file.
seed_sync_repo
printf 'settings=1\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
printf '{"model": "shared"}\n' > "$CLAUDE_SYNC_DIR/settings.shared.json"
printf '{"model": "machine"}\n' > "$CLAUDE_SYNC_DIR/settings.json"
printf '{"x": 1}\n' > "$CLAUDE_SYNC_DIR/settings.local.json"
run_sync push
assert_code 0 "push"
git --git-dir="$TEST_BARE" cat-file -e main:settings.shared.json || fail "settings.shared.json did not reach the remote"
if git --git-dir="$TEST_BARE" cat-file -e main:settings.json 2>/dev/null; then
  fail "settings.json reached the remote"
fi
if git --git-dir="$TEST_BARE" cat-file -e main:settings.local.json 2>/dev/null; then
  fail "settings.local.json reached the remote"
fi
assert_err_contains "left untracked (not on the allowlist)"

# A secret in settings.shared.json blocks the commit and is never printed.
head_before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)
secret="sk-ant-$(repeat_char b 24)"
printf '{\n  "model": "shared",\n  "env": {"ANTHROPIC_API_KEY": "%s"}\n}\n' "$secret" > "$CLAUDE_SYNC_DIR/settings.shared.json"
run_sync push
assert_code 5 "secret in settings.shared.json"
assert_err_contains "settings.shared.json:3:sk-ant"
assert_output_lacks "$secret"
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)" "$head_before" "secret created a commit"

# Turned off again, an edit to settings.shared.json is not committed.
printf '{"model": "shared"}\n' > "$CLAUDE_SYNC_DIR/settings.shared.json"
printf 'settings=0\n' > "$CLAUDE_SYNC_DIR/claude-sync.conf"
run_sync push
assert_code 0 "push with settings=0 commits the conf"
printf '{"model": "changed"}\n' > "$CLAUDE_SYNC_DIR/settings.shared.json"
run_sync push
assert_code 0 "push with settings=0"
assert_eq "$(git --git-dir="$TEST_BARE" show main:settings.shared.json | tr -d '\r')" '{"model": "shared"}' "settings.shared.json changed while off"
