#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")
mkdir -p "$CLAUDE_HOME/skills/one" "$CLAUDE_HOME/skills/two"
printf 'one\n' > "$CLAUDE_HOME/skills/one/SKILL.md"
printf 'two\n' > "$CLAUDE_HOME/skills/two/SKILL.md"
run_sync init --remote "$url"
assert_code 0 "init two skills"
assert_dir_link "$CLAUDE_HOME/skills/two"

make_other_home
enter_home "$TEST_HOME_B"
run_sync join "$url"
assert_code 0 "join on an empty machine"
assert_dir_link "$CLAUDE_HOME/skills/two"

rm -rf "$CLAUDE_SYNC_DIR/skills/two"
run_sync push
assert_code 0 "push skill deletion"
case "$(git -C "$CLAUDE_SYNC_DIR" ls-files)" in
  *skills/two/*) fail "deleted skill is still tracked" ;;
esac

enter_home "$TEST_HOME"
run_sync pull
assert_code 0 "pull removes the dangling skill link"
if [ -e "$CLAUDE_HOME/skills/two" ] || [ -L "$CLAUDE_HOME/skills/two" ]; then
  fail "dangling skill link was left in place"
fi
assert_dir_link "$CLAUDE_HOME/skills/one"
assert_eq "$(cat "$CLAUDE_HOME/skills/one/SKILL.md")" "one"
