# shellcheck shell=bash
# Sourced by tests/*.test.sh. Assertions and temp HOME / bare-remote helpers.
# Does not set shell options.

LIB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$LIB_DIR/.." && pwd)
SYNC_BIN="$ROOT/bin/claude-sync"

export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-Test User}"
export GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-test@example.com}"
export GIT_COMMITTER_NAME="${GIT_COMMITTER_NAME:-Test User}"
export GIT_COMMITTER_EMAIL="${GIT_COMMITTER_EMAIL:-test@example.com}"
export GIT_TERMINAL_PROMPT=0

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

assert_eq() {
  if [ "$1" != "$2" ]; then
    printf 'FAIL: %s\n expected: [%s]\n actual:   [%s]\n' "${3:-values differ}" "$2" "$1" >&2
    exit 1
  fi
}

assert_code() {
  if [ "$RUN_CODE" -ne "$1" ]; then
    printf 'FAIL: %s\n expected exit %s, got %s\n stderr: [%s]\n stdout: [%s]\n' \
      "${2:-exit status}" "$1" "$RUN_CODE" "$RUN_ERR" "$RUN_OUT" >&2
    exit 1
  fi
}

assert_out_contains() {
  case "$RUN_OUT" in
    *"$1"*) ;;
    *) fail "stdout missing [$1]: [$RUN_OUT]" ;;
  esac
}

assert_err_contains() {
  case "$RUN_ERR" in
    *"$1"*) ;;
    *) fail "stderr missing [$1]: [$RUN_ERR]" ;;
  esac
}

assert_err_lines() {
  local want="$1" n=0 line
  if [ -n "$RUN_ERR" ]; then
    while IFS= read -r line || [ -n "$line" ]; do
      [ -n "$line" ] || continue
      n=$((n + 1))
    done <<EOF
$RUN_ERR
EOF
  fi
  if [ "$n" -ne "$want" ]; then
    fail "expected $want stderr line(s), got $n: [$RUN_ERR]"
  fi
}

# Fail if the needle appears in command output. The needle itself is not printed.
assert_output_lacks() {
  local needle="$1"
  printf '%s\n%s\n' "$RUN_OUT" "$RUN_ERR" > "$TEST_WORK/output-check"
  if grep -F -q -e "$needle" "$TEST_WORK/output-check"; then
    fail "command output contained a secret value"
  fi
}

cleanup() {
  local pid
  if [ "${#CLEAN_PIDS[@]}" -gt 0 ]; then
    for pid in "${CLEAN_PIDS[@]}"; do
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    done
  fi
  if [ -n "${TEST_WORK:-}" ] && [ -d "$TEST_WORK" ]; then
    rm -rf "$TEST_WORK"
  fi
}

track_pid() {
  CLEAN_PIDS[${#CLEAN_PIDS[@]}]=$1
}

make_work() {
  TEST_WORK=$(mktemp -d)
  CLEAN_PIDS=()
  trap cleanup EXIT
  TEST_HOME="$TEST_WORK/home dir"
  mkdir -p "$TEST_HOME/.claude" "$TEST_HOME/.local/state"
  enter_home "$TEST_HOME"
}

enter_home() {
  HOME="$1"
  export HOME
  CLAUDE_HOME="$HOME/.claude"
  export CLAUDE_HOME
  CLAUDE_SYNC_DIR="$HOME/.claude-sync"
  export CLAUDE_SYNC_DIR
  XDG_STATE_HOME="$HOME/.local/state"
  export XDG_STATE_HOME
  mkdir -p "$CLAUDE_HOME" "$XDG_STATE_HOME"
}

file_url() {
  local p="$1"
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
      p=$(cygpath -m "$p")
      ;;
  esac
  p=${p// /%20}
  case "$p" in
    /*) printf 'file://%s' "$p" ;;
    *) printf 'file:///%s' "$p" ;;
  esac
}

harden_repo() {
  git -C "$1" config core.autocrlf false
  git -C "$1" config commit.gpgsign false
}

prepare_bare() {
  local seed
  make_work
  TEST_BARE="$TEST_WORK/remote.git"
  git init --bare "$TEST_BARE" >/dev/null
  seed="$TEST_WORK/seed"
  git init -b main "$seed" >/dev/null
  harden_repo "$seed"
  printf 'seed\n' > "$seed/README.md"
  git -C "$seed" add README.md
  git -C "$seed" commit -m seed >/dev/null
  git -C "$seed" remote add origin "$(file_url "$TEST_BARE")"
  git -C "$seed" push -u origin main >/dev/null 2>&1
  git --git-dir="$TEST_BARE" symbolic-ref HEAD refs/heads/main
  rm -rf "$seed"
}

clone_here() {
  git -c core.autocrlf=false clone "$(file_url "$TEST_BARE")" "$CLAUDE_SYNC_DIR" >/dev/null 2>&1
  harden_repo "$CLAUDE_SYNC_DIR"
}

add_second_home() {
  TEST_HOME_B="$TEST_WORK/home b"
  mkdir -p "$TEST_HOME_B/.claude" "$TEST_HOME_B/.local/state"
  enter_home "$TEST_HOME_B"
  clone_here
  enter_home "$TEST_HOME"
}

run_sync() {
  local was_e=0
  : > "$TEST_WORK/stdout"
  : > "$TEST_WORK/stderr"
  case $- in
    *e*) was_e=1 ;;
  esac
  set +e
  "$SYNC_BIN" "$@" >"$TEST_WORK/stdout" 2>"$TEST_WORK/stderr"
  RUN_CODE=$?
  if [ "$was_e" -eq 1 ]; then
    set -e
  fi
  RUN_OUT=$(<"$TEST_WORK/stdout")
  RUN_ERR=$(<"$TEST_WORK/stderr")
}

repeat_char() {
  local char="$1" count="$2" i=0 out=""
  while [ "$i" -lt "$count" ]; do
    out="${out}${char}"
    i=$((i + 1))
  done
  printf '%s' "$out"
}

assert_no_rebase() {
  local dir="$1" gd
  gd=$(git -C "$dir" rev-parse --absolute-git-dir)
  if [ -d "$gd/rebase-merge" ] || [ -d "$gd/rebase-apply" ]; then
    fail "rebase still in progress"
  fi
}

snapshot_repo() {
  local dir="$1"
  {
    git -C "$dir" rev-parse HEAD
    git -C "$dir" status --porcelain
    git -C "$dir" hash-object -- "$dir/CLAUDE.md"
  } 2>/dev/null || true
}
