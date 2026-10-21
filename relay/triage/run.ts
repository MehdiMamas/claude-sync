// Feedback triage. A local script: the maintainer's machine runs it (by hand or from
// cron/launchd, see relay/README.md) and Claude runs through the local `claude` CLI, signed
// in with the maintainer's own Claude account. No API key, no hosted runner.
//
//   prepare  Picks the open feedback issues not triaged yet, checks the relay labelled them,
//            writes issue.md / open-issues.md and checks out each issue's attachments.
//   debounce Waits until the newest of them is at least 10 minutes old, so a burst of
//            reports is handled together.
//   analyze  ONE `claude -p --model sonnet` session per batch of 5 reports, read-only tools,
//            no GitHub token in its environment. Prints no report content.
//   post     Sanitizes each answer, comments and labels; or moderates. Writes only as the
//            triage GitHub App bot (installation token, see bot.ts), never as the maintainer.
//
// node relay/triage/run.ts [--issue <n|all>] [--force] [--dry-run] [--model sonnet]
//                          [--out <dir>] [--attachments <clone>] [--min-age-minutes 10]

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { botConfig, botToken, NOT_CONFIGURED } from "./bot.ts";
import { discard, github, githubJson, repoPath } from "../lib/github.ts";
import {
  BATCH_SIZE,
  buildBatchPrompt,
  MIN_AGE_MS,
  parseBatch,
  waitMs,
  type IssueData,
  type IssueEvent,
  issueMarkdown,
  labelledByTrusted,
  MODERATION_LABEL,
  MODERATION_NOTE,
  needsTriage,
  openIssuesMarkdown,
  TRIAGED_LABEL,
  triageComment,
  trustedLogins,
  type Verdict,
} from "./lib.ts";

export interface Queued {
  number: number;
  issueDir: string;
  attachmentsDir: string | null;
  createdAt?: string;
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
    queue.issues.push({ number, issueDir, attachmentsDir, ...(issue.created_at ? { createdAt: issue.created_at } : {}) });
    ctx.log(`#${number}: queued (${attachmentsDir ? "with" : "no"} attachments)`);
  }
  writeFileSync(join(ctx.out, "queue.json"), JSON.stringify(queue, null, 2));
  return queue;
}

