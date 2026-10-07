# shellcheck shell=bash
# Sourced by bin/claude-sync. Reads the config repo's claude-sync.conf, which
# holds the opt-in switches (cursor=1 for lib/cursor.sh, settings=1 for
# lib/settings.sh). Does not run network git commands.

# Value of KEY in the repo's claude-sync.conf, or nothing. Lines are key=value.
# A '#' starts a comment through the end of the line, on its own line or after
# a value, so a value cannot contain '#'. Blank lines are skipped. Spaces
# around the key and the value are ignored. A repeated key keeps the last
# value. The file is never evaluated.
conf_value() {
  local key="$1" file="$CLAUDE_SYNC_DIR/claude-sync.conf" line k v val=""
  [ -f "$file" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    line=${line%$'\r'}
    line=${line%%#*}
    case "$line" in
      *=*) ;;
      *) continue ;;
    esac
    k=${line%%=*}
    v=${line#*=}
    k=${k#"${k%%[![:space:]]*}"}
    k=${k%"${k##*[![:space:]]}"}
    v=${v#"${v%%[![:space:]]*}"}
    v=${v%"${v##*[![:space:]]}"}
    case "$k" in
      '#'*) continue ;;
    esac
    if [ "$k" = "$key" ]; then
      val=$v
    fi
  done < "$file"
  printf '%s' "$val"
}
