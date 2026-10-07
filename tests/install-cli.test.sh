#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=tests/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# install-cli writes ~/.local/bin/claude-sync, a shim that runs the plugin's
# bin/claude-sync. It is idempotent, leaves other files alone, prints a PATH hint
# only when needed, and falls back to the plugin cache when its copy is gone.
make_work
shim="$HOME/.local/bin/claude-sync"
version=$("$SYNC_BIN" --version)

# A plugin copy in a path with a space and a quote, as a cache path might have.
copy_plugin() {
  mkdir -p "$1"
  cp -R "$ROOT/bin" "$ROOT/lib" "$ROOT/templates" "$1/"
}
plugin_a="$TEST_WORK/plugin a'b"
copy_plugin "$plugin_a"

run_sync install-cli
assert_code 0 "install-cli"
assert_out_contains "installed: $shim"
assert_out_contains "is not on PATH"
[ -f "$shim" ] || fail "no shim written"
[ -x "$shim" ] || fail "shim is not executable"
head -n 2 "$shim" | grep -q -F '# claude-sync install-cli shim' || fail "shim has no marker line"
assert_eq "$("$shim" --version)" "$version" "shim runs claude-sync"

cp "$shim" "$TEST_WORK/shim.before"
run_sync install-cli
assert_code 0 "second install-cli"
assert_out_contains "already installed"
assert_file_eq "$shim" "$TEST_WORK/shim.before"

PATH="$HOME/.local/bin:$PATH" run_sync install-cli
assert_code 0 "install-cli with the dir on PATH"
case "$RUN_OUT" in
  *"not on PATH"*) fail "PATH hint printed when the dir is on PATH" ;;
esac

# Arguments and exit codes pass through.
set +e
"$shim" status >/dev/null 2>&1
code=$?
set -e
assert_eq "$code" "2" "shim passes the exit code through"

# Install from another copy: the shim now runs that copy.
set +e
bash "$plugin_a/bin/claude-sync" install-cli >"$TEST_WORK/out" 2>&1
code=$?
set -e
assert_eq "$code" "0" "install-cli from a copy"
grep -q -F "runs $plugin_a/bin/claude-sync" "$TEST_WORK/out" || fail "install-cli did not name the copy: $(cat "$TEST_WORK/out")"
assert_eq "$("$shim" --version)" "$version" "shim runs the copy"

# The copy is gone (the plugin was updated): the newest copy in the plugin cache runs.
rm -rf "$plugin_a"
CLAUDE_CONFIG_DIR="$TEST_WORK/config dir"
export CLAUDE_CONFIG_DIR
copy_plugin "$CLAUDE_CONFIG_DIR/plugins/cache/config-sync-marketplace/config-sync/0.0.1"
assert_eq "$("$shim" --version)" "$version" "shim falls back to the plugin cache"

rm -rf "$CLAUDE_CONFIG_DIR/plugins"
set +e
"$shim" --version >"$TEST_WORK/out" 2>"$TEST_WORK/err"
code=$?
set -e
assert_eq "$code" "2" "shim with no plugin anywhere"
grep -q 'plugin was not found' "$TEST_WORK/err" || fail "shim did not say the plugin is missing: $(cat "$TEST_WORK/err")"
unset CLAUDE_CONFIG_DIR

# A file install-cli did not write is left alone.
printf 'my own script\n' > "$shim"
cp "$shim" "$TEST_WORK/own.before"
run_sync install-cli
assert_code 2 "install-cli over another file"
assert_err_contains "was not written by install-cli"
assert_file_eq "$shim" "$TEST_WORK/own.before"

# So is a directory at that path.
rm -f "$shim"
mkdir -p "$shim"
run_sync install-cli
assert_code 2 "install-cli over a directory"
assert_err_contains "not a regular file"
[ -d "$shim" ] || fail "the directory was replaced"
rmdir "$shim"

run_sync install-cli --hook
assert_code 0 "install-cli rejects --hook"
assert_err_lines 1
assert_err_contains "unknown argument"
