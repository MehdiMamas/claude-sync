# shellcheck shell=bash
# Sourced by bin/claude-sync. Opt-in shared settings (SPEC.md section 16): with
# settings=1 in claude-sync.conf, the repo's settings.shared.json is merged into
# $CLAUDE_HOME/settings.json after each successful pull. This is the only file
# that reads or writes settings.json, and it never copies settings.json into the
# repo. The merge uses jq (CLAUDE_SYNC_JQ, default jq). Values are never printed
# or logged. Does not run network git commands.

SETTINGS_SHARED_NAME=settings.shared.json

settings_enabled() {
  [ "$(conf_value settings)" = "1" ]
}

# Objects merge key by key. An array gets the shared items it does not have yet,
# in shared order. Any other pair is a scalar conflict, and the shared value wins.
# A local object or array whose shared value has another type is an error that
# names the key path, because replacing it would remove local keys.
# The program prints the merged object, or nothing when it equals the local one.
# The $ names are jq variables, not shell ones.
# shellcheck disable=SC2016
SETTINGS_MERGE_JQ='
def merge($l; $s; $p):
  if ($l | type) == "object" and ($s | type) == "object" then
    reduce ($s | keys_unsorted[]) as $k ($l;
      if has($k) then .[$k] = merge(.[$k]; $s[$k]; $p + [$k])
      else .[$k] = $s[$k] end)
  elif ($l | type) == "array" and ($s | type) == "array" then
    reduce $s[] as $x ($l; if any(.[]; . == $x) then . else . + [$x] end)
  elif ($l | type) == "object" or ($l | type) == "array" then
    error("claude-sync-type-conflict:" + ($p | map(tostring) | join(".")))
  else $s end;
.[0] as $l | merge($l; .[1]; []) as $m | if $m == $l then empty else $m end
'

# 0 when the file holds exactly one JSON object. stdin is used, not a path
# argument, so MSYS path conversion does not matter for a native jq.exe.
settings_is_object() {
  local jq_bin="$1" file="$2" out
  out=$("$jq_bin" -s 'length == 1 and (.[0] | type) == "object"' < "$file" 2>/dev/null) || return 1
  out=$(printf '%s' "$out" | tr -d '\r')
  [ "$out" = "true" ]
}

