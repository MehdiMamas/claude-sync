#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

# A secret that is not on the allowlist must not be committed, and must not block
# an allowlisted change that is clean.
printf 'ok\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
outside="sk-ant-$(repeat_char a 20)"
printf '%s\n' "$outside" > "$CLAUDE_SYNC_DIR/notes.txt"
run_sync push
assert_code 0 "untracked secret outside the allowlist"
assert_output_lacks "$outside"
case "$(git -C "$CLAUDE_SYNC_DIR" status --porcelain)" in
  *notes.txt*) ;;
  *) fail "notes.txt with a secret was committed" ;;
esac
head_before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)

block_one() {
  local secret="$1" name="$2" line_no="$3"
  printf '%s\n' "$secret" > "$CLAUDE_SYNC_DIR/CLAUDE.md"
  if [ "$line_no" -eq 3 ]; then
    printf 'one\ntwo\n%s\n' "$secret" > "$CLAUDE_SYNC_DIR/CLAUDE.md"
  fi
  run_sync push
  assert_code 5 "$name"
  assert_err_contains "CLAUDE.md:${line_no}:${name}"
  assert_output_lacks "$secret"
  git -C "$CLAUDE_SYNC_DIR" diff --cached --quiet || fail "$name left a staged diff"
  assert_eq "$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD)" "$head_before" "$name created a commit"
}

block_one "sk-ant-$(repeat_char a 20)" sk-ant 3
block_one "ghp_$(repeat_char a 36)" ghp 1
block_one "github_pat_$(repeat_char a 50)" github_pat 1
block_one "gho_$(repeat_char a 36)" gh-token 1
block_one "AKIA$(repeat_char A 16)" aws-access-key 1
block_one "xoxb-$(repeat_char a 10)" slack-token 1
block_one "-----BEGIN RSA PRIVATE KEY-----" private-key 1
block_one '"access_token":' token-field 1
