#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# With a repo set up, the SessionStart hook pulls and links with empty stdout
# (Claude Code adds hook stdout to the session), and the SessionEnd hook returns
# before the push finishes: the push runs on in the background, ignores SIGHUP,
# and reaches the remote.
need_jq
seed_sync_repo
git -C "$CLAUDE_SYNC_DIR" push -q origin main 2>/dev/null
state="$XDG_STATE_HOME/claude-sync"

hook_command() {
  "$TEST_JQ" -r ".hooks.$1[0].hooks[0].command" < "$ROOT/hooks/hooks.json" | tr -d '\r'
}

run_hook() {
  local cmd
  cmd=$(hook_command "$1")
  set +e
  CLAUDE_PLUGIN_ROOT="$ROOT" bash -c "$cmd" >"$TEST_WORK/stdout" 2>"$TEST_WORK/stderr" </dev/null
  RUN_CODE=$?
  set -e
  RUN_OUT=$(<"$TEST_WORK/stdout")
  RUN_ERR=$(<"$TEST_WORK/stderr")
}

wait_for() {
  local i=0
  while [ "$i" -lt 120 ]; do
    if [ -e "$1" ]; then
      return 0
    fi
    sleep 0.5
    i=$((i + 1))
  done
  return 1
}

run_hook SessionStart
assert_code 0 "SessionStart"
assert_eq "$RUN_OUT" "" "SessionStart stdout"
# Hook mode prints at most one stderr line; Claude Code shows it only in the transcript.
assert_err_at_most 1
[ -f "$state/last-pull" ] || fail "SessionStart did not write last-pull"
assert_dir_link "$CLAUDE_HOME/skills/alpha"

# Edit a synced skill through its link, the way a session would.
printf 'edited in a session\n' >> "$CLAUDE_HOME/skills/alpha/SKILL.md"

# A git that takes 3 s to push, so the push is still running when the hook returns.
real_git=$(command -v git)
mkdir -p "$TEST_WORK/slow-bin"
cat > "$TEST_WORK/slow-bin/git" <<SLOW
#!/usr/bin/env bash
for a in "\$@"; do
  if [ "\$a" = push ]; then
    sleep 3
    break
  fi
done
exec "$real_git" "\$@"
SLOW
chmod +x "$TEST_WORK/slow-bin/git"

PATH="$TEST_WORK/slow-bin:$PATH" run_hook SessionEnd
assert_code 0 "SessionEnd"
assert_eq "$RUN_OUT" "" "SessionEnd stdout"
assert_eq "$RUN_ERR" "" "SessionEnd stderr"
[ ! -e "$state/last-push" ] || fail "SessionEnd waited for the push to finish"
pid=$(sed -n 's/.*push started in the background (pid \([0-9]*\)).*/\1/p' "$state/log" | tail -n 1)
[ -n "$pid" ] || fail "no background push pid in the log"
# A closing terminal sends SIGHUP; the detached push ignores it.
kill -HUP "$pid" 2>/dev/null || true

wait_for "$state/last-push" || fail "the background push never finished"
i=0
while [ -d "$state/lock" ] && [ "$i" -lt 60 ]; do
  sleep 0.5
  i=$((i + 1))
done
[ ! -d "$state/lock" ] || fail "the background push left the lock"
subject=$(git --git-dir="$TEST_BARE" log -1 --format=%s main)
case "$subject" in
  sync:*) ;;
  *) fail "the remote's last commit is not a sync commit: [$subject]" ;;
esac
git --git-dir="$TEST_BARE" show main:skills/alpha/SKILL.md | grep -q 'edited in a session' \
  || fail "the edit did not reach the remote"
