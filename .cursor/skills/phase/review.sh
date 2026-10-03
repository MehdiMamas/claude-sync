#!/usr/bin/env bash
# Runs the claude-sync phase review on this machine with the Claude Code CLI and
# posts the result as a comment on the pull request. Replaces the old
# "Claude Code Review" GitHub workflow, so no Actions minutes are used.
#
# Usage: bash .cursor/skills/phase/review.sh <pr-number>
#
# Needs: claude (logged in), gh (authenticated), and the PR's branch checked out
# at its pushed head commit.
set -euo pipefail

pr="${1:-}"
if ! [[ "$pr" =~ ^[0-9]+$ ]]; then
	echo "usage: bash .cursor/skills/phase/review.sh <pr-number>" >&2
	exit 2
fi

for tool in claude gh git; do
	command -v "$tool" >/dev/null || { echo "$tool is not on PATH" >&2; exit 2; }
done

repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
head_ref=$(gh pr view "$pr" --json headRefName --jq .headRefName)
head_sha=$(gh pr view "$pr" --json headRefOid --jq .headRefOid)
base_ref=$(gh pr view "$pr" --json baseRefName --jq .baseRefName)

num=$(printf '%s' "$head_ref" | sed -nE 's/^phase-0*([0-9]+)-.*/\1/p')
if [ -z "$num" ]; then
	echo "Branch $head_ref does not match phase-<NN>-<slug>" >&2
	exit 2
fi

# The reviewer reads local files, so they must be exactly what was pushed.
local_sha=$(git rev-parse HEAD)
if [ "$local_sha" != "$head_sha" ]; then
	echo "Local HEAD $local_sha is not the PR head $head_sha. Check out $head_ref and push or pull first." >&2
	exit 2
fi
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
	echo "Working tree has uncommitted changes. Commit and push them first." >&2
	exit 2
fi

# Read the instructions from the base branch so a PR cannot rewrite its own review.
git fetch -q origin "$base_ref"
instructions=$(git show "origin/$base_ref:.cursor/skills/phase/review-prompt.md") || {
	echo "origin/$base_ref has no .cursor/skills/phase/review-prompt.md" >&2
	exit 2
}

out_dir="$(git rev-parse --git-common-dir)/phase-review"
mkdir -p "$out_dir"
prompt_file="$out_dir/pr-$pr-$head_sha.prompt.md"
report_file="$out_dir/pr-$pr-$head_sha.md"

{
	printf '%s\n\n' "$instructions"
	echo '## This pull request'
	echo "- Repository: $repo"
	echo "- PR number: $pr"
	echo "- Head branch: $head_ref"
	echo "- Head commit: $head_sha (the local checkout is at this commit)"
	echo "- Base branch: origin/$base_ref"
	echo "- Phase number: $num"
} > "$prompt_file"

# Model ids live in ~/.claude/skills/phasing/review-models.env.
# The last numbered phase plan is the whole-product review.
selector="${PHASE_REVIEW_SELECTOR:-$HOME/.claude/skills/phasing/review-model.sh}"
addendum="${PHASE_FINAL_REVIEW_ADDENDUM:-$HOME/.claude/skills/phasing/final-review-addendum.md}"
if [ ! -f "$selector" ]; then
	echo "Missing $selector. That file chooses the review model for every project." >&2
	exit 2
fi
model_line=$(bash "$selector" "$num")
review_model=${model_line%%$'\t'*}
review_kind=${model_line#*$'\t'}
if [ -z "$review_model" ] || [ "$review_kind" = "$model_line" ]; then
	echo "review-model.sh did not return a model and a kind." >&2
	exit 2
fi
if [ "$review_kind" = "final" ] && [ -f "$addendum" ]; then
	printf '\n' >> "$prompt_file"
	cat "$addendum" >> "$prompt_file"
fi

echo "Reviewing PR #$pr ($head_ref @ ${head_sha:0:12}) with $review_model ($review_kind). This can take a while."

# Read-only tools: the review never edits files, comments or merges by itself.
claude -p \
	--model "$review_model" \
	--max-turns 200 \
	--output-format text \
	--allowedTools "Read,Glob,Grep,Bash(git diff:*),Bash(git log:*),Bash(git show:*),Bash(git rev-parse:*),Bash(git fetch:*),Bash(gh pr view:*),Bash(gh pr diff:*),Bash(gh api repos/$repo/pulls/$pr/comments:*),Bash(gh run list:*),Bash(gh run view:*)" \
	--disallowedTools "Edit,Write,NotebookEdit" \
	< "$prompt_file" > "$report_file"

# Keep only the report: from the first "## Phase" heading to the end.
report=$(sed -n '/^## Phase /,$p' "$report_file")
if [ -z "$report" ] || ! printf '%s' "$report" | grep -q "reviewed-sha: $head_sha"; then
	echo "Claude did not return a report for $head_sha. Raw output: $report_file" >&2
	exit 1
fi
printf '%s\n' "$report" > "$report_file"

gh pr comment "$pr" --body-file "$report_file"

verdict=$(head -n 1 "$report_file" | sed -nE 's/.* — (.*)$/\1/p')
echo "Report: $report_file"
echo "VERDICT: ${verdict:-unknown}"
