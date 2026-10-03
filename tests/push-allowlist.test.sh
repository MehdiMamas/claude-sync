#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

printf 'instructions\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md"
mkdir -p "$CLAUDE_SYNC_DIR/skills/my skill" "$CLAUDE_SYNC_DIR/agents" "$CLAUDE_SYNC_DIR/commands" "$CLAUDE_SYNC_DIR/output-styles"
printf 'skill\n' > "$CLAUDE_SYNC_DIR/skills/my skill/SKILL.md"
printf 'agent\n' > "$CLAUDE_SYNC_DIR/agents/helper.md"
printf 'cmd\n' > "$CLAUDE_SYNC_DIR/commands/ship.md"
printf 'style\n' > "$CLAUDE_SYNC_DIR/output-styles/plain.md"
printf 'note\n' > "$CLAUDE_SYNC_DIR/notes.txt"
printf 'nope\n' > "$CLAUDE_SYNC_DIR/CLAUDE.md.bak"
mkdir -p "$CLAUDE_SYNC_DIR/not-skills"
printf 'nope\n' > "$CLAUDE_SYNC_DIR/not-skills/x.md"
printf 'changed\n' >> "$CLAUDE_SYNC_DIR/README.md"

run_sync push
assert_code 0
assert_err_contains "notes.txt"
assert_err_contains "CLAUDE.md.bak"

names=$(git -C "$CLAUDE_SYNC_DIR" show --pretty=format: --name-only HEAD)
case "$names" in
  *CLAUDE.md*) ;;
  *) fail "commit missed CLAUDE.md: [$names]" ;;
esac
case "$names" in
  *skills/my\ skill/SKILL.md*) ;;
  *) fail "commit missed the spaced skill path: [$names]" ;;
esac
case "$names" in
  *agents/helper.md*) ;;
  *) fail "commit missed agents: [$names]" ;;
esac
case "$names" in
  *commands/ship.md*) ;;
  *) fail "commit missed commands: [$names]" ;;
esac
case "$names" in
  *output-styles/plain.md*) ;;
  *) fail "commit missed output-styles: [$names]" ;;
esac
case "$names" in
  *notes.txt*|*CLAUDE.md.bak*|*README.md*|*not-skills*) fail "commit included a non-allowlisted path: [$names]" ;;
esac

status=$(git -C "$CLAUDE_SYNC_DIR" status --porcelain)
case "$status" in
  *notes.txt*) ;;
  *) fail "notes.txt was not left untracked: [$status]" ;;
esac
case "$status" in
  *README.md*) ;;
  *) fail "README.md change was committed or lost: [$status]" ;;
esac

rm -f "$CLAUDE_SYNC_DIR/agents/helper.md"
run_sync push
assert_code 0
if git -C "$CLAUDE_SYNC_DIR" ls-files --error-unmatch -- agents/helper.md >/dev/null 2>&1; then
  fail "deleted allowlisted file is still tracked"
fi
case "$(git -C "$CLAUDE_SYNC_DIR" status --porcelain)" in
  *notes.txt*) ;;
  *) fail "notes.txt was picked up by the second push" ;;
esac