// Claude gets no GitHub token: only what it needs to run and use the local login.
const CLAUDE_ENV = ["PATH", "HOME", "USER", "LANG", "TMPDIR", "TEMP", "TMP", "SYSTEMROOT", "USERPROFILE", "APPDATA", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CONFIG_DIR"];

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

export interface ClaudeRequest {
  prompt: string;
  cwd: string;
  addDirs: string[];
  model: string;
}

/** Runs one Claude session and returns its answer text. Injected so tests never start `claude`. */
export type ClaudeRunner = (req: ClaudeRequest) => string;

/** The real runner: `claude -p --model <model> --output-format json`, prompt on stdin, the local login. */
export function localClaude(bin = "claude"): ClaudeRunner {
  return (req) => {
    const env: Record<string, string> = { DISABLE_AUTOUPDATER: "1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1" };
    for (const k of CLAUDE_ENV) if (process.env[k] !== undefined) env[k] = process.env[k] as string;
    const args = [
      "-p",
      "--model",
      req.model,
      "--output-format",
      "json",
      "--max-turns",
      "150",
      "--allowedTools",
      "Read,Grep,Glob",
      "--disallowedTools",
      DISALLOWED.join(","),
      ...req.addDirs.flatMap((d) => ["--add-dir", d]),
    ];
    const stdout = execFileSync(bin, args, {
      cwd: req.cwd,
      env,
      input: req.prompt,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      timeout: 30 * 60 * 1000,
      stdio: ["pipe", "pipe", "ignore"],
    });
    const answer = JSON.parse(stdout) as { result?: unknown; is_error?: boolean; subtype?: string };
    if (answer.is_error || typeof answer.result !== "string") throw new Error(`Claude ended with ${answer.subtype ?? "an error"}`);
    return answer.result;
  };
}

export function analyze(opts: {
  out: string;
  runner: ClaudeRunner;
  model: string;
  repo: string;
  cwd: string;
  batchSize?: number;
  log: (line: string) => void;
}): Result[] {
  const queue = JSON.parse(readFileSync(join(opts.out, "queue.json"), "utf8")) as Queue;
  const size = Math.max(1, opts.batchSize ?? BATCH_SIZE);
  const results: Result[] = [];
  for (let i = 0; i < queue.issues.length; i += size) {
    const batch = queue.issues.slice(i, i + size);
    const prompt = buildBatchPrompt({ repo: opts.repo, items: batch });
    let verdicts: Map<number, Result>;
    try {
      const answer = opts.runner({ prompt, cwd: opts.cwd, addDirs: batch.map((q) => q.issueDir), model: opts.model });
      verdicts = parseBatch(answer, batch.map((q) => q.number));
    } catch (err) {
      // The message names the failure only; Claude's output is never printed.
      const message = (err instanceof Error ? err.message.split("\n")[0] : String(err)) ?? "";
      verdicts = new Map(batch.map((q) => [q.number, { error: message.slice(0, 300) }]));
    }
    for (const q of batch) {
      const result = verdicts.get(q.number) ?? { error: "no result" };
      opts.log("error" in result ? `#${q.number}: analysis failed: ${result.error}` : `#${q.number}: verdict ${result.verdict}`);
      writeFileSync(join(q.issueDir, "result.json"), JSON.stringify(result));
      results.push(result);
    }
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
  // Writes only ever use the bot's installation token; there is no other source for it.
  if (!ctx.token) throw new Error(NOT_CONFIGURED);
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

export type RunOutcome = { status: "nothing" | "waiting" | "dry-run" | "done"; waitMs?: number; failed?: number };

/** The whole run: prepare, debounce, analyze in batches, post. */
export async function runTriage(
  ctx: Ctx & { owner: string; trusted?: string; issue: string; force: boolean; out: string; attachments?: string; sha: string },
  runner: ClaudeRunner,
  opts: { model: string; cwd: string; minAgeMs?: number; now?: () => number; dryRun?: boolean },
): Promise<RunOutcome> {
  const queue = await prepare(ctx);
  if (queue.issues.length === 0) {
    ctx.log("nothing to triage");
    return { status: "nothing" };
  }
  // An explicit issue number is a manual request: no waiting. "all" is debounced.
  if (ctx.issue === "all") {
    const wait = waitMs(queue.issues.map((q) => q.createdAt), (opts.now ?? Date.now)(), opts.minAgeMs ?? MIN_AGE_MS);
    if (wait > 0) {
      ctx.log(`waiting: the newest report is less than ${Math.round((opts.minAgeMs ?? MIN_AGE_MS) / 60000)} minutes old, try again in ${Math.ceil(wait / 60000)} minutes`);
      return { status: "waiting", waitMs: wait };
    }
  }
  if (opts.dryRun) {
    ctx.log(`dry run: would triage ${queue.issues.map((q) => `#${q.number}`).join(", ")}`);
    return { status: "dry-run" };
  }
  analyze({ out: ctx.out, runner, model: opts.model, repo: ctx.repo, cwd: opts.cwd, log: ctx.log });
  // A failed analysis does not stop the run: the others are posted, and the exit code says some failed.
  const failed = await post({ ...ctx, out: ctx.out, sha: ctx.sha });
  return { status: "done", failed };
}

function flag(args: string[], name: string): string | undefined {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : undefined;
}

function sh(cmd: string, args: string[]): string {
  return execFileSync(cmd, args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
}

/**
 * The token for this run. A real run needs the bot (throws when it is not configured); a
 * dry run reads with the bot when configured and otherwise unauthenticated. Never `gh auth
 * token`, GH_TOKEN or GITHUB_TOKEN.
 */
export async function runToken(
  dryRun: boolean,
  env: Record<string, string | undefined> = process.env,
  fetchFn: typeof fetch = fetch,
): Promise<string> {
  if (!dryRun || botConfig(env)) return botToken(env, fetchFn);
  return "";
}

async function main(argv: string[]): Promise<number> {
  const args = argv;
  const log = (line: string): void => console.log(line);
  const repo = process.env.GITHUB_REPOSITORY ?? "MehdiMamas/claude-sync";
  const given = flag(args, "out");
  const out = resolve(given ?? mkdtempSync(join(tmpdir(), "claude-sync-triage-")));
  mkdirSync(out, { recursive: true });
  let attachments = flag(args, "attachments");
  let cloned: string | undefined;
  if (!attachments) {
    // Blobs are not downloaded: prepare checks out only issues/<n>/ of the queued issues.
    const attRepo = process.env.FEEDBACK_ATTACHMENTS_REPO ?? `${repo}-feedback`;
    cloned = join(out, "attachments-clone");
    try {
      sh("gh", ["repo", "clone", attRepo, cloned, "--", "--depth", "1", "--filter=blob:none", "--no-checkout", "--quiet"]);
      attachments = cloned;
    } catch {
      log(`could not clone ${attRepo}: triaging without attachments`);
    }
  }
  try {
    const dryRun = args.includes("--dry-run");
    const token = await runToken(dryRun);
    const minutes = Number.parseFloat(flag(args, "min-age-minutes") ?? "10");
    const outcome = await runTriage(
      {
        repo,
        token,
        log,
        owner: repo.split("/")[0] ?? "",
        trusted: process.env.TRIAGE_TRUSTED_LOGINS,
        issue: flag(args, "issue") ?? "all",
        force: args.includes("--force"),
        out,
        attachments,
        sha: sh("git", ["rev-parse", "HEAD"]),
      },
      localClaude(process.env.CLAUDE_BIN || "claude"),
      { model: flag(args, "model") ?? "sonnet", cwd: process.cwd(), minAgeMs: minutes * 60_000, dryRun },
    );
    return outcome.failed ? 1 : 0;
  } finally {
    // The private attachments do not stay on disk.
    if (cloned) rmSync(cloned, { recursive: true, force: true });
    if (!given) rmSync(out, { recursive: true, force: true });
  }
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
