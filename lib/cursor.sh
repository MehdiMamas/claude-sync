# shellcheck shell=bash
# Sourced by bin/claude-sync. Opt-in Cursor sync: claude-sync.conf parsing and the
# cursor/ items (skills/<name>, rules, agents, commands). Uses link_one and item_state
# from lib/link.sh. Does not run network git commands.

# Value of KEY in the repo's claude-sync.conf, or nothing. Lines are key=value;
# blank lines and lines starting with # are skipped. The file is never evaluated.
conf_value() {
  local key="$1" file="$CLAUDE_SYNC_DIR/claude-sync.conf" line k v val=""
  [ -f "$file" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    line=${line%$'\r'}
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

cursor_enabled() {
  [ "$(conf_value cursor)" = "1" ]
}

# Directories Cursor reads as one folder. Skills are one link per child, below.
cursor_whole_dirs() {
  printf '%s\n' rules agents commands
}

# Local path for a repo-relative item. cursor/<x> lives under CURSOR_HOME.
local_path_of() {
  case "$1" in
    cursor/*) printf '%s/%s' "$CURSOR_HOME" "${1#cursor/}" ;;
    *) printf '%s/%s' "$CLAUDE_HOME" "$1" ;;
  esac
}

# Cursor links that exist locally. The callers keep only links that point into the repo.
each_cursor_link() {
  local path name
  cursor_enabled || return 0
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    path="$CURSOR_HOME/$name"
    if is_link "$path"; then
      printf '%s\n' "$path"
    fi
  done <<EOF
$(cursor_whole_dirs)
EOF
  path="$CURSOR_HOME/skills"
  if [ -d "$path" ] && [ ! -L "$path" ]; then
    for path in "$CURSOR_HOME/skills"/*; do
      [ -e "$path" ] || [ -L "$path" ] || continue
      if is_link "$path"; then
        printf '%s\n' "$path"
      fi
    done
  fi
}

# Real (not linked) Cursor items, as repo-relative names.
each_cursor_local_real() {
  local name path
  path="$CURSOR_HOME/skills"
  if [ -d "$path" ] && ! is_link "$path"; then
    for path in "$CURSOR_HOME/skills"/*; do
      [ -d "$path" ] || continue
      is_link "$path" && continue
      printf '%s\n' "cursor/skills/$(basename -- "$path")"
    done
  fi
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    path="$CURSOR_HOME/$name"
    if [ -d "$path" ] && ! is_link "$path"; then
      printf '%s\n' "cursor/$name"
    fi
  done <<EOF
$(cursor_whole_dirs)
EOF
}

link_cursor() {
  local repo_skills="$CLAUDE_SYNC_DIR/cursor/skills" local_skills="$CURSOR_HOME/skills" dir name
  if [ -e "$CURSOR_HOME" ] && [ ! -d "$CURSOR_HOME" ]; then
    note "conflict: $CURSOR_HOME is not a directory. Left it unchanged."
    LINK_CONFLICTS=$((LINK_CONFLICTS + 1))
    return 0
  fi
  if [ -d "$repo_skills" ]; then
    if [ -L "$local_skills" ] || { [ -e "$local_skills" ] && [ ! -d "$local_skills" ]; }; then
      note "conflict: cursor/skills is not a real directory. Left it unchanged. Move it aside, then run claude-sync link again."
      LINK_CONFLICTS=$((LINK_CONFLICTS + 1))
    else
      mkdir -p "$local_skills" || die 3 "could not create $local_skills."
      for dir in "$repo_skills"/*/; do
        [ -d "$dir" ] || continue
        link_one "$local_skills/$(basename -- "$dir")" "$dir"
      done
    fi
  fi
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    [ -d "$CLAUDE_SYNC_DIR/cursor/$name" ] || continue
    mkdir -p "$CURSOR_HOME" || die 3 "could not create $CURSOR_HOME."
    link_one "$CURSOR_HOME/$name" "$CLAUDE_SYNC_DIR/cursor/$name"
  done <<EOF
$(cursor_whole_dirs)
EOF
}

cursor_status_items() {
  local dir name target
  cursor_enabled || return 0
  if [ -d "$CLAUDE_SYNC_DIR/cursor/skills" ]; then
    for dir in "$CLAUDE_SYNC_DIR/cursor/skills"/*/; do
      [ -d "$dir" ] || continue
      basename -- "$dir"
    done | sort | while IFS= read -r name; do
      [ -n "$name" ] || continue
      target=$(existing_dir "$CLAUDE_SYNC_DIR/cursor/skills/$name")
      printf 'cursor/skills/%s: %s\n' "$name" "$(item_state "$CURSOR_HOME/skills/$name" "$target")"
    done
  fi
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    if [ -d "$CLAUDE_SYNC_DIR/cursor/$name" ]; then
      target=$(existing_dir "$CLAUDE_SYNC_DIR/cursor/$name")
    else
      target=""
    fi
    printf 'cursor/%s: %s\n' "$name" "$(item_state "$CURSOR_HOME/$name" "$target")"
  done <<EOF
$(cursor_whole_dirs)
EOF
}
