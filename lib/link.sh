# shellcheck shell=bash
# Sourced by bin/claude-sync. Directory links, the CLAUDE.md block, unlink, status.
# Platform differences live only in the helpers below.
# Does not run network git commands. pull calls link in Phase 3.

# Dispatcher exports these before sourcing this file.
: "${WARNED:=0}"

BLOCK_BEGIN='<!-- claude-sync:begin (managed, do not edit inside) -->'
BLOCK_END='<!-- claude-sync:end -->'

on_windows() {
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) return 0 ;;
    *) return 1 ;;
  esac
}

# MSYS turns a single /J into a drive path before cmd sees it. //J arrives as /J.
# Same for //c, which arrives as /c. Do not set MSYS2_ARG_CONV_EXCL here.
run_cmd() {
  cmd.exe //c "$@"
}

to_unix_path() {
  local p="$1"
  if on_windows; then
    p=$(cygpath -u -- "$p")
  fi
  printf '%s' "$p"
}

fold_path() {
  local p="$1"
  if on_windows; then
    printf '%s' "$p" | tr '[:upper:]' '[:lower:]'
  else
    printf '%s' "$p"
  fi
}

# Absolute path with . and .. collapsed. The path does not have to exist.
lexical_abs() {
  local p="$1" rest part out=""
  p=$(to_unix_path "$p")
  p=${p%$'\r'}
  case "$p" in
    /*) ;;
    *) p="$(pwd -L)/$p" ;;
  esac
  while [ "${p%/}" != "$p" ] && [ "$p" != "/" ]; do
    p=${p%/}
  done
  rest=${p#/}
  out=""
  while [ -n "$rest" ]; do
    part=${rest%%/*}
    case "$rest" in
      */*) rest=${rest#*/} ;;
      *) rest="" ;;
    esac
    case "$part" in
      ''|.) ;;
      ..)
        out=${out%/*}
        ;;
      *)
        out="$out/$part"
        ;;
    esac
  done
  [ -n "$out" ] || out=/
  printf '%s' "$out"
}

canon_path() {
  fold_path "$(lexical_abs "$1")"
}

paths_equal() {
  [ "$(canon_path "$1")" = "$(canon_path "$2")" ]
}

path_inside() {
  local child parent
  child=$(canon_path "$1")
  parent=$(canon_path "$2")
  [ "$child" = "$parent" ] && return 0
  case "$child" in
    "$parent"/*) return 0 ;;
    *) return 1 ;;
  esac
}

existing_dir() {
  (cd "$1" && pwd -L)
}

is_reparse() {
  local w
  w=$(cygpath -w -- "$1")
  run_cmd fsutil reparsepoint query "$w" >/dev/null 2>&1
}

is_link() {
  if [ -L "$1" ]; then
    return 0
  fi
  # A junction bash does not report as a symlink is still a directory reparse point.
  on_windows || return 1
  [ -d "$1" ] || return 1
  is_reparse "$1"
}

junction_print_name() {
  local w out line
  w=$(cygpath -w -- "$1")
  out=$(run_cmd fsutil reparsepoint query "$w" 2>/dev/null) || return 1
  while IFS= read -r line || [ -n "$line" ]; do
    line=${line%$'\r'}
    case "$line" in
      "Print Name:"*)
        line=${line#Print Name:}
        while [ "${line# }" != "$line" ]; do
          line=${line# }
        done
        while [ "${line#	}" != "$line" ]; do
          line=${line#	}
        done
        printf '%s' "$line"
        return 0
        ;;
    esac
  done <<EOF
$out
EOF
  return 1
}

link_target() {
  local path="$1" target
  if [ -L "$path" ]; then
    target=$(readlink -- "$path") || return 1
  elif on_windows && [ -e "$path" ] && is_reparse "$path"; then
    target=$(junction_print_name "$path") || return 1
  else
    return 1
  fi
  target=${target%$'\r'}
  case "$target" in
    /*|[A-Za-z]:\\*|[A-Za-z]:/*) ;;
    *) target="$(dirname -- "$path")/$target" ;;
  esac
  printf '%s' "$target"
}

link_points_inside() {
  local target
  target=$(link_target "$1") || return 1
  path_inside "$target" "$CLAUDE_SYNC_DIR"
}

target_exists() {
  local target
  target=$(link_target "$1") || return 1
  [ -e "$target" ]
}

make_dir_link() {
  local link="$1" target="$2" wlink wtarget msg status
  if on_windows; then
    wlink=$(cygpath -w -- "$link")
    wtarget=$(cygpath -w -- "$target")
    status=0
    msg=$(run_cmd mklink //J "$wlink" "$wtarget" 2>&1) || status=$?
    if [ "$status" -ne 0 ]; then
      note "$msg"
      die 3 "could not create a junction at $link."
    fi
  else
    ln -s -- "$target" "$link"
  fi
}

remove_dir_link() {
  local link="$1" wlink msg status
  while [ "$link" != "/" ] && [ "${link%/}" != "$link" ]; do
    link=${link%/}
  done
  if [ -z "$link" ] || [ "$link" = "/" ] || [ "$link" = "." ]; then
    die 3 "refusing to remove $link."
  fi
  if ! is_link "$link"; then
    die 3 "refusing to remove $link because it is not a link."
  fi
  if on_windows; then
    wlink=$(cygpath -w -- "$link")
    status=0
    # rmdir removes the junction. It does not delete the target directory.
    msg=$(run_cmd rmdir "$wlink" 2>&1) || status=$?
    if [ "$status" -ne 0 ]; then
      note "$msg"
      die 3 "could not remove the junction at $link. The target was left in place."
    fi
  else
    # No -r, and no trailing slash, so rm removes the symlink itself.
    rm -- "$link"
  fi
}

repo_import_prefix() {
  local repo home repo_abs home_abs repo_c home_c
  repo=$(existing_dir "$CLAUDE_SYNC_DIR")
  home=$(existing_dir "$HOME")
  repo_abs=$(lexical_abs "$repo")
  home_abs=$(lexical_abs "$home")
  repo_c=$(fold_path "$repo_abs")
  home_c=$(fold_path "$home_abs")
  case "$repo_c" in
    "$home_c")
      printf '~'
      ;;
    "$home_c"/*)
      # Compare case-insensitively on Windows, but keep the repo's own spelling.
      # The tilde is text for Claude's @import line, not a path this shell expands.
      printf '~%s' "${repo_abs:${#home_abs}}"
      ;;
    *)
      if on_windows; then
        # Native Claude Code cannot open an MSYS path such as /d/sync or /tmp.
        cygpath -m -- "$repo"
      else
        printf '%s' "$repo"
      fi
      ;;
  esac
}

import_line() {
  printf '@%s/CLAUDE.md' "$(repo_import_prefix)"
}

render_block() {
  printf '%s\n%s\n%s\n' "$BLOCK_BEGIN" "$(import_line)" "$BLOCK_END"
}

first_marker_line() {
  local file="$1" needle="$2" line num=0
  while IFS= read -r line || [ -n "$line" ]; do
    num=$((num + 1))
    line=${line%$'\r'}
    if [ "$line" = "$needle" ]; then
      printf '%s' "$num"
      return 0
    fi
  done < "$file"
  return 1
}

block_import_line() {
  local file="$1" begin_line end_line line num=0
  begin_line=$(first_marker_line "$file" "$BLOCK_BEGIN") || return 1
  end_line=$(first_marker_line "$file" "$BLOCK_END") || return 1
  if [ "$end_line" -le "$begin_line" ]; then
    return 1
  fi
  while IFS= read -r line || [ -n "$line" ]; do
    num=$((num + 1))
    if [ "$num" -gt "$begin_line" ] && [ "$num" -lt "$end_line" ]; then
      line=${line%$'\r'}
      printf '%s' "$line"
      return 0
    fi
  done < "$file"
  return 1
}

require_repo_dir() {
  if [ ! -d "$CLAUDE_SYNC_DIR" ]; then
    die 2 "no repo at $CLAUDE_SYNC_DIR. Set CLAUDE_SYNC_DIR to your config repo."
  fi
}

# Replace dest with the bytes in src. A failed write leaves dest as it was.
apply_mode() {
  local src="$1" dest="$2" mode
  if chmod --reference="$src" "$dest" 2>/dev/null; then
    return 0
  fi
  mode=$(stat -f '%OLp' "$src" 2>/dev/null) || return 0
  chmod "$mode" "$dest" 2>/dev/null || true
}

install_file() {
  local dest="$1" src="$2" tmp
  tmp="${dest}.claude-sync-new"
  rm -f -- "$tmp" || true
  if ! cat -- "$src" > "$tmp"; then
    rm -f -- "$tmp"
    die 3 "could not update $dest. It was left unchanged."
  fi
  if [ -f "$dest" ] && [ ! -L "$dest" ]; then
    apply_mode "$dest" "$tmp"
  fi
  if ! mv -f -- "$tmp" "$dest"; then
    rm -f -- "$tmp"
    die 3 "could not update $dest. It was left unchanged."
  fi
}

fail_block_write() {
  rm -f -- "${1:-}"
  die 3 "could not update CLAUDE.md. It was left unchanged."
}

# Rewrite CLAUDE.md so the managed block is present and points at this repo.
# Returns 0, or 1 when the file was left unchanged because it is a conflict.
sync_claude_block() {
  local file="$CLAUDE_HOME/CLAUDE.md" tmp begin_line="" end_line=""
  if [ -L "$file" ] || [ -d "$file" ]; then
    note "conflict: CLAUDE.md is not a regular file. Left it unchanged. Move it aside, then run claude-sync link again."
    return 1
  fi
  tmp=$(mktemp "${TMPDIR:-/tmp}/claude-sync-block.XXXXXX") || die 3 "could not create a temp file to update CLAUDE.md. It was left unchanged."
  if [ -f "$file" ]; then
    if begin_line=$(first_marker_line "$file" "$BLOCK_BEGIN"); then
      if ! end_line=$(first_marker_line "$file" "$BLOCK_END"); then
        rm -f "$tmp"
        note "conflict: CLAUDE.md has a managed-block start without an end. Left it unchanged. Close the block, then run claude-sync link again."
        return 1
      fi
      if [ "$end_line" -lt "$begin_line" ]; then
        rm -f "$tmp"
        note "conflict: CLAUDE.md managed block is out of order. Left it unchanged."
        return 1
      fi
      if [ "$begin_line" -gt 1 ]; then
        head -n "$((begin_line - 1))" "$file" > "$tmp" || fail_block_write "$tmp"
      fi
      render_block >> "$tmp" || fail_block_write "$tmp"
      tail -n "+$((end_line + 1))" "$file" >> "$tmp" || fail_block_write "$tmp"
    else
      render_block > "$tmp" || fail_block_write "$tmp"
      cat -- "$file" >> "$tmp" || fail_block_write "$tmp"
    fi
    if cmp -s "$file" "$tmp"; then
      rm -f "$tmp"
      return 0
    fi
    install_file "$file" "$tmp"
    rm -f "$tmp"
    note "updated the managed block in CLAUDE.md."
    return 0
  fi
  render_block > "$tmp" || fail_block_write "$tmp"
  install_file "$file" "$tmp"
  rm -f "$tmp" || true
  note "created CLAUDE.md with the managed block."
  return 0
}

remove_claude_block() {
  local file="$CLAUDE_HOME/CLAUDE.md" tmp begin_line end_line
  if [ ! -f "$file" ] || [ -L "$file" ] || [ -d "$file" ]; then
    return 0
  fi
  if ! begin_line=$(first_marker_line "$file" "$BLOCK_BEGIN"); then
    return 0
  fi
  if ! end_line=$(first_marker_line "$file" "$BLOCK_END"); then
    note "conflict: CLAUDE.md has a managed-block start without an end. Left it unchanged. Close the block, then run claude-sync link again."
    return 1
  fi
  if [ "$end_line" -lt "$begin_line" ]; then
    note "conflict: CLAUDE.md managed block is out of order. Left it unchanged."
    return 1
  fi
  tmp=$(mktemp "${TMPDIR:-/tmp}/claude-sync-block.XXXXXX") || die 3 "could not create a temp file to update CLAUDE.md. It was left unchanged."
  if [ "$begin_line" -gt 1 ]; then
    head -n "$((begin_line - 1))" "$file" > "$tmp" || fail_block_write "$tmp"
  fi
  tail -n "+$((end_line + 1))" "$file" >> "$tmp" || fail_block_write "$tmp"
  if cmp -s "$file" "$tmp"; then
    rm -f "$tmp" || true
    return 0
  fi
  install_file "$file" "$tmp"
  rm -f "$tmp" || true
  note "removed the managed block from CLAUDE.md."
  return 0
}

# Skill names, plus agents, commands and output-styles. Not the skills directory
# itself: that directory also holds local skills, so it is never one link.
# Does not walk real directories such as projects/, and does not walk through a link.
each_managed_link() {
  local path name
  for name in agents commands output-styles; do
    path="$CLAUDE_HOME/$name"
    if is_link "$path"; then
      printf '%s\n' "$path"
    fi
  done
  path="$CLAUDE_HOME/skills"
  if [ -d "$path" ] && [ ! -L "$path" ]; then
    for path in "$CLAUDE_HOME/skills"/*; do
      [ -e "$path" ] || [ -L "$path" ] || continue
      if is_link "$path"; then
        printf '%s\n' "$path"
      fi
    done
  fi
}

# Managed links, plus any other direct child of ~/.claude that is a link.
# A dangling link inside an unrelated real directory is left alone.
each_dangling_candidate() {
  local path name
  each_managed_link
  for path in "$CLAUDE_HOME"/*; do
    [ -e "$path" ] || [ -L "$path" ] || continue
    name=$(basename -- "$path")
    case "$name" in
      agents|commands|output-styles) continue ;;
    esac
    if is_link "$path"; then
      printf '%s\n' "$path"
    fi
  done
}

cleanup_dangling() {
  local path
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    is_link "$path" || continue
    link_points_inside "$path" || continue
    if target_exists "$path"; then
      continue
    fi
    remove_dir_link "$path"
    note "removed dangling link ${path#"$CLAUDE_HOME"/}."
  done <<EOF
$(each_dangling_candidate)
EOF
}

remove_our_links() {
  local path
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    is_link "$path" || continue
    link_points_inside "$path" || continue
    remove_dir_link "$path"
    note "unlinked ${path#"$CLAUDE_HOME"/}."
  done <<EOF
$(each_managed_link)
EOF
}

rel_name() {
  local path="$1"
  printf '%s' "${path#"$CLAUDE_HOME"/}"
}

link_one() {
  local link="$1" target="$2" current
  target=$(existing_dir "$target")
  if is_link "$link"; then
    current=$(link_target "$link" || true)
    if [ -n "$current" ] && paths_equal "$current" "$target" && target_exists "$link"; then
      return 0
    fi
    note "conflict: $(rel_name "$link") points somewhere else. Left it unchanged. Move it aside, then run claude-sync link again."
    LINK_CONFLICTS=$((LINK_CONFLICTS + 1))
    return 0
  fi
  if [ -e "$link" ]; then
    if [ -d "$link" ]; then
      note "conflict: $(rel_name "$link") is a directory. Left it unchanged. Move it aside, then run claude-sync link again."
    else
      note "conflict: $(rel_name "$link") is a file. Left it unchanged. Move it aside, then run claude-sync link again."
    fi
    LINK_CONFLICTS=$((LINK_CONFLICTS + 1))
    return 0
  fi
  make_dir_link "$link" "$target"
  note "linked $(rel_name "$link")."
}

link_skills() {
  local repo_skills="$CLAUDE_SYNC_DIR/skills" local_skills="$CLAUDE_HOME/skills" dir name
  [ -d "$repo_skills" ] || return 0
  if [ -L "$local_skills" ] || { [ -e "$local_skills" ] && [ ! -d "$local_skills" ]; }; then
    note "conflict: skills is not a real directory. Left it unchanged. Move it aside, then run claude-sync link again."
    LINK_CONFLICTS=$((LINK_CONFLICTS + 1))
    return 0
  fi
  mkdir -p "$local_skills"
  for dir in "$repo_skills"/*/; do
    [ -d "$dir" ] || continue
    name=$(basename -- "$dir")
    [ "$name" = "synced" ] && continue
    link_one "$local_skills/$name" "$dir"
  done
}

