#!/usr/bin/env bash
# Hands off to the next phase after a phase pull request is merged. Starts a new
# headless Cursor CLI session (agent -p) that runs /phase with a fresh context,
# so each phase reads only what it needs. That session hands off the same way, so
# phases run one at a time until one stops (Needs user, Blocked, a failed review,
# the fix-round cap) or every phase is completed.
#
# Usage: bash .cursor/skills/phase/next-phase.sh
# Env:   PHASE_AUTOCHAIN=0      do not start the next session
#        PHASE_AGENT_MODEL=<id> model for the next session (default: the CLI's, Auto)
#        PHASE_CHAIN_MAX=<n>    most sessions in one chain (default 30)
# Needs: the Cursor CLI (agent), logged in. Logs go to <git dir>/phase-runs/.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
common=$(cd "$(git rev-parse --git-common-dir)" && pwd)

# PROGRESS.md on origin/main, or the local file in repos that keep it out of git.
git fetch -q origin main 2>/dev/null || true
progress=$(git show origin/main:PROGRESS.md 2>/dev/null) || {
	if [ ! -f "$root/PROGRESS.md" ]; then
		echo "No PROGRESS.md on origin/main or in this checkout" >&2
		exit 2
	fi
	progress=$(< "$root/PROGRESS.md")
}

# Phase rows look like: | 7 | `phase-07-slug.plan.md` | not started | ... |
remaining=$(printf '%s\n' "$progress" | tr -d '\r' | awk -F'|' '
	$2 ~ /^ *[0-9]+ *$/ && $3 ~ /phase-[0-9]+-/ {
		status = $4
		gsub(/^ +| +$/, "", status)
		if (status != "completed") {
			n = $2
			gsub(/ /, "", n)
			printf "%s%s", sep, n
			sep = " "
		}
	}')

if [ -z "$remaining" ]; then
	echo "NEXT: none. Every phase in PROGRESS.md is completed."
	exit 0
fi

manual="Run /phase in a new chat to continue."
if [ "${PHASE_AUTOCHAIN:-1}" = "0" ]; then
	echo "NEXT: phases not completed: $remaining. PHASE_AUTOCHAIN=0, so nothing was started. $manual"
	exit 0
fi

depth=$(( ${PHASE_CHAIN_DEPTH:-0} + 1 ))
if [ "$depth" -gt "${PHASE_CHAIN_MAX:-30}" ]; then
	echo "NEXT: chain limit reached ($depth sessions). Phases not completed: $remaining. $manual"
	exit 0
fi

agent_bin=""
for candidate in agent agent.cmd cursor-agent "$HOME/.local/bin/agent" "${LOCALAPPDATA:-}/cursor-agent/agent.cmd"; do
	if command -v "$candidate" >/dev/null 2>&1; then
		agent_bin=$(command -v "$candidate")
		break
	fi
done
if [ -z "$agent_bin" ]; then
	echo "NEXT: the Cursor CLI (agent) is not installed. Phases not completed: $remaining. $manual"
	exit 0
fi
if "$agent_bin" status 2>&1 | grep -qi 'not logged in'; then
	echo "NEXT: the Cursor CLI is not logged in (run: agent login). Phases not completed: $remaining. $manual"
	exit 0
fi

runs="$common/phase-runs"
mkdir -p "$runs"
stamp=$(date +%Y%m%d-%H%M%S)
log="$runs/$stamp-next.log"
model_args=()
if [ -n "${PHASE_AGENT_MODEL:-}" ]; then
	model_args=(--model "$PHASE_AGENT_MODEL")
fi
# One line, no quotes or shell metacharacters: it is passed through PowerShell on Windows.
prompt='/phase - Run the phase skill with no argument: read .cursor/skills/phase/SKILL.md and follow it exactly. It picks the next phase from PROGRESS.md on origin/main. You are a headless session started after the previous phase merged and nobody is watching: where the skill says to stop and tell the user, write that in your final message and stop.'

case "$(uname -s)" in
	MINGW* | MSYS* | CYGWIN*)
		# PowerShell runner: when the CLI starts from Git Bash it runs Cursor hooks
		# through bash with PowerShell syntax, and every hook call fails.
		psq() { local s=${1//\'/\'\'}; printf "'%s'" "$s"; }
		agent_ps1="$(dirname "$agent_bin")/agent.ps1"
		[ -f "$agent_ps1" ] || agent_ps1="$agent_bin"
		runner="$runs/$stamp-next.ps1"
		{
			echo "\$env:PHASE_CHAIN_DEPTH = '$depth'"
			echo "foreach (\$v in 'SHELL','MSYSTEM','MINGW_PREFIX','OSTYPE') { Remove-Item \"Env:\$v\" -ErrorAction SilentlyContinue }"
			echo "Set-Location -LiteralPath $(psq "$(cygpath -w "$root")")"
			echo "\$log = $(psq "$(cygpath -w "$log")")"
			echo "\"# \$(Get-Date -Format o) phase session $depth, phases not completed: $remaining\" | Out-File -Encoding utf8 \$log"
			printf '& %s -p --force --trust --output-format text --workspace %s' "$(psq "$(cygpath -w "$agent_ps1")")" "$(psq "$(cygpath -w "$root")")"
			for a in ${model_args[@]+"${model_args[@]}"}; do printf ' %s' "$(psq "$a")"; done
			printf ' %s 2>&1 | Out-File -Append -Encoding utf8 $log\n' "$(psq "$prompt")"
			echo "\"# \$(Get-Date -Format o) exit \$LASTEXITCODE\" | Out-File -Append -Encoding utf8 \$log"
		} > "$runner"
		powershell.exe -NoProfile -Command "Start-Process powershell.exe -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','$(cygpath -w "$runner")'"
		;;
	*)
		runner="$runs/$stamp-next.sh"
		{
			echo '#!/usr/bin/env bash'
			printf 'export PHASE_CHAIN_DEPTH=%q\n' "$depth"
			printf 'cd %q || exit 1\n' "$root"
			printf 'echo "# $(date -u +%%FT%%TZ) phase session %s, phases not completed: %s" > %q\n' "$depth" "$remaining" "$log"
			printf '%q -p --force --trust --output-format text --workspace %q' "$agent_bin" "$root"
			for a in ${model_args[@]+"${model_args[@]}"}; do printf ' %q' "$a"; done
			printf ' %q >> %q 2>&1 < /dev/null\n' "$prompt" "$log"
			printf 'echo "# $(date -u +%%FT%%TZ) exit $?" >> %q\n' "$log"
		} > "$runner"
		nohup bash "$runner" > /dev/null 2>&1 < /dev/null &
		disown "$!" 2>/dev/null || true
		;;
esac

echo "NEXT: started the next phase in a new headless Cursor CLI session (phases not completed: $remaining). Log: $log"
