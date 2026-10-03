# shellcheck shell=bash
# Sourced by bin/claude-sync. Pull, push, the lock, and hook-mode behavior.
# This is the only file that runs network git commands (pull, push).
# link is intentionally not called; Phase 3 wires that in after a pull.

# shellcheck source=secrets.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/secrets.sh"

prepare_git() {
  # Network git must fail instead of prompting, paging, or opening an editor.
  export GIT_TERMINAL_PROMPT=0
  export GIT_PAGER=cat
  export PAGER=cat
  export GIT_EDITOR=true
  export GIT_SEQUENCE_EDITOR=true
  if [ -z "${GIT_SSH_COMMAND:-}" ]; then
    export GIT_SSH_COMMAND="ssh -o BatchMode=yes"
  fi
}

validate_timeout() {
  case "${CLAUDE_SYNC_TIMEOUT}" in
    ''|*[!0-9]*)
      die 1 "CLAUDE_SYNC_TIMEOUT must be a whole number of seconds."
      ;;
  esac
  if [ "$CLAUDE_SYNC_TIMEOUT" -lt 1 ]; then
    die 1 "CLAUDE_SYNC_TIMEOUT must be at least 1 second."
  fi
}

log_line() {
  mkdir -p "$STATE_DIR"
  printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >> "$STATE_DIR/log"
}

# One user-visible line. Hook mode keeps only the first of these on stderr.
note() {
  log_line "$*"
  if [ "$HOOK" -eq 1 ]; then
    if [ "$WARNED" -eq 0 ]; then
      printf 'claude-sync: %s\n' "$*" >&2
      WARNED=1
    fi
  else
    printf 'claude-sync: %s\n' "$*" >&2
  fi
}

die() {
  local code="$1"
  shift
  note "$*"
  if [ "$HOOK" -eq 1 ]; then
    exit 0
  fi
  exit "$code"
}

release_lock() {
  local lock
  if [ "${LOCK_HELD:-0}" != 1 ]; then
    return 0
  fi
  LOCK_HELD=0
  lock="$STATE_DIR/lock"
  if [ -L "$lock" ]; then
    rm -f "$lock"
    return 0
  fi
  if [ -d "$lock" ]; then
    rm -f "$lock/pid" "$lock/time"
    rmdir "$lock" 2>/dev/null || true
  fi
}

remove_lock_dir() {
  local lock="$1"
  if [ -L "$lock" ]; then
    rm -f "$lock"
    return 0
  fi
  rm -f "$lock/pid" "$lock/time"
  rmdir "$lock" 2>/dev/null || true
}

pid_alive() {
  local pid="$1"
  case "$pid" in
    ''|*[!0-9]*) return 1 ;;
  esac
  kill -0 "$pid" 2>/dev/null
}

lock_is_stale() {
  local lock="$1"
  local now stamp pid
  if [ ! -d "$lock" ]; then
    return 1
  fi
  now=$(date +%s)
  if [ -f "$lock/time" ]; then
    stamp=$(tr -d '[:space:]' < "$lock/time")
    case "$stamp" in
      ''|*[!0-9]*) stamp=$now ;;
    esac
    if [ $((now - stamp)) -ge 600 ]; then
      return 0
    fi
  fi
  if [ -f "$lock/pid" ]; then
    pid=$(tr -d '[:space:]' < "$lock/pid")
    if ! pid_alive "$pid"; then
      return 0
    fi
  fi
  return 1
}

# Returns 0 when this process holds the lock. Hook mode does not wait.
acquire_lock() {
  local lock="$STATE_DIR/lock"
  local waited=0
  mkdir -p "$STATE_DIR"
  while true; do
    if mkdir "$lock" 2>/dev/null; then
      printf '%s\n' "$$" > "$lock/pid"
      printf '%s\n' "$(date +%s)" > "$lock/time"
      LOCK_HELD=1
      return 0
    fi
    if lock_is_stale "$lock"; then
      log_line "taking over stale lock"
      remove_lock_dir "$lock"
      if [ ! -e "$lock" ]; then
        continue
      fi
    fi
    if [ "$HOOK" -eq 1 ]; then
      return 1
    fi
    if [ "$waited" -ge 30 ]; then
      return 1
    fi
    sleep 1
    waited=$((waited + 1))
  done
}

