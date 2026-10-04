# shellcheck shell=bash
# Sourced by bin/claude-sync. init, join, backup, and import.
# May call sync and link. Does not run network git itself (clone, ls-remote, push).

INIT_TEMPLATES=$(cd "$(dirname "${BASH_SOURCE[0]}")/../templates/user-repo" && pwd)

# Backup directory names cannot contain ':' on Windows, so the UTC stamp has none.
backup_stamp() {
  date -u +%Y%m%dT%H%M%SZ
}

repo_is_git() {
  [ -d "$CLAUDE_SYNC_DIR/.git" ] || [ -f "$CLAUDE_SYNC_DIR/.git" ]
}

begin_setup() {
  reset_sync_state
  IMPORTED=""
  LINKED=""
  BACKUP_DIR=""
  DID_COMMIT=0
  LINK_CREATED_CLAUDE=0
  INIT_CREATED_REPO=0
  COMMIT_TEMPLATES=0
  SETUP_DONE=0
  UNDONE=0
  LINK_ATTEMPTED=0
  SETUP_HOST=$(uname -n || true)
  [ -n "$SETUP_HOST" ] || SETUP_HOST=unknown
  SETUP_TMP=$(mktemp -d "${TMPDIR:-/tmp}/claude-sync.XXXXXX")
  UNDO_FILE=$SETUP_TMP/undo
  : > "$UNDO_FILE"
  # Until setup finishes, a failed command or a signal puts moved files back.
  trap 'on_setup_abort $?' ERR
  trap 'on_setup_abort 130' INT
  trap 'on_setup_abort 143' TERM
  trap finish_setup EXIT
  prepare_git
  validate_timeout
  if ! acquire_lock; then
    die 2 "another sync is running. Wait for it to finish, then try again."
  fi
}

new_tmp() {
  mktemp "$SETUP_TMP/tmp.XXXXXX"
}

on_setup_abort() {
  local status=$1
  trap - ERR INT TERM
  exit "$status"
}

finish_setup() {
  local status=$?
  trap - ERR INT TERM EXIT
  if [ "${SETUP_DONE:-0}" -ne 1 ] && [ "${UNDONE:-0}" -eq 0 ]; then
    undo_import || true
  fi
  if [ -n "${SETUP_TMP:-}" ] && [ -d "$SETUP_TMP" ]; then
    rm -rf -- "$SETUP_TMP"
  fi
  restore_outside_staged || true
  release_lock || true
  exit "$status"
}

# Drop a home path. A link or junction is removed as a link, never with rm -rf.
clear_home_item() {
  local path="$1"
  if [ ! -e "$path" ] && [ ! -L "$path" ]; then
    return 0
  fi
  if is_link "$path"; then
    remove_dir_link "$path"
    return 0
  fi
  rm -rf -- "$path"
}

remove_created_repo() {
  local dir="${CLAUDE_SYNC_DIR:-}"
  [ "${INIT_CREATED_REPO:-0}" -eq 1 ] || return 0
  [ -n "$dir" ] && [ "$dir" != "/" ] || return 0
  if is_link "$dir"; then
    return 0
  fi
  rm -rf -- "$dir"
  INIT_CREATED_REPO=0
}

remember() {
  local which="$1" val="$2" cur=""
  case "$which" in
    imported) cur=$IMPORTED ;;
    linked) cur=$LINKED ;;
    *) return 1 ;;
  esac
  if [ -z "$cur" ]; then
    cur=$val
  else
    cur="$cur $val"
  fi
  case "$which" in
    imported) IMPORTED=$cur ;;
    linked) LINKED=$cur ;;
  esac
}

record_undo() {
  printf '%s\t%s\n' "$1" "$2" >> "$UNDO_FILE"
}

ensure_backup() {
  local base="" n=0
  if [ -n "$BACKUP_DIR" ]; then
    return 0
  fi
  base="$CLAUDE_HOME/.claude-sync-backup/$(backup_stamp)"
  BACKUP_DIR=$base
  while [ -e "$BACKUP_DIR" ]; do
    n=$((n + 1))
    BACKUP_DIR="${base}-${n}"
  done
  mkdir -p -- "$BACKUP_DIR"
}

