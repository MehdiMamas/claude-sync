#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

seed_sync_repo
run_sync link
assert_code 0

# On Windows the skill link must be a junction. On Unix this asserts a symlink.
assert_dir_link "$CLAUDE_HOME/skills/alpha"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    parent=$(cygpath -w -- "$CLAUDE_HOME/skills")
    out=$(cmd.exe //c dir //AL "$parent") || fail "dir /AL failed"
    case "$out" in
      *"<JUNCTION>"*) ;;
      *) fail "Windows skill link is not a junction: $out" ;;
    esac
    ;;
  *)
    if [ ! -L "$CLAUDE_HOME/skills/alpha" ]; then
      fail "skill link is not a symlink"
    fi
    ;;
esac