rebase_in_progress() {
  local gd
  gd=$(git -C "$CLAUDE_SYNC_DIR" rev-parse --absolute-git-dir)
  [ -d "$gd/rebase-merge" ] || [ -d "$gd/rebase-apply" ]
}

abort_rebase_if_any() {
  if rebase_in_progress; then
    git -C "$CLAUDE_SYNC_DIR" rebase --abort
    log_line "rebase aborted"
  fi
}

# Force-stop a git process that ignored SIGTERM. On Windows, kill the tree.
kill_pid() {
  local pid="$1" winpid
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
      # ps WINPID is the Windows id. taskkill cannot see the MSYS pid.
      winpid=$(ps -p "$pid" | awk 'NR==2 { print $4 }')
      if [ -n "${winpid:-}" ]; then
        taskkill //F //T //PID "$winpid" >/dev/null 2>&1 || true
      fi
      kill -9 "$pid" 2>/dev/null || true
      ;;
    *)
      kill -9 "$pid" 2>/dev/null || true
      ;;
  esac
}

# Run a git network command with CLAUDE_SYNC_TIMEOUT. Exit 124 on expiry.
# kill -0 cannot see git.exe from this shell on Windows, so liveness uses ps.
# The child must not inherit the caller's stdout: a command substitution waits
# until every writer of that pipe closes, which would ignore the timeout.
run_limited() {
  local secs="$1"
  shift
  local out_file status=0 was_e=0
  case $- in
    *e*) was_e=1 ;;
  esac
  out_file=$(mktemp)
  set +e
  (
    set +e
    "$@" >"$out_file" 2>&1 &
    pid=$!
    elapsed=0
    while [ "$elapsed" -lt "$secs" ]; do
      if ! ps -p "$pid" >/dev/null 2>&1; then
        wait "$pid"
        exit $?
      fi
      sleep 1
      elapsed=$((elapsed + 1))
    done
    if ! ps -p "$pid" >/dev/null 2>&1; then
      wait "$pid"
      exit $?
    fi
    kill_pid "$pid"
    wait "$pid" 2>/dev/null || true
    exit 124
  )
  status=$?
  if [ "$was_e" -eq 1 ]; then
    set -e
  fi
  cat "$out_file"
  rm -f "$out_file"
  return "$status"
}

require_repo() {
  if [ ! -d "$CLAUDE_SYNC_DIR/.git" ] && [ ! -f "$CLAUDE_SYNC_DIR/.git" ]; then
    if [ "$HOOK" -eq 1 ]; then
      log_line "repo not ready: $CLAUDE_SYNC_DIR"
      exit 0
    fi
    die 2 "no repo at $CLAUDE_SYNC_DIR. Set CLAUDE_SYNC_DIR to your config repo."
  fi
}

begin_sync() {
  LOCK_HELD=0
  WARNED=0
  HAS_ALLOWLISTED=0
  OTHER_UNTRACKED=""
  trap release_lock EXIT
  prepare_git
  validate_timeout
  if ! acquire_lock; then
    if [ "$HOOK" -eq 1 ]; then
      log_line "lock held; skipping"
      exit 0
    fi
    die 2 "another sync is running. Wait for it to finish, then try again."
  fi
  require_repo
}

