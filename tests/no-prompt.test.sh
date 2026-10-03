#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

HANG_MARK="$TEST_WORK/hang-mark"
cat > "$TEST_WORK/hang.sh" << EOF
#!/usr/bin/env bash
printf 'started\n' > '$HANG_MARK'
sleep 30
EOF
chmod +x "$TEST_WORK/hang.sh"

bash_bin=$(command -v bash)
script="$TEST_WORK/hang.sh"
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    bash_bin=$(cygpath -m -s "$bash_bin")
    script=$(cygpath -m "$script")
    ;;
esac
# Git refuses ext:: remotes unless the repo allows them. This fixture opts in.
git -C "$CLAUDE_SYNC_DIR" config protocol.ext.allow always
git -C "$CLAUDE_SYNC_DIR" remote set-url origin "ext::${bash_bin} ${script}"
export CLAUDE_SYNC_TIMEOUT=2

start=$(date +%s)
run_sync pull
end=$(date +%s)
assert_code 3 "unreachable remote fails instead of hanging"
elapsed=$((end - start))
if [ "$elapsed" -gt 15 ]; then
  fail "pull hung for ${elapsed}s"
fi
if [ ! -f "$HANG_MARK" ]; then
  fail "git did not start the hanging remote helper"
fi

rm -f "$HANG_MARK"
start=$(date +%s)
run_sync pull --hook
end=$(date +%s)
assert_code 0 "hook mode still exits 0 when the remote hangs"
assert_err_lines 1
elapsed=$((end - start))
if [ "$elapsed" -gt 15 ]; then
  fail "hook pull hung for ${elapsed}s"
fi
