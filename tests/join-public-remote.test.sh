#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")
mkdir -p "$CLAUDE_HOME/skills/one"
printf 'private-body\n' > "$CLAUDE_HOME/CLAUDE.md"
printf 'one\n' > "$CLAUDE_HOME/skills/one/SKILL.md"

bin="$TEST_WORK/fakebin"
mkdir -p "$bin"
cat > "$bin/gh" << EOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "$TEST_WORK/gh-log"
case "\$1 \$2" in
  "repo view")
    printf '%s\n' "PUBLIC"
    ;;
  *)
    exit 1
    ;;
esac
EOF
chmod +x "$bin/gh"
export PATH="$bin:$PATH"

run_sync init --remote "$url"
assert_code 0 "init --remote warns when public"
assert_err_contains "public"

add_second_home
enter_home "$TEST_HOME_B"
: > "$TEST_WORK/gh-log"
run_sync join "$url"
assert_code 0 "join warns when public"
assert_err_contains "public"
assert_dir_link "$CLAUDE_HOME/skills/one"
case "$(cat "$TEST_WORK/gh-log")" in
  *"repo view"*) ;;
  *) fail "join did not ask gh about visibility: [$(cat "$TEST_WORK/gh-log")]" ;;
esac