link_whole_dirs() {
  local name repo
  for name in agents commands output-styles; do
    repo="$CLAUDE_SYNC_DIR/$name"
    [ -d "$repo" ] || continue
    link_one "$CLAUDE_HOME/$name" "$repo"
  done
}

cmd_link() {
  LINK_CONFLICTS=0
  require_repo_dir
  if [ -e "$CLAUDE_HOME" ] && [ ! -d "$CLAUDE_HOME" ]; then
    die 2 "CLAUDE_HOME is not a directory: $CLAUDE_HOME"
  fi
  mkdir -p "$CLAUDE_HOME" || die 3 "could not create $CLAUDE_HOME."
  if ! sync_claude_block; then
    LINK_CONFLICTS=$((LINK_CONFLICTS + 1))
  fi
  cleanup_dangling
  link_skills
  link_whole_dirs
  if [ "$LINK_CONFLICTS" -gt 0 ]; then
    die 4 "$LINK_CONFLICTS conflict(s). Local files were left unchanged. Move them aside, then run claude-sync link again."
  fi
}

cmd_unlink() {
  require_repo_dir
  remove_our_links
  if ! remove_claude_block; then
    exit 4
  fi
}

item_state() {
  local link="$1" target="$2" current
  if [ ! -d "$target" ]; then
    printf 'missing'
    return 0
  fi
  if [ ! -e "$link" ] && [ ! -L "$link" ]; then
    printf 'missing'
    return 0
  fi
  if is_link "$link"; then
    current=$(link_target "$link" || true)
    if [ -n "$current" ] && paths_equal "$current" "$target" && target_exists "$link"; then
      printf 'linked'
      return 0
    fi
    printf 'conflict'
    return 0
  fi
  printf 'conflict'
}

