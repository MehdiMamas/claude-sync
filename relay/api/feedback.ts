import { randomUUID } from "node:crypto";
import { requireCallerWithToken } from "../lib/auth.ts";
import { env } from "../lib/env.ts";
import { buildIssueBody, type Feedback, logsNeedFile, parseFeedback } from "../lib/feedback.ts";
import { discard, github, GitHubError, githubJson, type Milestone, repoPath } from "../lib/github.ts";
import { handler, HttpError, json, logError, logWarn } from "../lib/http.ts";
import { redact } from "../lib/redact.ts";
import { pickSprint } from "../lib/milestone.ts";
import { FEEDBACK_LIMIT, FEEDBACK_WINDOW_MS, take } from "../lib/ratelimit.ts";

const ASSETS_BRANCH = "feedback-assets";
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
  labels?: Array<string | { name?: string }>;
  milestone?: { number?: number } | null;
}

function refPath(branch: string): string {
  return branch.split("/").map(encodeURIComponent).join("/");
}

/** Creates `feedback-assets` from the default branch head when it does not exist. */
async function ensureAssetsBranch(repo: string, token: string): Promise<void> {
  const base = repoPath(repo);
  const res = await github(`${base}/git/ref/heads/${ASSETS_BRANCH}`, { token });
  discard(res);
  if (res.ok) return;
  if (res.status !== 404) throw new GitHubError("feedback-assets ref", res.status);

  const { default_branch } = await githubJson<{ default_branch: string }>("repo", base, { token });
  const head = await githubJson<{ object: { sha: string } }>(
    "default branch ref",
    `${base}/git/ref/heads/${refPath(default_branch)}`,
    { token },
  );
  const created = await github(`${base}/git/refs`, {
    token,
    method: "POST",
    body: { ref: `refs/heads/${ASSETS_BRANCH}`, sha: head.object.sha },
  });
  discard(created);
  // 422: another request created it first.
  if (!created.ok && created.status !== 422) throw new GitHubError("create feedback-assets", created.status);
}

/** Commits one file to `feedback-assets` and returns a URL that renders it. */
async function commitAsset(repo: string, token: string, path: string, bytes: Buffer, message: string): Promise<string> {
  const res = await github(`${repoPath(repo)}/contents/${refPath(path)}`, {
    token,
    method: "PUT",
    body: { message, content: bytes.toString("base64"), branch: ASSETS_BRANCH },
  });
  discard(res);
  if (!res.ok) throw new GitHubError("commit feedback asset", res.status);
  return `https://github.com/${repo}/blob/${ASSETS_BRANCH}/${path}?raw=true`;
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
 * GitHub silently drops labels and milestone sent by someone without push access, so
 * after the caller opens the issue the relay sets them. Best effort: the issue exists,
 * so a failure here is logged and the post still succeeds.
 */
async function triage(repo: string, token: string, issue: CreatedIssue, labels: string[], milestone: number | null): Promise<void> {
  const have = new Set((issue.labels ?? []).map((l) => (typeof l === "string" ? l : l.name)));
  const labelsSet = labels.every((l) => have.has(l));
  const milestoneSet = milestone === null || issue.milestone?.number === milestone;
  if (labelsSet && milestoneSet) return;
  try {
    const res = await github(`${repoPath(repo)}/issues/${issue.number}`, {
      token,
      method: "PATCH",
      body: { labels, ...(milestone !== null ? { milestone } : {}) },
    });
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

async function storeAssets(
  repo: string,
  token: string,
  feedback: Feedback,
): Promise<{ screenshotUrl: string | null | "failed"; fullLogsUrl: string | null | "failed" }> {
  const wantLogs = logsNeedFile(feedback);
  if (!feedback.screenshot && !wantLogs) return { screenshotUrl: null, fullLogsUrl: null };

  const now = new Date();
  const dir = `feedback/${now.toISOString().slice(0, 7)}`;
  const id = randomUUID();
  let screenshotUrl: string | null | "failed" = feedback.screenshot ? "failed" : null;
  let fullLogsUrl: string | null | "failed" = wantLogs ? "failed" : null;
  // Best effort: feedback without its screenshot beats no feedback.
  try {
    await ensureAssetsBranch(repo, token);
    if (feedback.screenshot) {
      const { bytes, ext } = feedback.screenshot;
      screenshotUrl = await commitAsset(repo, token, `${dir}/${id}.${ext}`, bytes, `feedback: screenshot ${id}`);
    }
    if (wantLogs) {
      fullLogsUrl = await commitAsset(repo, token, `${dir}/${id}.log`, Buffer.from(feedback.logs, "utf8"), `feedback: logs ${id}`);
    }
  } catch (err) {
    logError("feedback assets", err);
  }
  return { screenshotUrl, fullLogsUrl };
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
  // Users cannot push, so the relay commits the screenshot and logs.
  const assets = await storeAssets(repo, relayToken, feedback);
  const milestone = await nearestSprint(repo, relayToken);
  const labels = ["feedback", `type:${feedback.kind}`];
  const fields = {
    title: feedback.title,
    body: buildIssueBody({ feedback, ...assets }),
    labels,
    ...(milestone !== null ? { milestone } : {}),
  };

  // Opened with the caller's token, so GitHub shows them as the author.
  const res = await github(`${repoPath(repo)}/issues`, { token: userToken, method: "POST", body: fields });
  if (!res.ok) throw await refused(repo, login, res);
  const issue = (await res.json()) as CreatedIssue;
  await triage(repo, relayToken, issue, labels, milestone);
  return json(201, { number: issue.number, url: issue.html_url });
});