# Merge after a successful pull. Prints one note on a change or a problem.
# Returns 0 (merged, or nothing to do), 2 (jq missing, invalid JSON, or a type
# conflict; settings.json unchanged) or 3 (a file could not be written;
# settings.json unchanged).
settings_merge() {
  local jq_bin="${CLAUDE_SYNC_JQ:-jq}" shared="$CLAUDE_SYNC_DIR/$SETTINGS_SHARED_NAME"
  local file="$CLAUDE_HOME/settings.json" work err status=0 key backup stamp n=0 tmp
  settings_enabled || return 0
  if [ ! -f "$shared" ]; then
    log_line "settings=1 but $SETTINGS_SHARED_NAME is not in the repo; nothing to merge"
    return 0
  fi
  if ! command -v "$jq_bin" >/dev/null 2>&1; then
    note "settings=1 needs jq to merge $SETTINGS_SHARED_NAME, and $jq_bin was not found. Install jq or set CLAUDE_SYNC_JQ, then pull again. settings.json was not changed."
    return 2
  fi
  if [ -e "$file" ] && { [ -L "$file" ] || [ ! -f "$file" ]; }; then
    note "settings.json is not a regular file, so $SETTINGS_SHARED_NAME was not merged. settings.json was not changed."
    return 2
  fi
  work=$(mktemp -d "${TMPDIR:-/tmp}/claude-sync-settings.XXXXXX") || {
    note "could not create a temp dir to merge $SETTINGS_SHARED_NAME. settings.json was not changed."
    return 3
  }
  if [ -f "$file" ]; then
    if ! cat -- "$file" > "$work/local.json"; then
      rm -rf -- "$work"
      note "could not read settings.json, so $SETTINGS_SHARED_NAME was not merged. settings.json was not changed."
      return 3
    fi
  else
    printf '{}\n' > "$work/local.json"
  fi
  if ! settings_is_object "$jq_bin" "$work/local.json"; then
    rm -rf -- "$work"
    note "settings.json is not one valid JSON object, so $SETTINGS_SHARED_NAME was not merged. Fix settings.json, then pull again. settings.json was not changed."
    return 2
  fi
  if ! settings_is_object "$jq_bin" "$shared"; then
    rm -rf -- "$work"
    note "$SETTINGS_SHARED_NAME is not one valid JSON object, so it was not merged. Fix it in the repo, then pull again. settings.json was not changed."
    return 2
  fi
  { cat -- "$work/local.json"; printf '\n'; cat -- "$shared"; } > "$work/both.json" || {
    rm -rf -- "$work"
    note "could not prepare the merge of $SETTINGS_SHARED_NAME. settings.json was not changed."
    return 3
  }
  status=0
  "$jq_bin" -s "$SETTINGS_MERGE_JQ" < "$work/both.json" > "$work/merged.raw" 2> "$work/err" || status=$?
  if [ "$status" -ne 0 ]; then
    err=$(tr -d '\r' < "$work/err")
    rm -rf -- "$work"
    case "$err" in
      *claude-sync-type-conflict:*)
        key=${err#*claude-sync-type-conflict:}
        key=${key%%$'\n'*}
        key=$(printf '%s' "$key" | tr -d '[:cntrl:]')
        [ -n "$key" ] || key="(top level)"
        note "$SETTINGS_SHARED_NAME has a value of another type for $key, which settings.json holds as an object or array, so nothing was merged. Make the types match, then pull again. settings.json was not changed."
        ;;
      *)
        note "jq could not merge $SETTINGS_SHARED_NAME. settings.json was not changed."
        ;;
    esac
    return 2
  fi
  # A native jq.exe writes CRLF. JSON strings escape a carriage return, so only line ends lose one.
  tr -d '\r' < "$work/merged.raw" > "$work/merged.json"
  if [ ! -s "$work/merged.json" ]; then
    rm -rf -- "$work"
    log_line "settings already include $SETTINGS_SHARED_NAME"
    return 0
  fi
  if ! settings_is_object "$jq_bin" "$work/merged.json"; then
    rm -rf -- "$work"
    note "jq did not return one JSON object for the merge of $SETTINGS_SHARED_NAME. settings.json was not changed."
    return 2
  fi
  backup=""
  if [ -f "$file" ]; then
    # Same layout as the init and join backups. No ':' because Windows forbids it.
    stamp="$CLAUDE_HOME/.claude-sync-backup/$(date -u +%Y%m%dT%H%M%SZ)"
    backup=$stamp
    while [ -e "$backup/settings.json" ]; do
      n=$((n + 1))
      backup="${stamp}-${n}"
    done
    if ! mkdir -p -- "$backup" || ! cp -p -- "$file" "$backup/settings.json"; then
      rm -rf -- "$work"
      note "could not back up settings.json, so $SETTINGS_SHARED_NAME was not merged. settings.json was not changed."
      return 3
    fi
  fi
  tmp="$file.claude-sync-new"
  rm -f -- "$tmp" || true
  if ! cat -- "$work/merged.json" > "$tmp"; then
    rm -f -- "$tmp" || true
    rm -rf -- "$work"
    note "could not write settings.json, so it was not changed."
    return 3
  fi
  if [ -f "$file" ]; then
    # apply_mode is in lib/link.sh. The merged file keeps the old file's mode.
    apply_mode "$file" "$tmp"
  fi
  if ! mv -f -- "$tmp" "$file"; then
    rm -f -- "$tmp" || true
    rm -rf -- "$work"
    note "could not write settings.json, so it was not changed."
    return 3
  fi
  rm -rf -- "$work"
  if [ -n "$backup" ]; then
    note "merged $SETTINGS_SHARED_NAME into settings.json. Backup: $backup/settings.json"
  else
    note "created settings.json from $SETTINGS_SHARED_NAME."
  fi
  return 0
}
