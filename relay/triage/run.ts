// Feedback triage, run by .github/workflows/feedback-triage.yml in three steps so no
// step holds more than it needs:
//
//   prepare  GH_TOKEN + the attachments clone. Picks the issues, checks the relay labelled
//            them, writes issue.md / open-issues.md and checks out each issue's attachments.
//   analyze  CLAUDE_CODE_OAUTH_TOKEN only. Runs Claude (read-only tools) on each issue and
//            writes its answer to result.json. Prints no report content: Actions logs of a
//            public repo are public.
//   post     GH_TOKEN. Sanitizes the answer, comments, labels; or moderates.
//
// node relay/triage/run.ts prepare --issue <n|all> [--force] --out <dir> [--attachments <clone>]
// node relay/triage/run.ts analyze --out <dir> --claude <bin> [--model opus]
// node relay/triage/run.ts post --out <dir>

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { discard, github, githubJson, repoPath } from "../lib/github.ts";
import {
  buildPrompt,
  type IssueData,
  type IssueEvent,
  issueMarkdown,
  labelledByTrusted,
  MODERATION_LABEL,
  MODERATION_NOTE,
  needsTriage,
  openIssuesMarkdown,
  parseVerdict,
  TRIAGED_LABEL,
  triageComment,
  trustedLogins,
  type Verdict,
} from "./lib.ts";

export interface Queued {
  number: number;
  issueDir: string;
  attachmentsDir: string | null;
}

export interface Queue {
  force: boolean;
  issues: Queued[];
}

export type Result = Verdict | { error: string };

interface Ctx {
  repo: string;
  token: string;
  log: (line: string) => void;
}

const MAX_PAGES = 10;

async function all<T>(ctx: Ctx, path: string): Promise<T[]> {
  const out: T[] = [];
  const sep = path.includes("?") ? "&" : "?";
  for (let page = 1; page <= MAX_PAGES; page++) {
    const rows = await githubJson<T[]>(path, `${path}${sep}per_page=100&page=${page}`, { token: ctx.token });
    out.push(...rows);
    if (rows.length < 100) break;
  }
  return out;
}

function git(args: string[]): string {
  return execFileSync("git", args, { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] });
}

/** Checks out issues/<n>/ of the attachments clone into `dir`; null when it has none. */
function checkoutAttachments(clone: string, number: number, dir: string): string | null {
  const path = `issues/${number}`;
  if (!git(["-C", clone, "ls-tree", "-d", "--name-only", "HEAD", path]).trim()) return null;
  mkdirSync(dir, { recursive: true });
  git(["-C", clone, "-c", "core.autocrlf=false", `--work-tree=${dir}`, "checkout", "HEAD", "--", path]);
  return join(dir, path);
}

export async function prepare(
  ctx: Ctx & { owner: string; trusted?: string; issue: string; force: boolean; out: string; attachments?: string },
): Promise<Queue> {
  const base = repoPath(ctx.repo);
  const trusted = trustedLogins(ctx.owner, ctx.trusted);
  const open = await all<IssueData>(ctx, `${base}/issues?state=open`);
  const numbers =
    ctx.issue === "all"
      ? open.filter((i) => !i.pull_request).map((i) => i.number)
      : [Number.parseInt(ctx.issue, 10)];
  if (numbers.some((n) => !Number.isInteger(n) || n <= 0)) throw new Error(`--issue must be a number or "all"`);

  const queue: Queue = { force: ctx.force, issues: [] };
  for (const number of numbers) {
    const issue = await githubJson<IssueData>("issue", `${base}/issues/${number}`, { token: ctx.token });
    const comments = await all<{ body?: string | null }>(ctx, `${base}/issues/${number}/comments`);
    if (!needsTriage(issue, comments, ctx.force)) {
      ctx.log(`#${number}: skipped (closed, not feedback, or already triaged)`);
      continue;
    }
    const events = await all<IssueEvent>(ctx, `${base}/issues/${number}/events`);
    if (!labelledByTrusted(events, trusted)) {
      ctx.log(`#${number}: skipped (the feedback label was not added by the relay)`);
      continue;
    }
    const issueDir = resolve(ctx.out, String(number));
    mkdirSync(issueDir, { recursive: true });
    writeFileSync(join(issueDir, "issue.md"), issueMarkdown(issue));
    writeFileSync(join(issueDir, "open-issues.md"), openIssuesMarkdown(open, number));
    const attachmentsDir = ctx.attachments ? checkoutAttachments(ctx.attachments, number, join(issueDir, "private")) : null;
    queue.issues.push({ number, issueDir, attachmentsDir });
    ctx.log(`#${number}: queued (${attachmentsDir ? "with" : "no"} attachments)`);
  }
  writeFileSync(join(ctx.out, "queue.json"), JSON.stringify(queue, null, 2));
  return queue;
}

