import { randomUUID } from "node:crypto";
import { requireCallerWithToken } from "../lib/auth.ts";
import { env } from "../lib/env.ts";
import { buildIssueBody, type Feedback, logsNeedFile, parseFeedback } from "../lib/feedback.ts";
import { discard, github, GitHubError, githubJson, type Milestone, repoPath } from "../lib/github.ts";
import { handler, HttpError, json, logError, logWarn } from "../lib/http.ts";
import { pickSprint } from "../lib/milestone.ts";
import { FEEDBACK_LIMIT, FEEDBACK_WINDOW_MS, take } from "../lib/ratelimit.ts";

const ASSETS_BRANCH = "feedback-assets";
// Vercel caps request bodies at 4.5 MB; refuse early above that.
const MAX_BODY_BYTES = 4_500_000;
// Statuses for creating the issue with the caller's token that come from the caller's
// side: a token without the scope, a user blocked from the repo, SSO, the user's own
// GitHub rate limit. The relay then files the issue with its own token, as before.
// 422 is included: the retry either works (the cause was the user's account) or fails
// the same way and answers 502 as before. 5xx and network errors are not: they answer
// 502/500 so the app keeps the report in its outbox and retries.
const FILE_WITH_RELAY_TOKEN = new Set([401, 403, 404, 410, 422, 429]);

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
 * 201 { number, url, asUser } with the new issue. asUser is false when the relay had to
 * file it with its own token.
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
  const issues = `${repoPath(repo)}/issues`;
  const fields = (reporter: string | null) => ({
    title: feedback.title,
    body: buildIssueBody({ feedback, reporter, ...assets }),
    labels,
    ...(milestone !== null ? { milestone } : {}),
  });

  // Opened with the caller's token, so GitHub shows them as the author.
  const asUser = await github(issues, { token: userToken, method: "POST", body: fields(null) });
  if (asUser.ok) {
    const issue = (await asUser.json()) as CreatedIssue;
    await triage(repo, relayToken, issue, labels, milestone);
    return json(201, { number: issue.number, url: issue.html_url, asUser: true });
  }
  discard(asUser);
  if (!FILE_WITH_RELAY_TOKEN.has(asUser.status)) throw new GitHubError("create issue", asUser.status);

  logWarn(`feedback: creating the issue as ${login} returned ${asUser.status}; filing it with the relay token`);
  const issue = await githubJson<CreatedIssue>("create issue", issues, {
    token: relayToken,
    method: "POST",
    body: fields(login),
  });
  return json(201, { number: issue.number, url: issue.html_url, asUser: false });
});
