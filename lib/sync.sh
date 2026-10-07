# shellcheck shell=bash
# Sourced by bin/claude-sync. Pull, push, the lock, and hook-mode behavior.
# This is the only file that runs network git commands (pull, push, clone, ls-remote).
# After a successful pull, the opt-in settings merge runs, then link, so new items
# appear and removed ones are cleaned up.

# shellcheck source=lib/secrets.sh
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
  # Git Credential Manager opens a GUI that GIT_TERMINAL_PROMPT does not stop.
  if [ "$HOOK" -eq 1 ]; then
    export GCM_INTERACTIVE=never
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

# Hook mode keeps the terminal's stderr on fd 3 and sends everything else to the log.
setup_hook_stdio() {
  if [ "$HOOK" -ne 1 ]; then
    return 0
  fi
  exec 3>&2
  HOOK_STDERR=1
  if mkdir -p "$STATE_DIR" 2>/dev/null && touch "$STATE_DIR/log" 2>/dev/null; then
    exec 2>>"$STATE_DIR/log" || exec 2>/dev/null
  else
    exec 2>/dev/null
  fi
}

log_line() {
  mkdir -p "$STATE_DIR" 2>/dev/null || return 0
  printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >> "$STATE_DIR/log" 2>/dev/null || true
}

# Append git's own output to the log. Credentials in URLs are removed first.
# Diff text is never passed here.
log_git_detail() {
  local line cleaned
  [ -n "${1:-}" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    [ -n "$line" ] || continue
    cleaned=$(printf '%s\n' "$line" | sed 's#://[^/@]*@#://#g')
    log_line "git: $cleaned"
  done <<EOF
$1
EOF
}

write_stamp() {
  mkdir -p "$STATE_DIR" 2>/dev/null || return 0
  printf '%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$STATE_DIR/$1" 2>/dev/null || true
}

# One user-visible line. Hook mode keeps only the first of these on the terminal.
note() {
  log_line "$*" || true
  if [ "$HOOK" -eq 1 ]; then
    if [ "$WARNED" -eq 0 ]; then
      WARNED=1
      if [ "${HOOK_STDERR:-0}" -eq 1 ]; then
        printf 'claude-sync: %s\n' "$*" >&3
      else
        printf 'claude-sync: %s\n' "$*" >&2
      fi
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

finish_sync() {
  local status=$?
  # Any exit path puts the user's non-allowlisted staging back.
  restore_outside_staged || true
  release_lock || true
  if [ "${HOOK:-0}" -eq 1 ]; then
    exit 0
  fi
  exit "$status"
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
  if [ ! -d "$lock" ] || [ -L "$lock" ]; then
    return 1
  fi
  # A lock dir with neither pid nor time was left behind mid-create.
  # Treat it as stale once the directory itself is older than 10 minutes.
  if [ ! -f "$lock/pid" ] && [ ! -f "$lock/time" ]; then
    if find "$lock" -prune -mmin +10 2>/dev/null | grep -q .; then
      return 0
    fi
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
  local waited=0 stale
  mkdir -p "$STATE_DIR" 2>/dev/null || return 1
  while true; do
    if mkdir "$lock" 2>/dev/null; then
      printf '%s\n' "$$" > "$lock/pid" || true
      printf '%s\n' "$(date +%s)" > "$lock/time" || true
      LOCK_HELD=1
      return 0
    fi
    if lock_is_stale "$lock"; then
      log_line "taking over stale lock"
      # Rename first so two waiters cannot both delete a lock the other just took.
      stale="${lock}.stale.$$"
      if mv "$lock" "$stale" 2>/dev/null; then
        remove_lock_dir "$stale"
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
  gd=$(git -C "$CLAUDE_SYNC_DIR" rev-parse --absolute-git-dir 2>/dev/null) || return 1
  [ -d "$gd/rebase-merge" ] || [ -d "$gd/rebase-apply" ]
}

abort_rebase_if_any() {
  if rebase_in_progress; then
    git -C "$CLAUDE_SYNC_DIR" rebase --abort
    log_line "rebase aborted"
  fi
}

# Stop a git process: SIGTERM, a short wait, then SIGKILL.
# On Windows, kill the process tree. A killed git can leave index.lock;
# SIGTERM gives it a chance to remove that lock first.
kill_pid() {
  local pid="$1" winpid i
  kill -TERM "$pid" 2>/dev/null || true
  i=0
  while [ "$i" -lt 10 ]; do
    if ! ps -p "$pid" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
    i=$((i + 1))
  done
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

reset_sync_state() {
  LOCK_HELD=0
  WARNED=0
  HAS_ALLOWLISTED=0
  OTHER_UNTRACKED=""
  OUTSIDE_SAVED=""
  HOOK_STDERR=0
}

begin_sync() {
  reset_sync_state
  trap finish_sync EXIT
  setup_hook_stdio
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
    CLAUDE.md|CLAUDE.md/*|skills|skills/*|agents|agents/*|commands|commands/*|output-styles|output-styles/*|claude-sync.conf)
      return 0
      ;;
    cursor|cursor/*)
      # Opt-in: only with cursor=1 in claude-sync.conf.
      if cursor_enabled; then
        return 0
      fi
      ;;
    settings.shared.json)
      # Opt-in: only with settings=1. settings.json itself is never allowlisted.
      if settings_enabled; then
        return 0
      fi
      ;;
  esac
  return 1
}

collect_status() {
  local tmp entry xy path old
  HAS_ALLOWLISTED=0
  OTHER_UNTRACKED=""
  mkdir -p "$STATE_DIR" 2>/dev/null || true
  tmp=$(mktemp "$STATE_DIR/status.XXXXXX" 2>/dev/null || mktemp)
  if ! git -C "$CLAUDE_SYNC_DIR" status --porcelain -z --untracked-files=all > "$tmp"; then
    rm -f "$tmp"
    die 3 "could not read git status in $CLAUDE_SYNC_DIR. Check that the repo is usable, then try again."
  fi
  while IFS= read -r -d '' entry || [ -n "$entry" ]; do
    [ -n "$entry" ] || continue
    xy=${entry:0:2}
    path=${entry:3}
    old=""
    # porcelain -z rename/copy records are "XY <new>\0<old>\0".
    if [ "${xy:0:1}" = "R" ] || [ "${xy:0:1}" = "C" ]; then
      IFS= read -r -d '' old || true
    fi
    if path_allowlisted "$path" || { [ -n "$old" ] && path_allowlisted "$old"; }; then
      HAS_ALLOWLISTED=1
    elif [ "$xy" = "??" ]; then
      OTHER_UNTRACKED="${OTHER_UNTRACKED}${path}
"
    fi
  done < "$tmp"
  rm -f "$tmp"
}

path_was_reported() {
  local path="$1" line
  [ -f "$STATE_DIR/reported-untracked" ] || return 1
  while IFS= read -r line || [ -n "$line" ]; do
    [ "$line" = "$path" ] && return 0
  done < "$STATE_DIR/reported-untracked"
  return 1
}

remember_untracked() {
  mkdir -p "$STATE_DIR" 2>/dev/null || return 0
  printf '%s\n' "$1" >> "$STATE_DIR/reported-untracked" 2>/dev/null || true
}

# Untracked files outside the allowlist are mentioned once per path.
report_untracked() {
  local path fresh="" list
  [ -n "$OTHER_UNTRACKED" ] || return 0
  while IFS= read -r path || [ -n "$path" ]; do
    [ -n "$path" ] || continue
    if path_was_reported "$path"; then
      continue
    fi
    remember_untracked "$path"
    fresh="${fresh}${path}
"
  done <<EOF
$OTHER_UNTRACKED
EOF
  [ -n "$fresh" ] || return 0
  list=$(printf '%s' "$fresh" | tr '\n' ' ')
  list=${list% }
  log_line "left untracked: $list"
  note "left untracked (not on the allowlist): $list"
}

stage_allowlist() {
  local p err status
  set -- CLAUDE.md skills agents commands output-styles claude-sync.conf
  if cursor_enabled; then
    set -- "$@" cursor
  fi
  if settings_enabled; then
    set -- "$@" settings.shared.json
  fi
  for p in "$@"; do
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

# Record one non-allowlisted index entry, then unstage it so the commit cannot include it.
save_index_entry() {
  local path="$1" line mode sha
  # :(literal) so a path like notes[1].txt is not a glob.
  line=$(git -C "$CLAUDE_SYNC_DIR" ls-files -s -z -- ":(literal)$path" | tr '\0' '\n' || true)
  line=${line%%$'\n'*}
  if [ -n "$line" ]; then
    mode=${line%% *}
    sha=${line#* }
    sha=${sha%% *}
    printf 'C\0%s\0%s\0%s\0' "$mode" "$sha" "$path" >> "$OUTSIDE_SAVED"
  else
    printf 'R\0%s\0' "$path" >> "$OUTSIDE_SAVED"
  fi
  if git -C "$CLAUDE_SYNC_DIR" rev-parse --verify HEAD >/dev/null 2>&1; then
    git -C "$CLAUDE_SYNC_DIR" reset -q -- ":(literal)$path"
  else
    git -C "$CLAUDE_SYNC_DIR" rm --cached -q -- ":(literal)$path" >/dev/null 2>&1 || true
  fi
}

# Pull non-allowlisted staged paths out of the index. A rename or copy stays
# only when the new path is allowlisted. A rename out of the allowlist commits
# the deletion and leaves the new path unstaged from this commit.
shelve_outside_staged() {
  local tmp status path old new
  OUTSIDE_SAVED=$(mktemp)
  tmp=$(mktemp)
  if ! git -C "$CLAUDE_SYNC_DIR" diff --cached --name-status -z > "$tmp"; then
    rm -f "$tmp" "$OUTSIDE_SAVED"
    OUTSIDE_SAVED=""
    return 1
  fi
  while IFS= read -r -d '' status || [ -n "$status" ]; do
    [ -n "$status" ] || continue
    case "$status" in
      R*|C*)
        # name-status -z is "R100\0<old>\0<new>\0", unlike porcelain -z.
        IFS= read -r -d '' old || true
        IFS= read -r -d '' new || true
        if path_allowlisted "$new"; then
          continue
        fi
        save_index_entry "$new"
        if ! path_allowlisted "$old"; then
          save_index_entry "$old"
        fi
        ;;
      *)
        IFS= read -r -d '' path || true
        if path_allowlisted "$path"; then
          continue
        fi
        save_index_entry "$path"
        ;;
    esac
  done < "$tmp"
  rm -f "$tmp"
}

restore_outside_staged() {
  local kind mode sha path
  [ -n "${OUTSIDE_SAVED:-}" ] && [ -f "$OUTSIDE_SAVED" ] || return 0
  while IFS= read -r -d '' kind || [ -n "$kind" ]; do
    [ -n "$kind" ] || continue
    case "$kind" in
      C)
        IFS= read -r -d '' mode || break
        IFS= read -r -d '' sha || break
        IFS= read -r -d '' path || break
        git -C "$CLAUDE_SYNC_DIR" update-index --add --cacheinfo "$mode" "$sha" "$path"
        ;;
      R)
        IFS= read -r -d '' path || break
        git -C "$CLAUDE_SYNC_DIR" update-index --force-remove -- "$path"
        ;;
    esac
  done < "$OUTSIDE_SAVED"
  rm -f "$OUTSIDE_SAVED"
  OUTSIDE_SAVED=""
}

reset_index() {
  if git -C "$CLAUDE_SYNC_DIR" rev-parse --verify HEAD >/dev/null 2>&1; then
    git -C "$CLAUDE_SYNC_DIR" reset -q
  else
    git -C "$CLAUDE_SYNC_DIR" rm -r --cached -q -- . >/dev/null 2>&1 || true
  fi
}

abandon_staged_sync() {
  reset_index
  restore_outside_staged
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

pull_has_conflict() {
  local out="$1"
  if rebase_in_progress; then
    return 0
  fi
  if [ -n "$(git -C "$CLAUDE_SYNC_DIR" ls-files -u 2>/dev/null)" ]; then
    return 0
  fi
  case "$out" in
    *CONFLICT*|*"resulted in conflicts"*)
      return 0
      ;;
  esac
  return 1
}

stash_sha() {
  git -C "$CLAUDE_SYNC_DIR" rev-parse -q --verify refs/stash 2>/dev/null || true
}

# Returns 0 after putting back the pre-pull commit and the autostash this pull
# created. Returns 1, and does not touch the tree, when this pull created no stash.
undo_autostash_conflict() {
  local before="$1" stash_before="$2" stash_now
  stash_now=$(stash_sha)
  if [ -z "$stash_now" ] || [ "$stash_now" = "$stash_before" ]; then
    return 1
  fi
  if [ -n "$before" ]; then
    git -C "$CLAUDE_SYNC_DIR" reset --hard "$before" >/dev/null 2>&1 || return 1
  fi
  git -C "$CLAUDE_SYNC_DIR" stash pop >/dev/null 2>&1 || true
  if [ -n "$(git -C "$CLAUDE_SYNC_DIR" ls-files -u 2>/dev/null)" ] && [ -n "$before" ]; then
    git -C "$CLAUDE_SYNC_DIR" reset --hard "$before" >/dev/null 2>&1 || true
    if [ -n "$(stash_sha)" ] && [ "$(stash_sha)" != "$stash_before" ]; then
      git -C "$CLAUDE_SYNC_DIR" stash pop >/dev/null 2>&1 || true
    fi
  fi
  return 0
}

refuse_if_merging() {
  local gd
  if rebase_in_progress; then
    die 4 "a rebase is in progress in $CLAUDE_SYNC_DIR. Run git rebase --abort or finish it, then try again."
  fi
  gd=$(git -C "$CLAUDE_SYNC_DIR" rev-parse --absolute-git-dir 2>/dev/null || true)
  if [ -n "$gd" ] && [ -f "$gd/MERGE_HEAD" ]; then
    die 4 "a merge is in progress in $CLAUDE_SYNC_DIR. Finish or abort the merge, then try again."
  fi
  if [ -n "$(git -C "$CLAUDE_SYNC_DIR" ls-files -u 2>/dev/null)" ]; then
    die 4 "unmerged files are in $CLAUDE_SYNC_DIR. Resolve them, then try again."
  fi
}

push_with_one_retry() {
  local status=0 out retry_before="" stash_before=""
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git -C "$CLAUDE_SYNC_DIR" push 2>&1) || status=$?
  if [ "$status" -eq 0 ]; then
    return 0
  fi
  if [ "$status" -eq 124 ]; then
    log_git_detail "$out"
    die 3 "push timed out after ${CLAUDE_SYNC_TIMEOUT}s. Check the network and try again."
  fi
  if ! is_rejected_non_ff "$out"; then
    log_git_detail "$out"
    die 3 "push failed (git or network). Check the remote and try again."
  fi
  log_line "push rejected; rebasing once"
  retry_before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD 2>/dev/null || true)
  stash_before=$(stash_sha)
  status=0
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git -C "$CLAUDE_SYNC_DIR" pull --rebase --autostash 2>&1) || status=$?
  if [ "$status" -eq 124 ]; then
    log_git_detail "$out"
    abort_rebase_if_any || true
    die 3 "pull timed out after ${CLAUDE_SYNC_TIMEOUT}s while retrying the push. The local commit was kept. Try again."
  fi
  if rebase_in_progress; then
    log_git_detail "$out"
    abort_rebase_if_any || die 4 "push not applied. A rebase conflict is still open in $CLAUDE_SYNC_DIR. Run git rebase --abort there if it remains, then fix the conflict. Nothing was force-pushed."
    die 4 "push not applied. The remote has a conflicting change; the local commit was kept. Fix the conflict in the repo. Nothing was force-pushed."
  fi
  if pull_has_conflict "$out"; then
    log_git_detail "$out"
    if undo_autostash_conflict "$retry_before" "$stash_before"; then
      die 4 "push not applied. Local edits clashed with the remote while retrying. The local commit was kept and your edits are unchanged. Nothing was force-pushed."
    fi
    die 4 "push not applied. The remote has a conflicting change; the local commit was kept. Fix the conflict in the repo. Nothing was force-pushed."
  fi
  if [ "$status" -ne 0 ]; then
    log_git_detail "$out"
    abort_rebase_if_any || true
    die 3 "push failed while rebasing onto the remote. The local commit was kept. Try again."
  fi
  status=0
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git -C "$CLAUDE_SYNC_DIR" push 2>&1) || status=$?
  if [ "$status" -eq 0 ]; then
    return 0
  fi
  if [ "$status" -eq 124 ]; then
    log_git_detail "$out"
    die 3 "push timed out after ${CLAUDE_SYNC_TIMEOUT}s. The local commit was kept. Try again."
  fi
  if is_rejected_non_ff "$out"; then
    log_git_detail "$out"
    die 4 "push not applied after one rebase. The local commit was kept. Nothing was force-pushed."
  fi
  log_git_detail "$out"
  die 3 "push failed (git or network). The local commit was kept. Try again."
}

sync_pull() {
  local status=0 out before="" stash_before="" settings_status=0
  begin_sync
  refuse_if_merging
  before=$(git -C "$CLAUDE_SYNC_DIR" rev-parse HEAD 2>/dev/null || true)
  stash_before=$(stash_sha)
  status=0
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git -C "$CLAUDE_SYNC_DIR" pull --rebase --autostash --quiet 2>&1) || status=$?
  if [ "$status" -eq 124 ]; then
    log_git_detail "$out"
    abort_rebase_if_any || true
    die 3 "pull timed out after ${CLAUDE_SYNC_TIMEOUT}s. Check the network and try again."
  fi
  if pull_has_conflict "$out"; then
    log_git_detail "$out"
    if rebase_in_progress; then
      abort_rebase_if_any || die 4 "pull hit a conflict and the rebase could not be aborted. Run git rebase --abort in $CLAUDE_SYNC_DIR, then try again."
      die 4 "pull hit a conflict. The rebase was aborted and your files are unchanged. Fix the conflict in the repo, then pull again."
    fi
    if undo_autostash_conflict "$before" "$stash_before"; then
      die 4 "pull hit a conflict while reapplying local edits. The pull was undone and your files are unchanged. Fix the conflict in the repo, then pull again."
    fi
    die 4 "pull hit a conflict. The repo was left as it was. Resolve it, then try again."
  fi
  if [ "$status" -ne 0 ]; then
    log_git_detail "$out"
    die 3 "pull failed (git or network). Check the remote and try again."
  fi
  write_stamp last-pull
  log_line "pull ok"
  # settings_merge is in lib/settings.sh. It runs before link so a link conflict
  # does not hold back the merge, and its failure does not hold back link.
  settings_status=0
  settings_merge || settings_status=$?
  # link is defined in lib/link.sh, which the dispatcher sources before a command runs.
  cmd_link
  if [ "$settings_status" -ne 0 ]; then
    exit "$settings_status"
  fi
}

report_secret_hits() {
  local hits="$1" first="$2" line
  if [ "$HOOK" -eq 1 ]; then
    while IFS= read -r line || [ -n "$line" ]; do
      [ -n "$line" ] || continue
      log_line "secret $line"
    done <<EOF
$hits
EOF
    die 5 "secret found, nothing committed: $first. Remove it or add claude-sync: allow-secret on that line."
  fi
  while IFS= read -r line || [ -n "$line" ]; do
    [ -n "$line" ] || continue
    note "secret found, nothing committed: $line. Remove it or add claude-sync: allow-secret on that line."
  done <<EOF
$hits
EOF
  exit 5
}

sync_push() {
  local hits first scan_status=0 made_commit=0
  begin_sync
  refuse_if_merging
  collect_status
  if [ "$HAS_ALLOWLISTED" -eq 1 ]; then
    if ! shelve_outside_staged; then
      die 3 "could not read the index in $CLAUDE_SYNC_DIR. Nothing was committed."
    fi
    stage_allowlist
    if ! git -C "$CLAUDE_SYNC_DIR" diff --cached --quiet; then
      scan_status=0
      hits=$(scan_staged "$CLAUDE_SYNC_DIR") || scan_status=$?
      if [ "$scan_status" -ne 0 ]; then
        abandon_staged_sync
        die 3 "secret scan could not read the staged diff. Nothing was committed."
      fi
      if [ -n "$hits" ]; then
        first=$hits
        case "$hits" in
          *$'\n'*) first=${hits%%$'\n'*} ;;
        esac
        abandon_staged_sync
        report_secret_hits "$hits" "$first"
      fi
      if ! git -C "$CLAUDE_SYNC_DIR" -c commit.gpgsign=false commit -m "sync: $(uname -n) $(date -u +%Y-%m-%dT%H:%M:%SZ)" >/dev/null 2>&1; then
        abandon_staged_sync
        die 3 "commit failed in $CLAUDE_SYNC_DIR. Set git user.name and user.email, then try again."
      fi
      made_commit=1
      log_line "committed allowlisted paths"
    fi
    restore_outside_staged
  fi
  if [ "$made_commit" -eq 1 ] || branch_is_ahead; then
    push_with_one_retry
    log_line "push ok"
  else
    log_line "push nothing to send"
  fi
  write_stamp last-push
  report_untracked
}

# 0 when the remote has no refs. Dies on a network or timeout failure.
sync_remote_empty() {
  local url="$1" status=0 out=""
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git ls-remote "$url" 2>&1) || status=$?
  if [ "$status" -eq 124 ]; then
    log_git_detail "$out"
    die 3 "timed out contacting the remote after ${CLAUDE_SYNC_TIMEOUT}s. Check the URL and try again."
  fi
  if [ "$status" -ne 0 ]; then
    log_git_detail "$out"
    die 3 "could not read the remote. Check the URL and try again."
  fi
  if [ -n "$out" ]; then
    return 1
  fi
  return 0
}

sync_clone() {
  local url="$1" dest="$2" status=0 out=""
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git -c core.autocrlf=false clone "$url" "$dest" 2>&1) || status=$?
  if [ "$status" -eq 124 ]; then
    log_git_detail "$out"
    die 3 "clone timed out after ${CLAUDE_SYNC_TIMEOUT}s. Check the URL and try again."
  fi
  if [ "$status" -ne 0 ]; then
    log_git_detail "$out"
    die 3 "clone failed (git or network). Check the URL and try again."
  fi
  git -C "$dest" config core.autocrlf false
  git -C "$dest" config commit.gpgsign false
}

# First push sets upstream. A later push uses the one allowed rebase retry.
sync_publish() {
  local status=0 out=""
  if git -C "$CLAUDE_SYNC_DIR" rev-parse --abbrev-ref --symbolic-full-name '@{u}' >/dev/null 2>&1; then
    push_with_one_retry
    return 0
  fi
  out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" git -C "$CLAUDE_SYNC_DIR" push -u origin HEAD 2>&1) || status=$?
  if [ "$status" -eq 0 ]; then
    write_stamp last-push
    log_line "push ok"
    return 0
  fi
  if [ "$status" -eq 124 ]; then
    log_git_detail "$out"
    die 3 "push timed out after ${CLAUDE_SYNC_TIMEOUT}s. The local commit was kept. Try again."
  fi
  log_git_detail "$out"
  die 3 "push failed (git or network). The local commit was kept. Try again."
}
