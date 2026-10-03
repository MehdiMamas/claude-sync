#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")

mkdir -p "$CLAUDE_HOME/skills/one"
printf 'from-a\n' > "$CLAUDE_HOME/CLAUDE.md"
printf 'skill-a\n' > "$CLAUDE_HOME/skills/one/SKILL.md"
run_sync init --remote "$url"
assert_code 0 "init before replace"

make_other_home
enter_home "$TEST_HOME_B"
mkdir -p "$CLAUDE_HOME/skills/one" "$CLAUDE_HOME/skills/extra"
printf 'from-b\n' > "$CLAUDE_HOME/CLAUDE.md"
printf 'skill-b\n' > "$CLAUDE_HOME/skills/one/SKILL.md"
printf 'only-b\n' > "$CLAUDE_HOME/skills/extra/SKILL.md"
cp "$CLAUDE_HOME/CLAUDE.md" "$TEST_WORK/b-claude"
cp "$CLAUDE_HOME/skills/one/SKILL.md" "$TEST_WORK/b-one"
cp "$CLAUDE_HOME/skills/extra/SKILL.md" "$TEST_WORK/b-extra"

run_sync join --replace "$url"
assert_code 0 "join --replace"
backup=$(printf '%s\n' "$RUN_OUT" | sed -n 's/^backed up: //p')
[ -n "$backup" ] || fail "replace did not name a backup"
assert_file_eq "$backup/CLAUDE.md" "$TEST_WORK/b-claude"
assert_file_eq "$backup/skills/one/SKILL.md" "$TEST_WORK/b-one"
assert_file_eq "$backup/skills/extra/SKILL.md" "$TEST_WORK/b-extra"
assert_eq "$(cat "$CLAUDE_SYNC_DIR/CLAUDE.md")" "from-a"
assert_dir_link "$CLAUDE_HOME/skills/one"
assert_eq "$(cat "$CLAUDE_HOME/skills/one/SKILL.md")" "skill-a"
[ ! -e "$CLAUDE_HOME/skills/extra" ] || fail "replace kept a local-only skill"
case "$(git -C "$CLAUDE_SYNC_DIR" ls-files)" in
  *skills/extra/*) fail "replace committed the local-only skill" ;;
esac