backup_item() {
  local rel="$1"
  local src="$CLAUDE_HOME/$rel"
  local dest=""
  ensure_backup
  dest="$BACKUP_DIR/$rel"
  mkdir -p -- "$(dirname -- "$dest")"
  rm -rf -- "$dest"
  if [ -d "$src" ] && ! is_link "$src"; then
    cp -R "$src" "$dest"
  else
    cp "$src" "$dest"
  fi
}

remove_real_tree() {
  local path="$1"
  if is_link "$path"; then
    die 3 "refusing to remove a link at $path."
  fi
  rm -rf -- "$path"
}

# Write the CLAUDE.md body, outside any managed block, to dest.
claude_body_to() {
  local file="$1" dest="$2" begin="" end="" num=0 line=""
  if [ ! -f "$file" ] || [ -L "$file" ] || [ -d "$file" ]; then
    return 1
  fi
  begin=$(first_marker_line "$file" "$BLOCK_BEGIN" || true)
  end=$(first_marker_line "$file" "$BLOCK_END" || true)
  if [ -z "$begin" ] || [ -z "$end" ] || [ "$end" -le "$begin" ]; then
    cp "$file" "$dest"
    return 0
  fi
  : > "$dest"
  while IFS= read -r line || [ -n "$line" ]; do
    num=$((num + 1))
    if [ "$num" -ge "$begin" ] && [ "$num" -le "$end" ]; then
      continue
    fi
    printf '%s\n' "$line" >> "$dest"
  done < "$file"
}

claude_has_body() {
  local tmp="" has=1
  [ -f "$CLAUDE_HOME/CLAUDE.md" ] || return 1
  [ -L "$CLAUDE_HOME/CLAUDE.md" ] && return 1
  [ -d "$CLAUDE_HOME/CLAUDE.md" ] && return 1
  tmp=$(new_tmp)
  if ! claude_body_to "$CLAUDE_HOME/CLAUDE.md" "$tmp"; then
    rm -f "$tmp"
    return 1
  fi
  if grep -q '[^[:space:]]' "$tmp"; then
    has=0
  fi
  rm -f "$tmp"
  return "$has"
}

