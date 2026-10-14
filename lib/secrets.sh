# shellcheck shell=bash
# Sourced by lib/sync.sh. Scans the staged diff for SPEC.md section 7 patterns.
# Prints one "file:line:pattern-name" per hit. Never prints the matched text.

secret_add() {
  SECRET_NAME[${#SECRET_NAME[@]}]=$1
  SECRET_REGEX[${#SECRET_REGEX[@]}]=$2
}

SECRET_NAME=()
SECRET_REGEX=()
# Longer, more specific patterns first so a github_pat is not reported as something else.
secret_add github_pat 'github_pat_[A-Za-z0-9_]{50,}'
secret_add sk-ant 'sk-ant-[A-Za-z0-9_-]{20,}'
secret_add ghp 'ghp_[A-Za-z0-9]{36}'
secret_add gh-token 'gh[orsu]_[A-Za-z0-9]{36}'
secret_add aws-access-key 'AKIA[0-9A-Z]{16}'
secret_add slack-token 'xox[abpr]-[A-Za-z0-9-]{10,}'
secret_add private-key '-----BEGIN[[:space:]][A-Z[:space:]]*PRIVATE[[:space:]]KEY-----'
secret_add token-field '"(access|refresh)_token"[[:space:]]*:'

# True when the added line matches one pattern. The name is printed by the caller.
secret_match() {
  local content="$1"
  local regex="$2"
  # bash 3.2 word-splits an unquoted =~ operand. An empty IFS keeps the pattern whole.
  local IFS=
  [[ "$content" =~ $regex ]]
}

# scan_staged DIR
# Reads the staged unified diff and writes hits to stdout.
scan_staged() {
  local dir="$1"
  local line file="" in_hunk=0 start=0 seen=0 current=0 content
  local i regex name
  local diff_text diff_status=0

  # Pin prefixes so diff.noprefix / diff.mnemonicPrefix cannot hide a path.
  # git diff exits 1 when the diff is non-empty. Only a higher status is an error.
  diff_text=$(git -c diff.noprefix=false -c diff.mnemonicPrefix=false -c core.quotePath=false \
    -C "$dir" diff --cached --src-prefix=a/ --dst-prefix=b/ \
    --unified=0 --no-color --no-ext-diff --no-textconv --text) || diff_status=$?
  if [ "$diff_status" -gt 1 ]; then
    return 1
  fi

  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      'diff --git '*)
        in_hunk=0
        file=""
        ;;
      '@@ '*)
        in_hunk=1
        seen=0
        start=${line#*+}
        start=${start%%[!0-9]*}
        case "$start" in
          ''|*[!0-9]*) start=0 ;;
        esac
        ;;
      *)
        if [ "$in_hunk" -eq 0 ]; then
          case "$line" in
            '+++ b/'*)
              file=${line#+++ b/}
              file=${file%$'\t'}
              ;;
            '+++ "b/'*)
              file=${line#+++ \"b/}
              file=${file%$'\t'}
              file=${file%\"}
              ;;
            '+++ '*)
              file=""
              ;;
          esac
          continue
        fi
        case "$line" in
          '+'*)
            # An added line we cannot attach to a path is a scan failure.
            # Skipping it would let a secret through.
            if [ -z "$file" ]; then
              return 1
            fi
            content=${line#+}
            if [ "$seen" -eq 0 ]; then
              current=$start
            else
              current=$((current + 1))
            fi
            seen=$((seen + 1))
            case "$content" in
              *"claude-sync: allow-secret"*)
                continue
                ;;
            esac
            i=0
            while [ "$i" -lt ${#SECRET_NAME[@]} ]; do
              regex=${SECRET_REGEX[$i]}
              name=${SECRET_NAME[$i]}
              if secret_match "$content" "$regex"; then
                printf '%s:%s:%s\n' "$file" "$current" "$name"
                break
              fi
              i=$((i + 1))
            done
            ;;
          ' '*)
            if [ "$seen" -eq 0 ]; then
              current=$start
            else
              current=$((current + 1))
            fi
            seen=$((seen + 1))
            ;;
        esac
        ;;
    esac
  done <<EOF
$diff_text
EOF
}
