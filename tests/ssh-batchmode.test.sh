#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

prepare_bare
clone_here

bin="$TEST_WORK/fakebin"
mkdir -p "$bin"
cat > "$bin/ssh" << EOF
#!/usr/bin/env bash
printf '%s\n' "args=\$*" > '$TEST_WORK/ssh-record'
printf '%s\n' "prompt=\${GIT_TERMINAL_PROMPT-unset}" >> '$TEST_WORK/ssh-record'
printf '%s\n' "gcm=\${GCM_INTERACTIVE-unset}" >> '$TEST_WORK/ssh-record'
exit 255
EOF
chmod +x "$bin/ssh"
export PATH="$bin:$PATH"
git -C "$CLAUDE_SYNC_DIR" remote set-url origin "ssh://git@127.0.0.1/repo.git"
unset GIT_SSH_COMMAND
unset GIT_TERMINAL_PROMPT
unset GCM_INTERACTIVE

run_sync pull
assert_code 3 "ssh remote fails instead of prompting"
record=$(cat "$TEST_WORK/ssh-record")
case "$record" in
  *'BatchMode=yes'*) ;;
  *) fail "ssh was not passed BatchMode=yes: [$record]" ;;
esac
case "$record" in
  *'prompt=0'*) ;;
  *) fail "GIT_TERMINAL_PROMPT was not set for the ssh child: [$record]" ;;
esac
case "$record" in
  *'gcm=unset'*) ;;
  *) fail "CLI mode set GCM_INTERACTIVE: [$record]" ;;
esac

rm -f "$TEST_WORK/ssh-record"
run_sync pull --hook
assert_code 0 "hook ssh failure exits 0"
assert_err_at_most 1
record=$(cat "$TEST_WORK/ssh-record")
case "$record" in
  *'gcm=never'*) ;;
  *) fail "hook mode did not set GCM_INTERACTIVE=never: [$record]" ;;
esac

cat > "$bin/ssh-custom" << EOF
#!/usr/bin/env bash
printf '%s\n' "custom" > '$TEST_WORK/ssh-record'
printf '%s\n' "args=\$*" >> '$TEST_WORK/ssh-record'
exit 255
EOF
chmod +x "$bin/ssh-custom"
export GIT_SSH_COMMAND="$bin/ssh-custom -o BatchMode=no"
run_sync pull
assert_code 3 "user GIT_SSH_COMMAND is used"
record=$(cat "$TEST_WORK/ssh-record")
case "$record" in
  custom*) ;;
  *) fail "user GIT_SSH_COMMAND was replaced: [$record]" ;;
esac
case "$record" in
  *'BatchMode=yes'*) fail "user GIT_SSH_COMMAND gained BatchMode=yes: [$record]" ;;
esac