path_allowlisted() {
  case "$1" in
    CLAUDE.md|CLAUDE.md/*|skills|skills/*|agents|agents/*|commands|commands/*|output-styles|output-styles/*)
      return 0
      ;;
  esac
  return 1
}

collect_status() {
  local tmp entry xy path
  HAS_ALLOWLISTED=0
  OTHER_UNTRACKED=""
  mkdir -p "$STATE_DIR"
  tmp=$(mktemp "$STATE_DIR/status.XXXXXX")
  if ! git -C "$CLAUDE_SYNC_DIR" status --porcelain -z --untracked-files=all > "$tmp"; then
    rm -f "$tmp"
    die 3 "could not read git status in $CLAUDE_SYNC_DIR. Check that the repo is usable, then try again."
  fi
  while IFS= read -r -d '' entry || [ -n "$entry" ]; do
    [ -n "$entry" ] || continue
    xy=${entry:0:2}
    path=${entry:3}
    # Rename and copy records are "XY old\0new\0". The new path is what we stage.
    if [ "${xy:0:1}" = "R" ] || [ "${xy:0:1}" = "C" ]; then
      IFS= read -r -d '' path || true
    fi
    if path_allowlisted "$path"; then
      HAS_ALLOWLISTED=1
    elif [ "$xy" = "??" ]; then
      OTHER_UNTRACKED="${OTHER_UNTRACKED}${path}
"
    fi
  done < "$tmp"
  rm -f "$tmp"
}

log_untracked() {
  local list
  [ -n "$OTHER_UNTRACKED" ] || return 0
  list=$(printf '%s' "$OTHER_UNTRACKED" | tr '\n' ' ')
  list=${list% }
  log_line "left untracked: $list"
}

report_untracked() {
  local list
  log_untracked || true
  [ -n "$OTHER_UNTRACKED" ] || return 0
  list=$(printf '%s' "$OTHER_UNTRACKED" | tr '\n' ' ')
  list=${list% }
  note "left untracked (not on the allowlist): $list"
}

stage_allowlist() {
  local p err status
  for p in CLAUDE.md skills agents commands output-styles; do
    if [ ! -e "$CLAUDE_SYNC_DIR/$p" ] && [ -z "$(git -C "$CLAUDE_SYNC_DIR" ls-files -- "$p")" ]; then
      continue
    fi
    status=0
    err=$(git -C "$CLAUDE_SYNC_DIR" add -A -- "$p" 2>&1) || status=$?
    if [ "$status" -ne 0 ]; then
      case "$err" in
        *"did not match any files"*) ;;
        *)
          die 3 "could not stage $p. Check the repo, then try again."
          ;;
      esac
    fi
  done
}

unstage_all() {
  if git -C "$CLAUDE_SYNC_DIR" rev-parse --verify HEAD >/dev/null 2>&1; then
    git -C "$CLAUDE_SYNC_DIR" reset -q
  else
    git -C "$CLAUDE_SYNC_DIR" rm -r --cached -q -- . >/dev/null 2>&1 || true
  fi
}

branch_is_ahead() {
  local count
  count=$(git -C "$CLAUDE_SYNC_DIR" rev-list --count '@{u}..HEAD' 2>/dev/null) || return 1
  [ "$count" -gt 0 ]
}

is_rejected_non_ff() {
  # Git has used both wordings. "fetch first" is the current rejection;
  # "non-fast-forward" is the older one. Neither is a hook or auth failure.
  case "$1" in
    *non-fast-forward*|*"fetch first"*|*"tip of your current branch is behind"*|*"remote contains work that you do"*)
      return 0
      ;;
  esac
  return 1
}

push_with_one_retry() {
  local status=0 out
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git -C "$CLAUDE_SYNC_DIR" push 2>&1) || status=$?
  if [ "$status" -eq 0 ]; then
    return 0
  fi
  if [ "$status" -eq 124 ]; then
    die 3 "push timed out after ${CLAUDE_SYNC_TIMEOUT}s. Check the network and try again."
  fi
  if ! is_rejected_non_ff "$out"; then
    die 3 "push failed (git or network). Check the remote and try again."
  fi
  log_line "push rejected; rebasing once"
  status=0
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git -C "$CLAUDE_SYNC_DIR" pull --rebase --autostash 2>&1) || status=$?
  case "$out" in
    *CONFLICT*) status=4 ;;
  esac
  if rebase_in_progress || [ "$status" -eq 4 ]; then
    abort_rebase_if_any || die 4 "push not applied. A rebase conflict is still open in $CLAUDE_SYNC_DIR. Run git rebase --abort there if it remains, then fix the conflict. Nothing was force-pushed."
    die 4 "push not applied. The remote has a conflicting change; the local commit was kept. Fix the conflict in the repo. Nothing was force-pushed."
  fi
  if [ "$status" -eq 124 ]; then
    abort_rebase_if_any || true
    die 3 "pull timed out after ${CLAUDE_SYNC_TIMEOUT}s while retrying the push. The local commit was kept. Try again."
  fi
  if [ "$status" -ne 0 ]; then
    abort_rebase_if_any || true
    die 3 "push failed while rebasing onto the remote. The local commit was kept. Try again."
  fi
  status=0
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git -C "$CLAUDE_SYNC_DIR" push 2>&1) || status=$?
  if [ "$status" -eq 0 ]; then
    return 0
  fi
  if [ "$status" -eq 124 ]; then
    die 3 "push timed out after ${CLAUDE_SYNC_TIMEOUT}s. The local commit was kept. Try again."
  fi
  if is_rejected_non_ff "$out"; then
    die 4 "push not applied after one rebase. The local commit was kept. Nothing was force-pushed."
  fi
  die 3 "push failed (git or network). The local commit was kept. Try again."
}

sync_pull() {
  local status=0 out
  begin_sync
  status=0
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git -C "$CLAUDE_SYNC_DIR" pull --rebase --autostash --quiet 2>&1) || status=$?
  if [ "$status" -ne 0 ]; then
    case "$out" in
      *CONFLICT*) status=4 ;;
    esac
    if rebase_in_progress || [ "$status" -eq 4 ]; then
      abort_rebase_if_any || die 4 "pull hit a conflict and the rebase could not be aborted. Run git rebase --abort in $CLAUDE_SYNC_DIR, then try again."
      die 4 "pull hit a conflict. The rebase was aborted and your files are unchanged. Fix the conflict in the repo, then pull again."
    fi
    if [ "$status" -eq 124 ]; then
      die 3 "pull timed out after ${CLAUDE_SYNC_TIMEOUT}s. Check the network and try again."
    fi
    die 3 "pull failed (git or network). Check the remote and try again."
  fi
  if rebase_in_progress; then
    abort_rebase_if_any || true
    die 4 "pull hit a conflict. The rebase was aborted and your files are unchanged. Fix the conflict in the repo, then pull again."
  fi
  printf '%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$STATE_DIR/last-pull"
  log_line "pull ok"
}

sync_push() {
  local hits first hit_line made_commit=0 scan_status=0
  begin_sync
  collect_status
  if [ "$HAS_ALLOWLISTED" -eq 1 ]; then
    stage_allowlist
    if ! git -C "$CLAUDE_SYNC_DIR" diff --cached --quiet; then
      scan_status=0
      hits=$(scan_staged "$CLAUDE_SYNC_DIR") || scan_status=$?
      if [ "$scan_status" -ne 0 ]; then
        unstage_all
        die 3 "secret scan could not read the staged diff. Nothing was committed."
      fi
      if [ -n "$hits" ]; then
        unstage_all
        first=$hits
        case "$hits" in
          *$'\n'*) first=${hits%%$'\n'*} ;;
        esac
        while IFS= read -r hit_line || [ -n "$hit_line" ]; do
          [ -n "$hit_line" ] || continue
          log_line "secret $hit_line"
        done <<EOF
$hits
EOF
        die 5 "secret found, nothing committed: $first. Remove it or add claude-sync: allow-secret on that line."
      fi
      if ! git -C "$CLAUDE_SYNC_DIR" -c commit.gpgsign=false commit -m "sync: $(uname -n) $(date -u +%Y-%m-%dT%H:%M:%SZ)" >/dev/null 2>&1; then
        die 3 "commit failed in $CLAUDE_SYNC_DIR. Set git user.name and user.email, then try again."
      fi
      made_commit=1
      log_line "committed allowlisted paths"
    fi
  fi
  if [ "$made_commit" -eq 1 ] || branch_is_ahead; then
    push_with_one_retry
    printf '%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$STATE_DIR/last-push"
    log_line "push ok"
  else
    log_line "push nothing to send"
  fi
  report_untracked
}
