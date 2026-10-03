#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_empty_bare
url=$(file_url "$TEST_BARE")
bin="$TEST_WORK/fakebin"
mkdir -p "$bin"
cat > "$bin/gh" << EOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "$TEST_WORK/gh-log"
case "\$1 \$2" in
  "repo create")
    case " \$* " in
      *" --public "*|*" --public")
        printf 'refusing public\n' >&2
        exit 9
        ;;
    esac
    printf '%s\n' "$url"
    ;;
  "repo view")
    printf '%s\n' "\${GH_VISIBILITY:-PRIVATE}"
    ;;
  *)
    exit 1
    ;;
esac
EOF
chmod +x "$bin/gh"
export PATH="$bin:$PATH"

mkdir -p "$CLAUDE_HOME/skills/one"
printf 'created\n' > "$CLAUDE_HOME/skills/one/SKILL.md"
run_sync init --create claude-sync-private-test
assert_code 0 "init --create"
log=$(cat "$TEST_WORK/gh-log")
case "$log" in
  *"repo create claude-sync-private-test --private"*) ;;
  *) fail "gh was not called with repo create --private: [$log]" ;;
esac
case "$log" in
  *--public*) fail "gh was asked to create a public repo: [$log]" ;;
esac
assert_eq "$(git -C "$CLAUDE_SYNC_DIR" log -1 --format=%s)" "init: import from $(uname -n)"
assert_eq "$(git --git-dir="$TEST_BARE" log -1 --format=%s)" "init: import from $(uname -n)"
