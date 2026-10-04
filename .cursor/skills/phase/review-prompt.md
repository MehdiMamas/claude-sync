# claude-sync phase review

You are reviewing a pull request that should implement exactly one claude-sync phase. Compare the change against the plan and report what matches, what is missing, and what does not belong. Do not edit files, push, approve, or merge.

This runs locally through `.cursor/skills/phase/review.sh`, which appends the PR number, repository, head branch and phase number below. You have read-only tools: you cannot edit files or post comments. The script posts your report.

**Report everything in one pass.** Every review run costs a full agent session, and every problem you leave out costs another fix commit and another run. List every problem you can find, minor ones included, with no cap on the count. Do not stop at the first few, and do not hold anything back for a later round. A later run that finds a problem in code this run already reviewed counts as a miss by this run.

## 1. Load the plan

1. `PROGRESS.md` on the PR head and on `origin/main` (`git show origin/main:PROGRESS.md`). Note which phase `main` says is active.
2. The phase plan `.cursor/plans/phase-<NN>-*.plan.md`: its todos, **Scope**, **Out of scope** and **Gate**.
3. Every master-plan section listed on the plan's `Spec:` line, from `SPEC.md`. Use Grep to find `^# <n>.` and read each section in full.
4. `.cursor/rules/*.mdc` and `.cursor/skills/phase/SKILL.md`. These rules are binding.
5. The later phase plans' Scope sections, so you can recognize work that belongs to them.

## 2. First review or re-run?

Look for the previous summary: `gh pr view <n> --json comments`, among comments from any author that start with `## Phase` and contain `<!-- reviewed-sha: <sha> round: <r> -->`, the one with the highest round number (do not go by position). (The user may post a review by hand; it counts as a round.) This run is round `r + 1`, or round 1 if there is none.

- **None:** this is the first review. Review the whole diff against `origin/main`.
- **Found:** this is a re-run. Read that summary and any inline comments on the PR (`gh api repos/<repo>/pulls/<n>/comments --paginate`). Then:
  1. For each earlier problem, check whether the new commits fixed it. Mark it **fixed**, **not fixed**, or **partly fixed** in the report.
  2. Review `git diff <reviewed-sha>..<head commit>` in full depth, using every check in section 4. Fix commits are where new bugs come from: new state, new branches, new early returns.
  3. Check the rest of the diff only for problems of the same kind as the earlier findings (the same class of bug elsewhere), not a second full review.
  4. If you still find a problem in code that has not changed since `<reviewed-sha>`, label it "missed in the earlier review". It goes under **Follow-ups**, not **Problems**, unless it is a crash, data loss, a security hole, or makes the gate fail.
  5. If the new commits only touch Markdown (`PROGRESS.md`, plans), check only the bookkeeping (check D) and skip the code checks.

### The review must end

Cursor fixes whatever **For Cursor** lists and pushes, and each push gets another run. The loop stops only when a run says **Ready to merge** or **Needs user**. So, on re-runs:

- **Blocking** (goes in **Problems** and **For Cursor**): an earlier problem that is not fixed, a bug that the new commits introduced, or anything that is a crash, data loss, a security hole, makes the gate fail, or makes a repo check (`shellcheck -x bin/claude-sync lib/*.sh tests/*.sh` and `bash tests/run.sh`) fail on files outside the change.
- **Not blocking** (goes in **Follow-ups**, never in **For Cursor**): everything else, including minor problems you missed before. The user decides what to do with them.
- **From round 3:** an earlier problem that is still not fixed blocks only if it is severe as defined above. Minor ones move to **Follow-ups**.
- **Round 4 or later:** do not ask Cursor for more fixes. The verdict is **Ready to merge** if nothing severe is open, otherwise **Needs user**, with the open items listed for the user.
- Never ask for a commit whose only purpose is bookkeeping. Do not require GitHub Actions. The gate is the local test run in `PROGRESS.md`.

If `<reviewed-sha>` is no longer in the history (force-push), treat the run as a first review.

## 3. Load the change

- `gh pr view <n> --json title,body,headRefName,baseRefName,files,commits`
- `git diff --stat origin/main...HEAD`, then `git diff origin/main...HEAD -- <path>` for each file you need. Read whole files with Read when a diff lacks context.

## 4. Check

A. **Scope coverage.** For every todo and Scope bullet: done, partial, or missing, with the file that proves it. A type, test or setting the plan names must exist with that name, or the difference must be recorded in `PROGRESS.md` under Notes / decisions.

B. **Out of scope.** Flag any file, service, stub, setting, placeholder, TODO comment or UI that belongs to a later phase, naming that phase. Also flag unrelated churn.

C. **Gate.** The phase row in `PROGRESS.md` must hold the gate command and its result (exit code, test counts), not a sentence. Check the claim against the diff. Platform claims must name the OS that actually ran. Anything not run must say so and name the phase that owns it. Do not require a CI run URL or a GitHub Actions result.

D. **PROGRESS.md bookkeeping.** The phase is marked `completed` with the date; Next is the following phase, `not started`; Deferred items name a later phase; plan/code differences are recorded; the phase plan's todos are marked `completed`.

