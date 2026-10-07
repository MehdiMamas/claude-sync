# shellcheck shell=bash
# Sourced by bin/claude-sync. What the Claude Code plugin needs beyond pull and
# push: the detached SessionEnd push and the install-cli terminal shim (SPEC.md
# section 10). Runs no network git commands itself.

# The plugin's name in Claude Code. The plugin cache path uses it.
PLUGIN_NAME=config-sync
SHIM_MARKER="# claude-sync install-cli shim"

# Claude Code cancels a SessionEnd hook after about 1.5 seconds, which is too
# short for a push. This starts `push --hook` in the background with no stdio and
# SIGHUP ignored, then returns at once. With no repo it starts nothing.
detach_push() {
  if [ ! -d "$CLAUDE_SYNC_DIR/.git" ] && [ ! -f "$CLAUDE_SYNC_DIR/.git" ]; then
    log_line "repo not ready: $CLAUDE_SYNC_DIR"
    exit 0
  fi
  (
    trap '' HUP
    exec "${BASH:-bash}" "$ROOT/bin/claude-sync" push --hook
  ) </dev/null >/dev/null 2>&1 &
  log_line "push started in the background (pid $!)"
  disown "$!" 2>/dev/null || true
  exit 0
}

# The shim runs the plugin copy it was written for. When that copy is gone (the
# plugin was updated, so its versioned cache dir changed), it runs the newest
# config-sync copy in the Claude Code plugin cache.
shim_text() {
  printf '#!/usr/bin/env bash\n'
  printf '%s. Run claude-sync install-cli again to rewrite it.\n' "$SHIM_MARKER"
  printf 'set -euo pipefail\n'
  printf 'target=%q\n' "$1"
  printf 'plugin=%q\n' "$PLUGIN_NAME"
  cat <<'EOF'
if [ ! -f "$target" ]; then
  found=""
  for candidate in "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"/plugins/cache/*/"$plugin"/*/bin/claude-sync; do
    [ -f "$candidate" ] || continue
    if [ -z "$found" ] || [ "$candidate" -nt "$found" ]; then
      found=$candidate
    fi
  done
  if [ -z "$found" ]; then
    printf 'claude-sync: the %s plugin was not found at %s or in the plugin cache. Install the plugin, then run claude-sync install-cli again.\n' "$plugin" "$target" >&2
    exit 2
  fi
  target=$found
fi
exec bash "$target" "$@"
EOF
}

cmd_install_cli() {
  local dir="$HOME/.local/bin" shim target tmp
  shim="$dir/claude-sync"
  target="$ROOT/bin/claude-sync"
  if [ ! -f "$target" ]; then
    die 2 "cannot find $target. Run install-cli from an installed plugin or a checkout."
  fi
  if [ -L "$shim" ] || { [ -e "$shim" ] && [ ! -f "$shim" ]; }; then
    die 2 "$shim is a link or not a regular file, so it was left alone. Move it aside, then run claude-sync install-cli again."
  fi
  if [ -f "$shim" ]; then
    if [ "$shim" -ef "$target" ]; then
      die 2 "$shim is claude-sync itself, so it was left alone."
    fi
    if ! grep -q -F -- "$SHIM_MARKER" "$shim" 2>/dev/null; then
      die 2 "$shim already exists and was not written by install-cli, so it was left alone. Move it aside, then run claude-sync install-cli again."
    fi
  fi
  if ! mkdir -p -- "$dir" 2>/dev/null; then
    die 3 "could not create $dir. Nothing was installed."
  fi
  tmp="$shim.claude-sync-new"
  if ! { shim_text "$target" > "$tmp"; } 2>/dev/null || ! chmod 755 "$tmp" 2>/dev/null; then
    rm -f -- "$tmp" 2>/dev/null || true
    die 3 "could not write $shim. Nothing was installed."
  fi
  if [ -f "$shim" ] && cmp -s "$tmp" "$shim"; then
    rm -f -- "$tmp"
    printf 'already installed: %s runs %s\n' "$shim" "$target"
  elif mv -f -- "$tmp" "$shim" 2>/dev/null; then
    printf 'installed: %s runs %s\n' "$shim" "$target"
  else
    rm -f -- "$tmp" 2>/dev/null || true
    die 3 "could not write $shim. Nothing was installed."
  fi
  case ":$PATH:" in
    *":$dir:"*) ;;
    *)
      # Single quotes: the hint is for the user to paste, so $HOME and $PATH stay literal.
      # shellcheck disable=SC2016
      printf '%s is not on PATH. Add this line to your shell profile: export PATH="$HOME/.local/bin:$PATH"\n' "$dir"
      ;;
  esac
}