// Claude gets no GitHub token: only what it needs to run and sign in.
const CLAUDE_ENV = ["PATH", "HOME", "USER", "LANG", "TMPDIR", "TEMP", "TMP", "SYSTEMROOT", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CONFIG_DIR"];

const DISALLOWED = [
  "Bash",
  "Edit",
  "Write",
  "NotebookEdit",
  "WebFetch",
  "WebSearch",
  "Task",
  "Agent",
  "Read(//proc/**)",
  "Read(//sys/**)",
  "Read(//etc/**)",
];

export function analyze(opts: { out: string; claude: string[]; model: string; cwd: string; log: (line: string) => void }): Result[] {
  const queue = JSON.parse(readFileSync(join(opts.out, "queue.json"), "utf8")) as Queue;
  const env: Record<string, string> = { DISABLE_AUTOUPDATER: "1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1" };
  for (const k of CLAUDE_ENV) if (process.env[k] !== undefined) env[k] = process.env[k] as string;

  const results: Result[] = [];
  for (const q of queue.issues) {
    const prompt = buildPrompt({ repo: process.env.GITHUB_REPOSITORY ?? "", number: q.number, issueDir: q.issueDir, attachmentsDir: q.attachmentsDir });
    const [bin, ...pre] = opts.claude;
    const args = [
      ...pre,
      "-p",
      "--output-format",
      "json",
      "--model",
      opts.model,
      "--max-turns",
      "80",
      "--allowedTools",
      "Read,Grep,Glob",
      "--disallowedTools",
      DISALLOWED.join(","),
      "--add-dir",
      q.issueDir,
    ];
    let result: Result;
    try {
      const stdout = execFileSync(bin as string, args, {
        cwd: opts.cwd,
        env,
        input: prompt,
        encoding: "utf8",
        maxBuffer: 64 * 1024 * 1024,
        timeout: 20 * 60 * 1000,
        stdio: ["pipe", "pipe", "ignore"],
      });
      const answer = JSON.parse(stdout) as { result?: unknown; is_error?: boolean; subtype?: string };
      if (answer.is_error || typeof answer.result !== "string") throw new Error(`Claude ended with ${answer.subtype ?? "an error"}`);
      result = parseVerdict(answer.result);
      opts.log(`#${q.number}: verdict ${result.verdict}`);
    } catch (err) {
      // The message names the failure only; Claude's output is never printed.
      const message = err instanceof Error ? err.message.split("\n")[0] ?? "" : String(err);
      result = { error: message.slice(0, 300) };
      opts.log(`#${q.number}: analysis failed: ${result.error}`);
    }
    writeFileSync(join(q.issueDir, "result.json"), JSON.stringify(result));
    results.push(result);
  }
  return results;
}

async function call(ctx: Ctx, what: string, path: string, method: string, body?: unknown): Promise<void> {
  const res = await github(path, { token: ctx.token, method, body });
  discard(res);
  if (!res.ok) throw new Error(`GitHub ${what} returned ${res.status}`);
}

/** Posts every result. Returns how many issues failed, so the job can fail. */
export async function post(ctx: Ctx & { out: string; sha: string }): Promise<number> {
  const queue = JSON.parse(readFileSync(join(ctx.out, "queue.json"), "utf8")) as Queue;
  const base = repoPath(ctx.repo);
  let failed = 0;
  for (const q of queue.issues) {
    const file = join(q.issueDir, "result.json");
    const result = existsSync(file) ? (JSON.parse(readFileSync(file, "utf8")) as Result) : { error: "no result" };
    if ("error" in result) {
      failed++;
      ctx.log(`#${q.number}: nothing posted, the analysis failed`);
      continue;
    }
    try {
      // Someone may have triaged or closed it while Claude was reading.
      const issue = await githubJson<IssueData>("issue", `${base}/issues/${q.number}`, { token: ctx.token });
      const comments = await all<{ body?: string | null }>(ctx, `${base}/issues/${q.number}/comments`);
      if (!needsTriage(issue, comments, queue.force)) {
        ctx.log(`#${q.number}: skipped (changed while it was analysed)`);
        continue;
      }
      const issuePath = `${base}/issues/${q.number}`;
      if (result.verdict === "abusive") {
        await call(ctx, "comment", `${issuePath}/comments`, "POST", { body: MODERATION_NOTE });
        await call(ctx, "label", `${issuePath}/labels`, "POST", { labels: [MODERATION_LABEL] });
        await call(ctx, "close", issuePath, "PATCH", { state: "closed", state_reason: "not_planned" });
        await call(ctx, "lock", `${issuePath}/lock`, "PUT", { lock_reason: "spam" });
        ctx.log(`#${q.number}: closed, locked and labelled ${MODERATION_LABEL}`);
      } else {
        await call(ctx, "comment", `${issuePath}/comments`, "POST", { body: triageComment(result.body, ctx.repo, ctx.sha) });
        await call(ctx, "label", `${issuePath}/labels`, "POST", { labels: [TRIAGED_LABEL] });
        ctx.log(`#${q.number}: triage comment posted`);
      }
    } catch (err) {
      failed++;
      ctx.log(`#${q.number}: posting failed: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
  return failed;
}

function flag(args: string[], name: string): string | undefined {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : undefined;
}

function need(value: string | undefined, name: string): string {
  if (!value) throw new Error(`${name} is required`);
  return value;
}

async function main(argv: string[]): Promise<number> {
  const [step, ...args] = argv;
  const log = (line: string): void => console.log(line);
  const out = resolve(need(flag(args, "out"), "--out"));
  if (step === "analyze") {
    const claude = need(flag(args, "claude"), "--claude");
    // A failed analysis does not stop the job: post still runs for the others and fails it.
    analyze({ out, claude: [claude], model: flag(args, "model") ?? "opus", cwd: process.cwd(), log });
    return 0;
  }
  const ctx: Ctx = {
    repo: need(process.env.GITHUB_REPOSITORY, "GITHUB_REPOSITORY"),
    token: need(process.env.GH_TOKEN, "GH_TOKEN"),
    log,
  };
  if (step === "prepare") {
    mkdirSync(out, { recursive: true });
    await prepare({
      ...ctx,
      owner: need(process.env.GITHUB_REPOSITORY_OWNER, "GITHUB_REPOSITORY_OWNER"),
      trusted: process.env.TRIAGE_TRUSTED_LOGINS,
      issue: need(flag(args, "issue"), "--issue"),
      force: args.includes("--force"),
      out,
      attachments: flag(args, "attachments"),
    });
    return 0;
  }
  if (step === "post") {
    const failed = await post({ ...ctx, out, sha: need(process.env.GITHUB_SHA, "GITHUB_SHA") });
    if (failed > 0) log(`${failed} issue(s) could not be triaged`);
    return failed > 0 ? 1 : 0;
  }
  throw new Error("usage: run.ts prepare|analyze|post …");
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(import.meta.filename)) {
  main(process.argv.slice(2)).then(
    (code) => process.exit(code),
    (err: unknown) => {
      console.error(err instanceof Error ? err.message : String(err));
      process.exit(1);
    },
  );
}