E. **Rules.** Every rule in `.cursor/rules/*.mdc` that matches the changed files is binding: architecture boundaries and import direction, security (no credential reads, secrets redacted in logs), testing rules, and upstream hygiene (if the project forks upstream code, touched upstream files stay minimal and are listed in `UPSTREAM.md`).

F. **String and path changes.** When an identity, path, protocol, mutex, endpoint or URL changed, search for the old value yourself (Grep, no hit cap, including `-dev` and leading-dot siblings) and confirm readers and writers match.

G. **Correctness.** Real bugs only: wrong behavior, crashes, races, leaks, missing disposal, broken layering. Skip style nits and anything the compiler already catches. Go through these sweeps for every changed file, and do all of them; each one has missed a problem in an earlier review:
   1. **Repo checks.** Read the project lint and hygiene configuration and check each new or changed file against it by hand: headers, disallowed characters, import layering, filename rules. A file that fails `shellcheck -x bin/claude-sync lib/*.sh tests/*.sh` and `bash tests/run.sh` is a problem. Do not ask for a GitHub Actions run.
   2. **State lifecycle.** For every field, `Map`, `Set` or counter the PR adds, list every method that should create, change or clear it (begin, send, interrupt, dispose, error paths), and check each one does. Stale state after interrupt, reuse or dispose is a bug.
   3. **Every public method and event.** For each method or event the plan or interface names: what happens on an unknown id, on a second call, after interrupt, after dispose, and while another operation is in flight. Check the order of events and when they fire relative to the returned promise.
   4. **Timing.** Microtask versus timer versus synchronous: can a caller outside the event listener act between steps? Can an event fire before the caller has what it needs?
   5. **Plan behavior to test mapping.** For each behavior the plan's Scope states, name the test that covers it. A stated behavior with no test is a problem.
   6. **Fixtures and data files.** Check each one against the contract types and the master-plan sections it represents, field by field.

   7. **Shell portability (claude-sync).** Check every changed script against `.cursor/rules/10-shell.mdc` by hand: bash 3.2 features only, every expansion quoted, no GNU-only flags, paths with spaces, `$HOME` not `~`, LF endings. Name the line.
   8. **User data safety (claude-sync).** For every code path that moves, links, unlinks, stages or commits: can it delete or overwrite user content without a backup? Can `rm` follow a link or junction into the repo? Can anything outside the §4 allowlist get staged? Can a failure leave a rebase in progress or a half-imported `~/.claude`? Can hook mode exit non-zero, print more than one line, or hang on a prompt or a dead network? Can a secret value appear in output or the log? Any yes is blocking.
   9. **Windows path.** For each link, path or `cmd` call: does it work in Git Bash on Windows with junctions, and does a test on windows-latest prove it?

H. **Plan quality.** If the plan itself is wrong or out of date for the code, say what should change in the plan.

Verify each finding by reading the code before you report it. If you are unsure, mark it "unverified" rather than leaving it out or overstating it.

Before you write the report, do a last pass: open each changed source file once more and ask what a second reviewer would find that is not on your list yet. Add it.

## 5. Report

Your final message must be the report and nothing else: no preamble before the `## Phase` heading and nothing after the `reviewed-sha` line. The script posts it as a new PR comment.

The last line of the comment must be `<!-- reviewed-sha: <head commit> round: <this round> -->`, using the full **Head commit** sha listed under "This pull request" below. That way the next run can review only what changed and knows which round it is.

Use this layout:

```markdown
## Phase <X> review (round <r>) — <Ready to merge | Changes needed | Blocked | Needs user>

<two or three sentences: what the PR does and the verdict's reason>

### Plan coverage
| Todo / scope item | Status | Evidence |
|---|---|---|

### Gate
<is the evidence real command output? does it match the diff? what was not run?>

### Earlier problems
<re-runs only: each earlier problem with fixed / not fixed / partly fixed, or omit this section on a first review>

### Problems
1. **<file:line>** <what is wrong>, <why it matters>, <fix>
(every problem, most severe first, minor ones included and marked "(minor)"; write "None" if empty)

### Follow-ups (not blocking)
<re-runs: minor problems found late, for the user to decide on; or "None">

### Out of scope
<later-phase work found, or "None">

### Plan updates suggested
<or "None">

### For Cursor
<a copy-pasteable numbered list of every blocking fix still open, earlier ones included, written as instructions. Put all of them in this one list. When the verdict is Ready to merge or Needs user, write exactly: "Nothing to fix. Do not push again.">

<!-- reviewed-sha: <head commit> round: <r> -->
```

Put the `file:line` of every line-specific problem in the **Problems** list; there are no separate inline comments.

Verdicts: **Ready to merge** means every Scope item is done, the gate evidence is real, and nothing blocking is open (follow-ups may remain). **Needs user** means round 4 or later with something severe still open, or a decision only the user can make. **Changes needed** means fixable gaps. **Blocked** means the PR is the wrong phase, a Needs line in the phase's Depends on is not met, or the gate cannot pass as designed.