each_local_real() {
  local name="" path=""
  if claude_has_body; then
    printf '%s\n' "CLAUDE.md"
  fi
  path="$CLAUDE_HOME/skills"
  if [ -d "$path" ] && ! is_link "$path"; then
    for path in "$CLAUDE_HOME/skills"/*; do
      [ -d "$path" ] || continue
      is_link "$path" && continue
      name=$(basename -- "$path")
      [ "$name" = "synced" ] && continue
      printf '%s\n' "skills/$name"
    done
  fi
  for name in agents commands output-styles; do
    path="$CLAUDE_HOME/$name"
    if [ -d "$path" ] && ! is_link "$path"; then
      printf '%s\n' "$name"
    fi
  done
}

undo_import() {
  local kind="" rel="" restored_claude=0 restore_failed=0
  [ "${UNDONE:-0}" -eq 1 ] && return 0
  UNDONE=1
  # A second init has an empty undo file and has not linked. Do not touch the
  # setup that is already on this machine.
  if [ -s "${UNDO_FILE:-}" ] || [ "${LINK_ATTEMPTED:-0}" -eq 1 ]; then
    remove_our_links || restore_failed=1
    if [ -s "${UNDO_FILE:-}" ]; then
      while IFS=$'\t' read -r kind rel || [ -n "$kind" ]; do
        [ -n "$kind" ] || continue
        case "$kind" in
          C|A|L) [ "$rel" = "CLAUDE.md" ] && restored_claude=1 ;;
        esac
        case "$kind" in
          M)
            clear_home_item "$CLAUDE_HOME/$rel" || restore_failed=1
            if [ -e "$CLAUDE_SYNC_DIR/$rel" ]; then
              mkdir -p -- "$(dirname -- "$CLAUDE_HOME/$rel")"
              mv -- "$CLAUDE_SYNC_DIR/$rel" "$CLAUDE_HOME/$rel" || restore_failed=1
            fi
            ;;
          C)
            if [ -n "$BACKUP_DIR" ] && [ -f "$BACKUP_DIR/$rel" ]; then
              if cp "$BACKUP_DIR/$rel" "$CLAUDE_HOME/$rel"; then
                rm -f -- "$CLAUDE_SYNC_DIR/$rel"
              else
                restore_failed=1
              fi
            fi
            ;;
          A)
            git -C "$CLAUDE_SYNC_DIR" checkout -q -- "$rel" 2>/dev/null || true
            if [ -n "$BACKUP_DIR" ] && [ -f "$BACKUP_DIR/$rel" ]; then
              cp "$BACKUP_DIR/$rel" "$CLAUDE_HOME/$rel" || restore_failed=1
            fi
            ;;
          L)
            if [ -n "$BACKUP_DIR" ] && [ -e "$BACKUP_DIR/$rel" ]; then
              clear_home_item "$CLAUDE_HOME/$rel" || restore_failed=1
              if [ -d "$BACKUP_DIR/$rel" ]; then
                cp -R "$BACKUP_DIR/$rel" "$CLAUDE_HOME/$rel" || restore_failed=1
              else
                cp "$BACKUP_DIR/$rel" "$CLAUDE_HOME/$rel" || restore_failed=1
              fi
            fi
            ;;
          X)
            if [ -n "$BACKUP_DIR" ] && [ -e "$BACKUP_DIR/$rel" ]; then
              mkdir -p -- "$(dirname -- "$CLAUDE_HOME/$rel")"
              clear_home_item "$CLAUDE_HOME/$rel" || restore_failed=1
              cp -R "$BACKUP_DIR/$rel" "$CLAUDE_HOME/$rel" || restore_failed=1
            fi
            ;;
        esac
      done < "$UNDO_FILE"
    fi
    if [ "${LINK_CREATED_CLAUDE:-0}" -eq 1 ] && [ "$restored_claude" -eq 0 ]; then
      rm -f -- "$CLAUDE_HOME/CLAUDE.md" || restore_failed=1
    fi
    if repo_is_git; then
      reset_index || true
    fi
  fi
  if [ "$restore_failed" -eq 1 ]; then
    note "could not put every file back. Backup: ${BACKUP_DIR:-none}. Repo still at $CLAUDE_SYNC_DIR."
    return 1
  fi
  remove_created_repo
  return 0
}

import_claude_new() {
  local body=""
  body=$(new_tmp)
  claude_body_to "$CLAUDE_HOME/CLAUDE.md" "$body"
  backup_item CLAUDE.md
  record_undo C CLAUDE.md
  cp "$body" "$CLAUDE_SYNC_DIR/CLAUDE.md"
  : > "$CLAUDE_HOME/CLAUDE.md"
  remember imported CLAUDE.md
}

append_claude() {
  local body="" tmp=""
  body=$(new_tmp)
  tmp=$(new_tmp)
  claude_body_to "$CLAUDE_HOME/CLAUDE.md" "$body"
  backup_item CLAUDE.md
  record_undo A CLAUDE.md
  cp "$CLAUDE_SYNC_DIR/CLAUDE.md" "$tmp"
  printf '\n## From %s\n' "$SETUP_HOST" >> "$tmp"
  cat "$body" >> "$tmp"
  mv -- "$tmp" "$CLAUDE_SYNC_DIR/CLAUDE.md"
  : > "$CLAUDE_HOME/CLAUDE.md"
  remember imported CLAUDE.md
}

import_move() {
  local rel="$1"
  local dest="$CLAUDE_SYNC_DIR/$rel"
  backup_item "$rel"
  mkdir -p -- "$(dirname -- "$dest")"
  mv -- "$CLAUDE_HOME/$rel" "$dest"
  record_undo M "$rel"
  remember imported "$rel"
}

drop_local() {
  local rel="$1"
  backup_item "$rel"
  if [ "$rel" = "CLAUDE.md" ]; then
    : > "$CLAUDE_HOME/CLAUDE.md"
    record_undo L CLAUDE.md
  else
    remove_real_tree "$CLAUDE_HOME/$rel"
    record_undo X "$rel"
  fi
}

import_one() {
  local rel="$1"
  case "$rel" in
    CLAUDE.md) import_claude_new ;;
    *) import_move "$rel" ;;
  esac
}

merge_one() {
  local rel="$1"
  case "$rel" in
    CLAUDE.md)
      if [ -f "$CLAUDE_SYNC_DIR/CLAUDE.md" ]; then
        append_claude
      else
        import_claude_new
      fi
      ;;
    *)
      if [ -e "$CLAUDE_SYNC_DIR/$rel" ]; then
        drop_local "$rel"
      else
        import_move "$rel"
      fi
      ;;
  esac
}

apply_local() {
  local action="$1" rel=""
  while IFS= read -r rel || [ -n "$rel" ]; do
    [ -n "$rel" ] || continue
    case "$action" in
      import) import_one "$rel" ;;
      merge) merge_one "$rel" ;;
      replace) drop_local "$rel" ;;
    esac
  done <<EOF
$(each_local_real)
EOF
}

collect_linked() {
  local name="" path=""
  LINKED=""
  if [ -f "$CLAUDE_HOME/CLAUDE.md" ] && first_marker_line "$CLAUDE_HOME/CLAUDE.md" "$BLOCK_BEGIN" >/dev/null; then
    remember linked CLAUDE.md
  fi
  path="$CLAUDE_HOME/skills"
  if [ -d "$path" ] && ! is_link "$path"; then
    for path in "$CLAUDE_HOME/skills"/*; do
      [ -e "$path" ] || [ -L "$path" ] || continue
      is_link "$path" || continue
      remember linked "skills/$(basename -- "$path")"
    done
  fi
  for name in agents commands output-styles; do
    if is_link "$CLAUDE_HOME/$name"; then
      remember linked "$name"
    fi
  done
}

link_imported() {
  local failed=0 had_claude=0
  LINK_ATTEMPTED=1
  LINK_CONFLICTS=0
  if [ -e "$CLAUDE_HOME/CLAUDE.md" ]; then
    had_claude=1
  fi
  if [ -e "$CLAUDE_HOME" ] && [ ! -d "$CLAUDE_HOME" ]; then
    undo_import
    die 2 "CLAUDE_HOME is not a directory: $CLAUDE_HOME"
  fi
  mkdir -p -- "$CLAUDE_HOME" || {
    undo_import
    die 3 "could not create $CLAUDE_HOME."
  }
  if ! sync_claude_block; then
    failed=1
  fi
  if [ "$had_claude" -eq 0 ] && [ -f "$CLAUDE_HOME/CLAUDE.md" ]; then
    LINK_CREATED_CLAUDE=1
  fi
  cleanup_dangling
  link_skills
  link_whole_dirs
  if [ "$LINK_CONFLICTS" -gt 0 ] || [ "$failed" -eq 1 ]; then
    if undo_import; then
      die 4 "link conflict during setup. Local files were restored. Move the conflicting path aside and try again."
    fi
    die 4 "link conflict during setup. Some files could not be put back. Move the conflicting path aside and try again."
  fi
  collect_linked
}

commit_imported() {
  local message="$1" hits="" first="" scan_status=0 f=""
  if [ "${COMMIT_TEMPLATES:-0}" -eq 1 ]; then
    for f in .gitignore .gitattributes README.md; do
      if [ -f "$CLAUDE_SYNC_DIR/$f" ]; then
        git -C "$CLAUDE_SYNC_DIR" add -- "$f"
      fi
    done
  fi
  stage_allowlist
  if git -C "$CLAUDE_SYNC_DIR" diff --cached --quiet; then
    return 0
  fi
  hits=$(scan_staged "$CLAUDE_SYNC_DIR") || scan_status=$?
  if [ "$scan_status" -ne 0 ]; then
    if undo_import; then
      die 3 "secret scan could not read the staged diff. Local files were restored. Nothing was committed."
    fi
    die 3 "secret scan could not read the staged diff. Some files could not be put back. Nothing was committed."
  fi
  if [ -n "$hits" ]; then
    first=$hits
    case "$hits" in
      *$'\n'*) first=${hits%%$'\n'*} ;;
    esac
    if undo_import; then
      note "local files were restored."
    fi
    report_secret_hits "$hits" "$first"
  fi
  if ! git -C "$CLAUDE_SYNC_DIR" -c commit.gpgsign=false commit -m "$message" >/dev/null 2>&1; then
    if undo_import; then
      die 3 "commit failed in $CLAUDE_SYNC_DIR. Local files were restored. Set git user.name and user.email, then try again."
    fi
    die 3 "commit failed in $CLAUDE_SYNC_DIR. Some files could not be put back. Set git user.name and user.email, then try again."
  fi
  DID_COMMIT=1
}

print_summary() {
  local pushed="$1" imported="none" backed="none" linked="none"
  [ -n "$IMPORTED" ] && imported=$IMPORTED
  [ -n "$BACKUP_DIR" ] && backed=$BACKUP_DIR
  [ -n "$LINKED" ] && linked=$LINKED
  printf 'imported: %s\n' "$imported"
  printf 'backed up: %s\n' "$backed"
  printf 'linked: %s\n' "$linked"
  printf 'pushed: %s\n' "$pushed"
}

write_templates() {
  local f=""
  for f in .gitignore .gitattributes README.md; do
    cp "$INIT_TEMPLATES/$f" "$CLAUDE_SYNC_DIR/$f"
  done
}

setup_new_repo() {
  local url="$1"
  mkdir -p -- "$CLAUDE_SYNC_DIR"
  INIT_CREATED_REPO=1
  git init -b main "$CLAUDE_SYNC_DIR" >/dev/null
  git -C "$CLAUDE_SYNC_DIR" config core.autocrlf false
  git -C "$CLAUDE_SYNC_DIR" config commit.gpgsign false
  if [ -n "$url" ]; then
    git -C "$CLAUDE_SYNC_DIR" remote add origin "$url"
  fi
  write_templates
}

warn_if_public() {
  local target="$1" vis="" status=0
  command -v gh >/dev/null 2>&1 || return 0
  vis=$(run_limited "$CLAUDE_SYNC_TIMEOUT" gh repo view "$target" --json visibility --jq .visibility) || status=$?
  [ "$status" -eq 0 ] || return 0
  case "$vis" in
    PUBLIC|public|Public)
      note "this repo is public. Make it private so this Claude setup is not world-readable."
      ;;
  esac
}

create_private_remote() {
  local name="$1" gh_out="" line="" url="" status=0
  command -v gh >/dev/null 2>&1 || die 2 "gh is required for --create. Install GitHub CLI and log in, or use --remote."
  gh_out=$(run_limited "$CLAUDE_SYNC_TIMEOUT" gh repo create "$name" --private) || status=$?
  if [ "$status" -eq 124 ]; then
    die 3 "gh repo create timed out after ${CLAUDE_SYNC_TIMEOUT}s. Check the network, then try again."
  fi
  if [ "$status" -ne 0 ]; then
    die 3 "gh repo create failed. Check that gh is logged in, then try again."
  fi
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      https://*|http://*|git@*|ssh://*|file://*)
        url=$line
        break
        ;;
    esac
  done <<EOF
$gh_out
EOF
  if [ -z "$url" ]; then
    die 3 "gh repo create did not print a repo URL. Nothing was set up."
  fi
  CREATED_URL=$url
}

finish_publish() {
  SETUP_DONE=1
  if [ "$DID_COMMIT" -eq 1 ]; then
    sync_publish
    print_summary yes
  else
    print_summary no
  fi
}

fold_url() {
  local u="$1"
  u=${u//%20/ }
  u=${u%/}
  printf '%s' "$u"
}

urls_match() {
  local a="" b=""
  a=$(fold_url "$1")
  b=$(fold_url "$2")
  [ "$a" = "$b" ]
}

list_conflicts_and_stop() {
  local rel=""
  while IFS= read -r rel || [ -n "$rel" ]; do
    [ -n "$rel" ] || continue
    printf 'conflict: %s\n' "$rel"
  done <<EOF
$(each_local_real)
EOF
  die 4 "local content conflicts. Nothing local was changed. Re-run with --merge or --replace."
}

require_fresh_dir() {
  if repo_is_git; then
    die 2 "already set up at $CLAUDE_SYNC_DIR. Nothing was changed."
  fi
  if [ -e "$CLAUDE_SYNC_DIR" ]; then
    die 2 "$CLAUDE_SYNC_DIR exists and is not a git repo. Nothing was changed."
  fi
}

cmd_init() {
  local create="" remote=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --create)
        shift
        [ $# -gt 0 ] || bad_args
        [ -n "$1" ] || bad_args
        [ -n "$create" ] && bad_args
        create=$1
        ;;
      --remote)
        shift
        [ $# -gt 0 ] || bad_args
        [ -n "$1" ] || bad_args
        [ -n "$remote" ] && bad_args
        remote=$1
        ;;
      --dir)
        shift
        [ $# -gt 0 ] || bad_args
        [ -n "$1" ] || bad_args
        CLAUDE_SYNC_DIR=$1
        export CLAUDE_SYNC_DIR
        ;;
      *)
        bad_args
        ;;
    esac
    shift
  done
  if [ -n "$create" ] && [ -n "$remote" ]; then
    bad_args
  fi
  begin_setup
  require_fresh_dir
  if [ -n "$create" ]; then
    create_private_remote "$create"
    remote=$CREATED_URL
  fi
  if [ -n "$remote" ]; then
    if ! sync_remote_empty "$remote"; then
      die 2 "remote already has commits. Use join for a repo that is already in use. Nothing was changed."
    fi
    warn_if_public "$remote"
  fi
  setup_new_repo "$remote"
  apply_local import
  link_imported
  COMMIT_TEMPLATES=1
  commit_imported "init: import from $SETUP_HOST"
  if [ -n "$remote" ]; then
    finish_publish
  else
    SETUP_DONE=1
    print_summary no
    printf 'no remote yet. Add one with: git -C "%s" remote add origin <url>\n' "$CLAUDE_SYNC_DIR"
  fi
}

cmd_join() {
  local url="" mode="" choice="" existing=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --merge)
        [ -n "$mode" ] && bad_args
        mode=merge
        ;;
      --replace)
        [ -n "$mode" ] && bad_args
        mode=replace
        ;;
      --dir)
        shift
        [ $# -gt 0 ] || bad_args
        [ -n "$1" ] || bad_args
        CLAUDE_SYNC_DIR=$1
        export CLAUDE_SYNC_DIR
        ;;
      -*)
        bad_args
        ;;
      *)
        [ -n "$url" ] && bad_args
        url=$1
        ;;
    esac
    shift
  done
  [ -n "$url" ] || bad_args
  begin_setup
  warn_if_public "$url"
  if repo_is_git; then
    existing=$(git -C "$CLAUDE_SYNC_DIR" remote get-url origin 2>/dev/null || true)
    if ! urls_match "$existing" "$url"; then
      die 2 "already set up at $CLAUDE_SYNC_DIR (origin $existing). Nothing was changed."
    fi
  elif [ -e "$CLAUDE_SYNC_DIR" ]; then
    die 2 "$CLAUDE_SYNC_DIR exists and is not a git repo. Nothing was changed."
  else
    sync_clone "$url" "$CLAUDE_SYNC_DIR"
  fi
  if claude_has_body || [ -n "$(each_local_real)" ]; then
    if [ -z "$mode" ]; then
      if [ -t 0 ]; then
        printf 'Choose merge, replace, or abort: ' >&2
        IFS= read -r choice || choice=""
        case "$choice" in
          merge|replace) mode=$choice ;;
          *) die 4 "aborted. Nothing local was changed." ;;
        esac
      fi
    fi
    if [ -z "$mode" ]; then
      list_conflicts_and_stop
    fi
    apply_local "$mode"
  fi
  link_imported
  if [ "$mode" = "merge" ]; then
    commit_imported "join: merge from $SETUP_HOST"
  fi
  finish_publish
}
