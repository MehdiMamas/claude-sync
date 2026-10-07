#!/usr/bin/env bash
# This file compares literal ${CLAUDE_PLUGIN_ROOT} and backtick strings, so single quotes
# are meant.
# shellcheck disable=SC2016
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# The plugin files parse, name config-sync, agree on the version with
# claude-sync --version, and point at files that exist.
need_jq
make_work

json_raw() {
  "$TEST_JQ" -r "$2" < "$1" | tr -d '\r'
}

plugin="$ROOT/.claude-plugin/plugin.json"
market="$ROOT/.claude-plugin/marketplace.json"
hooks="$ROOT/hooks/hooks.json"
for f in "$plugin" "$market" "$hooks"; do
  [ -f "$f" ] || fail "missing $f"
  "$TEST_JQ" -e 'type == "object"' < "$f" >/dev/null || fail "$f is not one JSON object"
done

run_sync --version
assert_code 0
version=${RUN_OUT#claude-sync }
assert_eq "$(json_raw "$plugin" '.name')" "config-sync" "plugin name"
assert_eq "$(json_raw "$plugin" '.version')" "$version" "plugin.json version"
assert_eq "$(json_raw "$market" '.name')" "config-sync-marketplace" "marketplace name"
assert_eq "$(json_raw "$market" '.plugins | length')" "1" "marketplace lists one plugin"
assert_eq "$(json_raw "$market" '.plugins[0].name')" "config-sync" "marketplace plugin name"
assert_eq "$(json_raw "$market" '.plugins[0].source')" "./" "marketplace source"
assert_eq "$(json_raw "$market" '.plugins[0].version')" "$version" "marketplace version"

# Claude Code reserves these prefixes; a reserved name fails claude plugin validate.
for name in "$(json_raw "$plugin" '.name')" "$(json_raw "$market" '.name')"; do
  case "$name" in
    claude-*|anthropic-*|anthropics-*|cc-plugin-*|claude|anthropic)
      fail "reserved plugin or marketplace name: $name" ;;
  esac
done

assert_eq "$(json_raw "$hooks" '.hooks | keys | sort | join(",")')" "SessionEnd,SessionStart" "hook events"
assert_eq "$(json_raw "$hooks" '.hooks.SessionStart | length')" "1" "one SessionStart entry"
assert_eq "$(json_raw "$hooks" '.hooks.SessionEnd | length')" "1" "one SessionEnd entry"
assert_eq "$(json_raw "$hooks" '.hooks.SessionStart[0].matcher')" "startup|resume|clear" "SessionStart matcher"
assert_eq "$(json_raw "$hooks" '.hooks.SessionStart[0].hooks[0].command')" \
  'bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" pull --hook' "SessionStart command"
assert_eq "$(json_raw "$hooks" '.hooks.SessionEnd[0].hooks[0].command')" \
  'bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync" push --hook --detach' "SessionEnd command"
start_timeout=$(json_raw "$hooks" '.hooks.SessionStart[0].hooks[0].timeout')
# The pull's git step is limited by CLAUDE_SYNC_TIMEOUT (default 20 s); link runs after it.
[ "$start_timeout" -ge 60 ] || fail "SessionStart timeout $start_timeout does not cover CLAUDE_SYNC_TIMEOUT"
[ -f "$ROOT/bin/claude-sync" ] || fail "hooks point at a missing bin/claude-sync"

# Each command file runs claude-sync through the plugin root and never through an edit.
for name in setup status sync; do
  f="$ROOT/commands/$name.md"
  [ -f "$f" ] || fail "missing commands/$name.md"
  head -n 1 "$f" | grep -q '^---' || fail "commands/$name.md has no front matter"
  grep -q '^description: ' "$f" || fail "commands/$name.md has no description"
  grep -q -F 'bash "${CLAUDE_PLUGIN_ROOT}/bin/claude-sync"' "$f" || fail "commands/$name.md does not run the plugin's claude-sync"
  grep -q -F 'yourself' "$f" || fail "commands/$name.md does not forbid editing files directly"
done
grep -q -F 'Never pass `--merge` or `--replace` unless the user chose it' "$ROOT/commands/setup.md" \
  || fail "setup.md does not require asking before --merge or --replace"
grep -q -F 'conflict:' "$ROOT/commands/setup.md" || fail "setup.md does not show the conflict list"
