#!/usr/bin/env bash
# Runs every command on the v1 CLI (built from main) and the v2 CLI with a fake HOME and no
# network sign-in, then compares exit codes, the --help command list and the `status --json` keys.
# Usage: scripts/cli-parity.sh <scratch dir>   (sample data only; never touches your real home)
set -u
root=$(cd "$(dirname "$0")/.." && pwd)
scratch=${1:?scratch dir}
v1dir="$scratch/v1"
[ -d "$v1dir" ] || git -C "$root" worktree add "$v1dir" main
(cd "$v1dir" && cargo build -q -p claude-sync-cli) || exit 1
cargo build -q -p claude-sync-cli --manifest-path "$root/Cargo.toml" || exit 1
b1="$v1dir/target/debug/claude-sync-cli"
b2="$root/target/debug/claude-sync-cli"
[ -x "$b1" ] || b1="$root/target/debug/claude-sync-cli.v1"

run() { # binary, args...
  local bin=$1; shift
  local h; h=$(mktemp -d)
  HOME="$h" PATH=/usr/bin:/bin CLAUDE_HOME="$h/.claude" CLAUDE_SYNC_HOME="$h/.claude-sync" "$bin" "$@" >/dev/null 2>&1 </dev/null
  local c=$?
  rm -r "$h"
  echo $c
}
keys() { local h; h=$(mktemp -d); HOME="$h" CLAUDE_HOME="$h/.claude" CLAUDE_SYNC_HOME="$h/.claude-sync" "$1" status --json 2>/dev/null | python3 -c 'import json,sys;print(" ".join(sorted(json.load(sys.stdin))))'; rm -r "$h"; }
cmds() { "$1" --help 2>&1 | awk '/^Commands:/{f=1;next} /^$/{f=0} f{print $1;next} /^  [a-z]/{print $1}' | grep -v '^help$' | sort -u | tr '\n' ' '; }

bad=0
printf '%-28s %3s %3s\n' command v1 v2
while IFS= read -r line; do
  # shellcheck disable=SC2086
  a=$(run "$b1" $line); b=$(run "$b2" $line)
  m=""; [ "$a" = "$b" ] || { m="  DIFFERENT"; bad=1; }
  printf '%-28s %3s %3s%s\n' "${line:-<none>}" "$a" "$b" "$m"
done <<'LIST'

bogus
version
status
status --json
groups
groups --json
sync
join nogroup
join a/b
leave a/b
logout
personal status
personal enable
ssh status --json
ssh enable
ssh alias
activity status --json
activity enable
packs
review list
update --check
run-job nope
run-job a/b nope
run-job nope --now
service status
daemon --bogus
LIST
echo "v1 commands: $(cmds "$b1")"
echo "v2 commands: $(cmds "$b2")"
echo "v1 status keys: $(keys "$b1")"
echo "v2 status keys: $(keys "$b2")"
exit $bad
