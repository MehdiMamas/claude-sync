import { commitFiles } from "../lib/attachments.ts";
import { requireCallerWithToken } from "../lib/auth.ts";
import { env } from "../lib/env.ts";
import { attachmentsFor, buildIssueBody, type Feedback, parseFeedback } from "../lib/feedback.ts";
import { discard, github, GitHubError, githubJson, type Milestone, repoPath } from "../lib/github.ts";
import { handler, HttpError, json, logError, logWarn } from "../lib/http.ts";
import { redact } from "../lib/redact.ts";
import { pickSprint } from "../lib/milestone.ts";
import { FEEDBACK_LIMIT, FEEDBACK_WINDOW_MS, take } from "../lib/ratelimit.ts";

// Vercel caps request bodies at 4.5 MB; refuse early above that.
const MAX_BODY_BYTES = 4_500_000;
// Why GitHub refused the caller's create, in words the app can show. The relay never
// files the issue another way: an issue only ever exists under the account that wrote it.
const REFUSED: Record<number, string> = {
  401: "GitHub no longer accepts your sign-in; sign out and in again",
  403: "your account is blocked from the repository, or your sign-in lacks the repo scope",
  404: "the repository is not visible to your account",
  410: "issues are turned off on the repository",
  422: "GitHub rejected the issue",
};
const MAX_REASON = 200;

interface CreatedIssue {
  number: number;
  html_url: string;
  created_at?: string;
}

async function nearestSprint(repo: string, token: string): Promise<number | null> {
  try {
    const milestones = await githubJson<Milestone[]>(
      "milestones",
      `${repoPath(repo)}/milestones?state=open&per_page=100`,
      { token },
    );
    return pickSprint(milestones, new Date())?.number ?? null;
  } catch (err) {
    logError("feedback milestones", err);
    return null;
  }
}

/**
 * Commits the screenshot and logs to the private attachments repo under issues/<number>/.
 * Best effort: the issue exists, so a failure is logged and the issue says so.
 */
async function storeAttachments(token: string, feedback: Feedback, issue: CreatedIssue, login: string, repo: string): Promise<boolean> {
  const files = attachmentsFor(feedback, {
    number: issue.number,
    url: issue.html_url,
    login,
    repo,
    createdAt: issue.created_at ?? new Date().toISOString(),
  });
  try {
    await commitFiles(env.attachmentsRepo(), token, files, `feedback: ${repo}#${issue.number}`);
    return true;
  } catch (err) {
    logError("feedback attachments", err);
    return false;
  }
}

/**
 * GitHub silently drops labels and milestone sent by someone without push access, so
 * the relay sets them. The `feedback` label starts the triage workflow, which reads the
 * attachments, so this runs after they are stored. Best effort: a failure is logged and
 * the post still succeeds.
 */
async function label(repo: string, token: string, number: number, fields: Record<string, unknown>): Promise<void> {
  try {
    const res = await github(`${repoPath(repo)}/issues/${number}`, { token, method: "PATCH", body: fields });
    discard(res);
    if (!res.ok) throw new GitHubError("label issue", res.status);
  } catch (err) {
    logError("feedback labels", err);
  }
}

/** GitHub's own one-line message from an error body, redacted and cut short. */
async function githubMessage(res: Response): Promise<string> {
  try {
    const body = (await res.json()) as { message?: unknown };
    if (typeof body.message !== "string") return "";
    const line = redact(body.message).replace(/[\u0000-\u001f\u007f]+/g, " ").trim();
    return line.length > MAX_REASON ? `${line.slice(0, MAX_REASON - 1)}…` : line;
  } catch {
    return "";
  }
}

/**
 * The error for a create GitHub refused. A rate limit (429, or a 403 that carries one)
 * answers 429 and a 5xx answers 502, so the app keeps the report and sends it later.
 * Every other refusal answers 403 or 422 with a reason the app shows to the user.
 */
async function refused(repo: string, login: string, res: Response): Promise<HttpError> {
  const limited =
    res.status === 429 ||
    (res.status === 403 && (res.headers.has("retry-after") || res.headers.get("x-ratelimit-remaining") === "0"));
  if (limited) {
    discard(res);
    logWarn(`feedback: GitHub rate-limited ${login} creating the issue (${res.status})`);
    const wait = Number(res.headers.get("retry-after"));
    return new HttpError(429, "GitHub is rate-limiting your account, try again later", {
      "retry-after": String(Number.isFinite(wait) && wait > 0 ? Math.ceil(wait) : 60),
    });
  }
  const hint = REFUSED[res.status];
  if (!hint) {
    discard(res);
    return new GitHubError("create issue", res.status);
  }
  const detail = await githubMessage(res);
  logWarn(`feedback: GitHub refused the issue from ${login} with ${res.status}`);
  return new HttpError(
    res.status === 422 ? 422 : 403,
    `GitHub did not let your account open an issue on ${repo}: ${hint}${detail ? ` (GitHub said: ${detail})` : ""}.`,
  );
}

async function readBody(req: Request): Promise<unknown> {
  const length = Number(req.headers.get("content-length") ?? "0");
  if (length > MAX_BODY_BYTES) throw new HttpError(413, "request body is too large");
  const text = await req.text();
  if (Buffer.byteLength(text) > MAX_BODY_BYTES) throw new HttpError(413, "request body is too large");
  try {
    return JSON.parse(text);
  } catch {
    throw new HttpError(400, "body must be JSON");
  }
}

/**
 * POST /api/feedback
 * JSON { title, description, kind: "bug"|"idea", appVersion, os, logs?, screenshot? }
 * 201 { number, url } with the new issue, opened under the caller's account. When GitHub
 * refuses the caller: 403/422 with the reason (the app shows it), or 429/502 (the app retries).
 */
export const POST = handler("feedback", async (req) => {
  const { login, token: userToken } = await requireCallerWithToken(req);
  const feedback = parseFeedback(await readBody(req));

  const slot = take(`feedback:${login}`, FEEDBACK_LIMIT, FEEDBACK_WINDOW_MS);
  if (!slot.ok) {
    throw new HttpError(429, "too many feedback posts, try again later", {
      "retry-after": String(slot.retryAfterSeconds),
    });
  }

  const repo = env.feedbackRepo();
  const relayToken = env.relayToken();

  // Opened with the caller's token, so GitHub shows them as the author. The body holds
  // nothing from the user's disk, and no labels: those come last (see `label`).
  const res = await github(`${repoPath(repo)}/issues`, {
    token: userToken,
    method: "POST",
    body: { title: feedback.title, body: buildIssueBody(feedback) },
  });
  if (!res.ok) throw await refused(repo, login, res);
  const issue = (await res.json()) as CreatedIssue;

  // Users cannot push, so the relay commits the screenshot and logs, privately.
  const stored = await storeAttachments(relayToken, feedback, issue, login, repo);
  const milestone = await nearestSprint(repo, relayToken);
  await label(repo, relayToken, issue.number, {
    labels: ["feedback", `type:${feedback.kind}`],
    ...(milestone !== null ? { milestone } : {}),
    ...(stored ? {} : { body: buildIssueBody(feedback, false) }),
  });
  return json(201, { number: issue.number, url: issue.html_url });
});
