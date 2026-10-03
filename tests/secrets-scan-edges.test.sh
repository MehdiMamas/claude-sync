#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

token() {
  printf 'ghp_%s' "$(repeat_char a 36)"
}

block_with_config() {
  local key="$1" value="$2" secret
  secret=$(token)
  git -C "$CLAUDE_SYNC_DIR" config "$key" "$value"
  printf '%s\n' "$secret" > "$CLAUDE_SYNC_DIR/CLAUDE.md"
  run_sync push
  assert_code 5 "$key=$value still scans"
  assert_err_contains "CLAUDE.md:1:ghp"
  assert_output_lacks "$secret"
  git -C "$CLAUDE_SYNC_DIR" diff --cached --quiet || fail "$key left a staged diff"
  git -C "$CLAUDE_SYNC_DIR" config --unset "$key"
}

block_with_config diff.noprefix true
block_with_config diff.mnemonicPrefix true

printf 'ok\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
secret=$(token)
mkdir -p "$CLAUDE_SYNC_DIR/skills/my skill"
printf '%s\n' "$secret" > "$CLAUDE_SYNC_DIR/skills/my skill/SKILL.md"
run_sync push
assert_code 5 "secret in a path with a space"
assert_err_contains "skills/my skill/SKILL.md:1:ghp"
assert_output_lacks "$secret"
case "$RUN_ERR" in
  *$'SKILL.md\t'*) fail "reported path kept a trailing tab" ;;
esac
rm -rf "$CLAUDE_SYNC_DIR/skills"

other="sk-ant-$(repeat_char b 20)"
secret=$(token)
printf '%s\n%s\n' "$secret" "$other" > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push
assert_code 5 "every hit is printed"
assert_err_contains "CLAUDE.md:1:ghp"
assert_err_contains "CLAUDE.md:2:sk-ant"
assert_output_lacks "$secret"
assert_output_lacks "$other"

printf '%s\n%s\n' "$secret" "$other" > "$CLAUDE_SYNC_DIR/CLAUDE.md"
run_sync push --hook
assert_code 0 "hook still exits 0 when several secrets match"
assert_err_lines 1
assert_err_contains "CLAUDE.md:1:ghp"
case "$RUN_ERR" in
  *CLAUDE.md:2:sk-ant*) fail "hook mode printed more than the first hit" ;;
esac
log=$(cat "$XDG_STATE_HOME/claude-sync/log")
case "$log" in
  *CLAUDE.md:2:sk-ant*) ;;
  *) fail "hook log is missing the second hit" ;;
esac
assert_output_lacks "$secret"
