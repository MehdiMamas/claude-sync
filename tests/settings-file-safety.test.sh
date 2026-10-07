#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# How the merge treats the settings.json file itself: links, a missing Claude dir,
# the file mode, a failed backup, and a pull stopped by a signal mid-merge.
need_jq
seed_sync_repo
enable_shared_settings '{"model": "shared"}'
s="$CLAUDE_HOME/settings.json"

is_windows() {
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) return 0 ;;
  esac
  return 1
}

# A link, live or dangling, is never replaced and never followed.
if is_windows; then
  echo "skip: symlink settings.json cases (Git Bash ln -s copies instead of linking without Developer Mode)"
else
  printf '{"model": "target"}\n' > "$TEST_WORK/real-settings.json"
  cp "$TEST_WORK/real-settings.json" "$TEST_WORK/real-settings.before"
  ln -s "$TEST_WORK/real-settings.json" "$s"
  run_sync pull
  assert_code 2 "live link"
  assert_err_contains "settings.json is a link or not a regular file"
  [ -L "$s" ] || fail "live link was replaced"
  assert_eq "$(readlink "$s")" "$TEST_WORK/real-settings.json" "live link target"
  assert_file_eq "$TEST_WORK/real-settings.json" "$TEST_WORK/real-settings.before"
  [ ! -d "$CLAUDE_HOME/.claude-sync-backup" ] || fail "live link made a backup"
  [ ! -e "$s.claude-sync-new" ] || fail "live link left a temp file"
  rm -f "$s"

  ln -s "$TEST_WORK/not-there.json" "$s"
  run_sync pull
  assert_code 2 "dangling link"
  assert_err_contains "settings.json is a link or not a regular file"
  [ -L "$s" ] || fail "dangling link was replaced"
  assert_eq "$(readlink "$s")" "$TEST_WORK/not-there.json" "dangling link target"
  [ ! -e "$TEST_WORK/not-there.json" ] || fail "the merge wrote through a dangling link"
  [ ! -d "$CLAUDE_HOME/.claude-sync-backup" ] || fail "dangling link made a backup"
  [ ! -e "$s.claude-sync-new" ] || fail "dangling link left a temp file"
  rm -f "$s"
fi

# No Claude dir yet: the merge creates it, with no shell error on stderr.
rm -rf "$CLAUDE_HOME"
run_sync pull
assert_code 0 "pull with no Claude dir"
assert_err_contains "created settings.json from settings.shared.json"
case "$RUN_ERR" in
  *"No such file"*|*"could not"*) fail "pull with no Claude dir printed an error: [$RUN_ERR]" ;;
esac
assert_eq "$(json_get "$s" '.')" '{"model":"shared"}' "created in a new Claude dir"
rm -rf "$CLAUDE_HOME"
run_sync pull --hook
assert_code 0 "hook pull with no Claude dir"
assert_err_at_most 1
assert_eq "$(json_get "$s" '.')" '{"model":"shared"}' "hook pull created it too"

# The merged file keeps the old file's mode.
printf '{"model": "local", "keep": 1}\n' > "$s"
if is_windows; then
  echo "skip: file mode check (Windows has no POSIX file modes)"
  run_sync pull
  assert_code 0 "merge"
else
  chmod 600 "$s"
  run_sync pull
  assert_code 0 "merge keeps mode"
  mode=$(stat -c '%a' "$s" 2>/dev/null || stat -f '%OLp' "$s")
  assert_eq "$mode" "600" "mode after merge"
fi
assert_eq "$(json_get "$s" '.')" '{"model":"shared","keep":1}' "merged"

# A backup that cannot be made stops the merge before any write.
rm -rf "$CLAUDE_HOME/.claude-sync-backup"
printf 'not a dir\n' > "$CLAUDE_HOME/.claude-sync-backup"
printf '{"model": "local again"}\n' > "$s"
cp "$s" "$TEST_WORK/settings.before"
run_sync pull
assert_code 3 "backup failure"
assert_err_contains "could not back up settings.json"
assert_err_contains "settings.json was not changed"
assert_file_eq "$s" "$TEST_WORK/settings.before"
[ ! -e "$s.claude-sync-new" ] || fail "backup failure left a temp file"
assert_dir_link "$CLAUDE_HOME/skills/alpha"
rm -f "$CLAUDE_HOME/.claude-sync-backup"

# A pull stopped by SIGTERM during the merge leaves no temp files and no lock.
real_jq=$(command -v "$TEST_JQ")
mkdir -p "$TEST_WORK/tmp"
cat > "$TEST_WORK/stop-jq" <<EOF
#!/usr/bin/env bash
case "\$*" in
  *"def merge"*) kill -TERM "\$PPID"; sleep 1; exit 1 ;;
esac
exec "$real_jq" "\$@"
EOF
chmod +x "$TEST_WORK/stop-jq"
before_tmp=$(find "$TEST_WORK/tmp" -mindepth 1 | wc -l | tr -d ' ')
assert_eq "$before_tmp" "0" "temp dir starts empty"
set +e
CLAUDE_SYNC_JQ="$TEST_WORK/stop-jq" TMPDIR="$TEST_WORK/tmp" "$SYNC_BIN" pull >/dev/null 2>"$TEST_WORK/stopped.err"
code=$?
set -e
[ "$code" -ne 0 ] || fail "stopped pull exited 0"
assert_file_eq "$s" "$TEST_WORK/settings.before"
left=$(find "$TEST_WORK/tmp" -mindepth 1 | wc -l | tr -d ' ')
assert_eq "$left" "0" "merge temp files left after SIGTERM"
[ ! -e "$s.claude-sync-new" ] || fail "SIGTERM left settings.json.claude-sync-new"
[ ! -e "$XDG_STATE_HOME/claude-sync/lock" ] || fail "SIGTERM left the lock"
