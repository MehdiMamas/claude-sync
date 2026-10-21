#!/bin/sh
# Writes ~/Library/LaunchAgents/com.claude-sync.triage.plist from the template. It loads the
# agent only with --load. The three TRIAGE_APP_* values come from the environment.
#   TRIAGE_APP_ID=... TRIAGE_APP_INSTALLATION_ID=... TRIAGE_APP_KEY_FILE=~/.config/claude-sync/triage-app.pem \
#     sh relay/triage/install-launchd.sh [--load]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
relay=$(cd "$here/.." && pwd)
: "${TRIAGE_APP_ID:?set TRIAGE_APP_ID}"
: "${TRIAGE_APP_INSTALLATION_ID:?set TRIAGE_APP_INSTALLATION_ID}"
: "${TRIAGE_APP_KEY_FILE:?set TRIAGE_APP_KEY_FILE}"
dest="$HOME/Library/LaunchAgents/com.claude-sync.triage.plist"
mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
# A full user PATH: launchd's default has no node, npm, claude or gh.
path="$HOME/.local/bin:$HOME/.claude/local:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
esc() { printf '%s' "$1" | sed 's/[&|\\]/\\&/g'; }
sed -e "s|@RELAY_DIR@|$(esc "$relay")|g" \
    -e "s|@HOME@|$(esc "$HOME")|g" \
    -e "s|@PATH@|$(esc "$path")|g" \
    -e "s|@TRIAGE_APP_ID@|$(esc "$TRIAGE_APP_ID")|g" \
    -e "s|@TRIAGE_APP_INSTALLATION_ID@|$(esc "$TRIAGE_APP_INSTALLATION_ID")|g" \
    -e "s|@TRIAGE_APP_KEY_FILE@|$(esc "$TRIAGE_APP_KEY_FILE")|g" \
    "$here/launchd/com.claude-sync.triage.plist.template" > "$dest"
echo "wrote $dest"
if [ "${1:-}" = "--load" ]; then
  launchctl bootout "gui/$(id -u)" "$dest" 2>/dev/null || true
  launchctl bootstrap "gui/$(id -u)" "$dest"
  echo "loaded com.claude-sync.triage"
else
  echo "not loaded; run with --load, or: launchctl bootstrap gui/$(id -u) $dest"
fi