claude_md_state() {
  local file="$CLAUDE_HOME/CLAUDE.md" got want
  if [ -L "$file" ] || [ -d "$file" ]; then
    printf 'conflict'
    return 0
  fi
  if [ ! -f "$file" ]; then
    printf 'missing'
    return 0
  fi
  if ! first_marker_line "$file" "$BLOCK_BEGIN" >/dev/null; then
    printf 'missing'
    return 0
  fi
  got=$(block_import_line "$file" || true)
  want=$(import_line)
  if [ -n "$got" ] && [ "$got" = "$want" ]; then
    printf 'linked'
    return 0
  fi
  printf 'conflict'
}

status_items() {
  local dir name target
  printf 'CLAUDE.md: %s\n' "$(claude_md_state)"
  if [ -d "$CLAUDE_SYNC_DIR/skills" ]; then
    for dir in "$CLAUDE_SYNC_DIR/skills"/*/; do
      [ -d "$dir" ] || continue
      name=$(basename -- "$dir")
      [ "$name" = "synced" ] && continue
      printf '%s\n' "$name"
    done | sort | while IFS= read -r name; do
      [ -n "$name" ] || continue
      target=$(existing_dir "$CLAUDE_SYNC_DIR/skills/$name")
      printf 'skills/%s: %s\n' "$name" "$(item_state "$CLAUDE_HOME/skills/$name" "$target")"
    done
  fi
  for name in agents commands output-styles; do
    if [ -d "$CLAUDE_SYNC_DIR/$name" ]; then
      target=$(existing_dir "$CLAUDE_SYNC_DIR/$name")
    else
      target=""
    fi
    printf '%s: %s\n' "$name" "$(item_state "$CLAUDE_HOME/$name" "$target")"
  done
}

read_stamp() {
  local line
  if [ -f "$STATE_DIR/$1" ]; then
    IFS= read -r line < "$STATE_DIR/$1" || true
    line=${line%$'\r'}
    if [ -n "$line" ]; then
      printf '%s' "$line"
      return 0
    fi
  fi
  printf 'never'
}

status_repo() {
  local branch counts ahead behind dirty has_head
  if ! git -C "$CLAUDE_SYNC_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    printf 'branch: none\nahead: none\nbehind: none\ndirty: none\n'
    printf 'last-pull: %s\n' "$(read_stamp last-pull)"
    printf 'last-push: %s\n' "$(read_stamp last-push)"
    die 2 "no repo at $CLAUDE_SYNC_DIR. Set CLAUDE_SYNC_DIR to your config repo."
  fi
  if git -C "$CLAUDE_SYNC_DIR" rev-parse --verify -q HEAD >/dev/null 2>&1; then
    has_head=1
  else
    has_head=0
  fi
  branch=$(git -C "$CLAUDE_SYNC_DIR" symbolic-ref --short -q HEAD || true)
  branch=${branch%$'\r'}
  if [ -z "$branch" ]; then
    branch=detached
  fi
  if [ "$has_head" -eq 0 ]; then
    ahead=none
    behind=none
  elif git -C "$CLAUDE_SYNC_DIR" rev-parse --abbrev-ref '@{upstream}' >/dev/null 2>&1; then
    counts=$(git -C "$CLAUDE_SYNC_DIR" rev-list --left-right --count 'HEAD...@{upstream}') || die 3 "could not read how far $CLAUDE_SYNC_DIR is from upstream."
    ahead=${counts%%$'\t'*}
    behind=${counts#*$'\t'}
    ahead=${ahead%$'\r'}
    behind=${behind%$'\r'}
  else
    ahead=no-upstream
    behind=no-upstream
  fi
  if [ -n "$(git -C "$CLAUDE_SYNC_DIR" status --porcelain)" ]; then
    dirty=yes
  else
    dirty=no
  fi
  printf 'branch: %s\n' "$branch"
  printf 'ahead: %s\n' "$ahead"
  printf 'behind: %s\n' "$behind"
  printf 'dirty: %s\n' "$dirty"
  printf 'last-pull: %s\n' "$(read_stamp last-pull)"
  printf 'last-push: %s\n' "$(read_stamp last-push)"
}

cmd_status() {
  require_repo_dir
  prepare_git
  status_items
  status_repo
}
